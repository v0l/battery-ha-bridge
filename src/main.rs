//! Home Assistant MQTT bridge for `battery_control`.
//!
//! Discovers batteries (BLE / serial / CAN), connects to each, and exposes
//! them to Home Assistant via MQTT Discovery: sensors for telemetry, switches
//! for capability-gated controls, a number entity for the charge limit.

mod hass;

use battery_control::{Battery, Command, DiscoverOptions, discover, resolve};
use clap::Parser;
use log::{error, info, warn};
use rumqttc::{AsyncClient, Event, Incoming, LastWill, MqttOptions, QoS};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{Mutex, broadcast, mpsc};

#[derive(Parser, Debug, Clone)]
#[command(name = "battery-ha-bridge", version, about)]
struct Args {
    #[arg(long, env = "MQTT_HOST", default_value = "core-mosquitto")]
    mqtt_host: String,
    #[arg(long, env = "MQTT_PORT", default_value_t = 1883)]
    mqtt_port: u16,
    #[arg(long, env = "MQTT_USERNAME")]
    mqtt_username: Option<String>,
    #[arg(long, env = "MQTT_PASSWORD")]
    mqtt_password: Option<String>,
    /// Device id/label queries to bridge (repeatable / comma-separated).
    /// Empty bridges everything discovered.
    #[arg(long = "device", env = "DEVICES", value_delimiter = ',')]
    devices: Vec<String>,
    /// Poll interval in seconds.
    #[arg(long, env = "POLL_INTERVAL", default_value_t = 15)]
    interval: u64,
    /// BLE scan duration in seconds.
    #[arg(long, env = "BLE_SECS", default_value_t = 6)]
    ble_secs: u64,
    /// Seconds between background rediscovery scans.
    #[arg(long, env = "RESCAN_SECS", default_value_t = 30)]
    rescan_secs: u64,
    /// Home Assistant MQTT discovery prefix.
    #[arg(long, env = "DISCOVERY_PREFIX", default_value = "homeassistant")]
    discovery_prefix: String,
    /// Base topic for state/availability/command topics.
    #[arg(long, env = "BASE_TOPIC", default_value = "battery_control")]
    base_topic: String,
}

#[tokio::main(flavor = "multi_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    if let Err(e) = run(args).await {
        error!("fatal: {e}");
        std::process::exit(1);
    }
}

async fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    // --- MQTT (connect first; devices come and go independently) ------------
    let bridge_avail_t = format!("{}/bridge/availability", args.base_topic);
    let mut mq = MqttOptions::new("battery-ha-bridge", &args.mqtt_host, args.mqtt_port);
    mq.set_keep_alive(Duration::from_secs(30));
    mq.set_last_will(LastWill::new(&bridge_avail_t, "offline", QoS::AtLeastOnce, true));
    if let (Some(u), Some(p)) = (&args.mqtt_username, &args.mqtt_password) {
        mq.set_credentials(u, p);
    }
    let (client, mut eventloop) = AsyncClient::new(mq, 64);

    client.subscribe(format!("{}/+/set/+", args.base_topic), QoS::AtLeastOnce).await?;
    client.subscribe(format!("{}/+/setv/+", args.base_topic), QoS::AtLeastOnce).await?;
    client.subscribe(format!("{}/status", args.discovery_prefix), QoS::AtLeastOnce).await?;
    client.publish(&bridge_avail_t, QoS::AtLeastOnce, true, "online").await?;

    // Shared registry of connected devices, keyed by slug.
    let (republish_tx, _) = broadcast::channel::<()>(4);
    let routes: Arc<Mutex<HashMap<String, mpsc::Sender<Command>>>> =
        Arc::new(Mutex::new(HashMap::new()));

    // --- background discovery supervisor: scans forever, auto-connects new --
    tokio::spawn(discovery_loop(
        args.clone(),
        client.clone(),
        routes.clone(),
        republish_tx.clone(),
    ));

    // --- MQTT event loop: route commands, handle HA restarts ----------------
    loop {
        match eventloop.poll().await {
            Ok(Event::Incoming(Incoming::Publish(p))) => {
                let topic = p.topic.clone();
                let payload = String::from_utf8_lossy(&p.payload).to_string();
                if topic == format!("{}/status", args.discovery_prefix) {
                    if payload == "online" {
                        info!("Home Assistant restarted; republishing discovery");
                        let _ = republish_tx.send(());
                    }
                    continue;
                }
                if let Some((slug, cmd)) = parse_command(&args.base_topic, &topic, &payload) {
                    let tx = routes.lock().await.get(&slug).cloned();
                    match tx {
                        Some(tx) => {
                            if tx.send(cmd).await.is_err() {
                                warn!("device task for '{slug}' is gone");
                            }
                        }
                        None => warn!("command for unknown device '{slug}'"),
                    }
                }
            }
            Ok(_) => {}
            Err(e) => {
                warn!("mqtt error: {e}; reconnecting in 5s");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
}

/// Periodically scan for batteries and spawn a task for each newly seen one.
/// Never exits: with no devices in range it simply keeps scanning.
async fn discovery_loop(
    args: Args,
    client: AsyncClient,
    routes: Arc<Mutex<HashMap<String, mpsc::Sender<Command>>>>,
    republish_tx: broadcast::Sender<()>,
) {
    let opts = DiscoverOptions { ble_secs: args.ble_secs, ..Default::default() };
    loop {
        info!("scanning for batteries ({}s BLE scan + serial probe)...", opts.ble_secs);
        match discover(&opts).await {
            Ok(found) => {
                let selected: Vec<_> = if args.devices.is_empty() {
                    found.iter().collect()
                } else {
                    let mut sel = Vec::new();
                    for q in &args.devices {
                        match resolve(&found, q) {
                            Ok(d) => sel.push(d),
                            Err(e) => warn!("device query '{q}': {e}"),
                        }
                    }
                    sel
                };

                for d in selected {
                    let slug = hass::slugify(&d.id);
                    // Already connected/tracked? skip.
                    if routes.lock().await.contains_key(&slug) {
                        continue;
                    }
                    info!("found {} [{}] ({}); connecting", d.label, d.id, d.backend);
                    match d.connect(args.ble_secs).await {
                        Ok(bat) => {
                            info!("connected {} [{}]", d.label, d.id);
                            let (tx, rx) = mpsc::channel::<Command>(16);
                            routes.lock().await.insert(slug.clone(), tx);
                            let routes = routes.clone();
                            let client = client.clone();
                            let args = args.clone();
                            let label = d.label.clone();
                            let republish = republish_tx.subscribe();
                            let task_slug = slug.clone();
                            tokio::spawn(async move {
                                run_device(
                                    bat, task_slug.clone(), label, client, args, rx, republish,
                                )
                                .await;
                                // Task ended (disconnect/fatal): drop from registry so
                                // the next scan can rediscover and reconnect it.
                                routes.lock().await.remove(&task_slug);
                                info!("[{task_slug}] device task ended; will retry on next scan");
                            });
                        }
                        Err(e) => warn!("connect {} [{}] failed: {e}", d.label, d.id),
                    }
                }
            }
            Err(e) => warn!("discovery failed: {e}"),
        }
        tokio::time::sleep(Duration::from_secs(args.rescan_secs.max(1))).await;
    }
}

/// Parse `<base>/<slug>/set/<id>` (ON/OFF) and `<base>/<slug>/setv/<id>`.
fn parse_command(base: &str, topic: &str, payload: &str) -> Option<(String, Command)> {
    let rest = topic.strip_prefix(base)?.strip_prefix('/')?;
    let mut parts = rest.splitn(3, '/');
    let slug = parts.next()?.to_string();
    let kind = parts.next()?;
    let id = parts.next()?.to_string();
    match kind {
        "set" => Some((slug, Command::Toggle { id, on: payload.eq_ignore_ascii_case("ON") })),
        "setv" => Some((slug, Command::Set { id, value: payload.to_string() })),
        _ => None,
    }
}

async fn run_device(
    mut bat: Box<dyn Battery>,
    slug: String,
    label: String,
    client: AsyncClient,
    args: Args,
    mut cmds: mpsc::Receiver<Command>,
    mut republish: broadcast::Receiver<()>,
) {
    let avail_t = format!("{}/{}/availability", args.base_topic, slug);
    let state_t = format!("{}/{}/state", args.base_topic, slug);

    // First snapshot drives entity discovery. Give up after a few tries so the
    // supervisor can rediscover/reconnect instead of us blocking forever.
    let mut first = None;
    for attempt in 1..=3u32 {
        match bat.status().await {
            Ok(s) => {
                first = Some(s);
                break;
            }
            Err(e) => {
                warn!("[{slug}] initial status failed ({attempt}/3): {e}");
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        }
    }
    let Some(first) = first else {
        warn!("[{slug}] could not read initial status; giving up");
        return;
    };

    let configs = hass::discovery_configs(
        &args.discovery_prefix,
        &args.base_topic,
        &slug,
        &label,
        bat.info(),
        bat.capabilities(),
        &first,
    );
    info!("[{slug}] publishing {} entities", configs.len());
    publish_configs(&client, &configs).await;
    publish(&client, &avail_t, "online", true).await;
    publish(&client, &state_t, &hass::flatten(&first).to_string(), true).await;

    let mut online = true;
    let mut failures = 0u32;
    let mut tick = tokio::time::interval(Duration::from_secs(args.interval.max(1)));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await; // consume the immediate first tick

    loop {
        tokio::select! {
            _ = tick.tick() => {
                match bat.status().await {
                    Ok(s) => {
                        failures = 0;
                        if !online {
                            publish(&client, &avail_t, "online", true).await;
                            online = true;
                        }
                        publish(&client, &state_t, &hass::flatten(&s).to_string(), true).await;
                    }
                    Err(e) => {
                        failures += 1;
                        warn!("[{slug}] status failed ({failures}): {e}");
                        if failures >= 3 && online {
                            publish(&client, &avail_t, "offline", true).await;
                            online = false;
                        }
                        // Prolonged failure: assume the device is gone and exit so
                        // the discovery loop can reconnect it when it reappears.
                        if failures >= 10 {
                            warn!("[{slug}] too many failures; disconnecting");
                            publish(&client, &avail_t, "offline", true).await;
                            return;
                        }
                    }
                }
            }
            Some(cmd) = cmds.recv() => {
                info!("[{slug}] execute {cmd:?}");
                match bat.execute(cmd).await {
                    Ok(()) => {
                        // Refresh promptly so HA reflects the change.
                        if let Ok(s) = bat.status().await {
                            publish(&client, &state_t, &hass::flatten(&s).to_string(), true).await;
                        }
                    }
                    Err(e) => warn!("[{slug}] command failed: {e}"),
                }
            }
            Ok(()) = republish.recv() => {
                publish_configs(&client, &configs).await;
                publish(&client, &avail_t, if online { "online" } else { "offline" }, true).await;
            }
        }
    }
}

async fn publish(client: &AsyncClient, topic: &str, payload: &str, retain: bool) {
    if let Err(e) = client.publish(topic, QoS::AtLeastOnce, retain, payload).await {
        warn!("publish {topic} failed: {e}");
    }
}

async fn publish_configs(client: &AsyncClient, configs: &[(String, Value)]) {
    for (topic, cfg) in configs {
        publish(client, topic, &cfg.to_string(), true).await;
    }
}
