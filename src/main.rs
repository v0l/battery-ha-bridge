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
use std::time::Duration;
use tokio::sync::{broadcast, mpsc};

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
    // --- discover & connect devices --------------------------------------
    let opts = DiscoverOptions { ble_secs: args.ble_secs, ..Default::default() };
    info!("discovering batteries ({}s BLE scan + serial probe)...", opts.ble_secs);
    let found = discover(&opts).await?;
    for d in &found {
        info!("found {} [{}] ({})", d.label, d.id, d.backend);
    }

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
    if selected.is_empty() {
        return Err("no batteries matched/discovered".into());
    }

    let mut devices: Vec<(String, String, Box<dyn Battery>)> = Vec::new();
    for d in selected {
        match d.connect(args.ble_secs).await {
            Ok(b) => {
                info!("connected {} [{}]", d.label, d.id);
                devices.push((hass::slugify(&d.id), d.label.clone(), b));
            }
            Err(e) => warn!("connect {} [{}] failed: {e}", d.label, d.id),
        }
    }
    if devices.is_empty() {
        return Err("no batteries connected".into());
    }

    // --- MQTT ---------------------------------------------------------------
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

    // --- per-device tasks -----------------------------------------------
    let (republish_tx, _) = broadcast::channel::<()>(4);
    let mut routes: HashMap<String, mpsc::Sender<Command>> = HashMap::new();
    for (slug, label, bat) in devices {
        let (tx, rx) = mpsc::channel::<Command>(16);
        routes.insert(slug.clone(), tx);
        tokio::spawn(run_device(
            bat,
            slug,
            label,
            client.clone(),
            args.clone(),
            rx,
            republish_tx.subscribe(),
        ));
    }

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
                    match routes.get(&slug) {
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

    // First snapshot drives entity discovery.
    let first = loop {
        match bat.status().await {
            Ok(s) => break s,
            Err(e) => {
                warn!("[{slug}] initial status failed: {e}; retrying in 10s");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
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
