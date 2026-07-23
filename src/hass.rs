//! Home Assistant MQTT Discovery payloads and state flattening.
//!
//! The bridge publishes one flattened JSON document per device to
//! `<base>/<slug>/state`; every entity is a discovery config under
//! `<prefix>/<component>/bc_<slug>/<key>/config` with a `value_template`
//! into that document.

use battery_control::{BatteryStatus, Capabilities, DeviceInfo};
use serde_json::{Map, Value, json};

/// Round to 3 decimals to keep MQTT payloads stable/readable.
fn r3(v: f32) -> Value {
    json!(((v as f64) * 1000.0).round() / 1000.0)
}

fn onoff(b: bool) -> Value {
    json!(if b { "ON" } else { "OFF" })
}

/// Sanitize a hardware id (`ble:AA:BB…`, `serial:/dev/ttyUSB0`) into an MQTT/
/// object-id-safe slug.
pub fn slugify(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    let mut prev_us = false;
    for c in id.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
            prev_us = false;
        } else if !prev_us && !out.is_empty() {
            out.push('_');
            prev_us = true;
        }
    }
    out.trim_end_matches('_').to_string()
}

/// Flatten a [`BatteryStatus`] into a single-level JSON object used as the
/// entity state document.
pub fn flatten(s: &BatteryStatus) -> Value {
    let mut m = Map::new();

    macro_rules! num {
        ($k:expr, $v:expr) => {
            if let Some(v) = $v {
                m.insert($k.to_string(), r3(v));
            }
        };
    }

    num!("soc", s.soc);
    num!("soh", s.soh);
    num!("voltage", s.voltage);
    num!("current", s.current);
    num!("power_in", s.power_in);
    num!("power_out", s.power_out);
    num!("time_remaining_h", s.time_remaining_h);
    num!("capacity_remaining_ah", s.capacity_remaining_ah);
    num!("capacity_full_ah", s.capacity_full_ah);
    num!("charge_current_limit_a", s.charge_current_limit_a);
    num!("discharge_current_limit_a", s.discharge_current_limit_a);
    num!("soc_limit_max", s.soc_limit_max);
    num!("soc_limit_min", s.soc_limit_min);
    num!("temperature", s.temperature_c());
    num!("cell_min", s.cell_min());
    num!("cell_max", s.cell_max());
    num!("cell_delta", s.cell_delta());

    if let Some(c) = s.cycles {
        m.insert("cycles".into(), json!(c));
    }
    if let Some(b) = s.charging {
        m.insert("charging".into(), onoff(b));
    }
    if let Some(b) = s.discharging {
        m.insert("discharging".into(), onoff(b));
    }
    for t in &s.temperatures {
        m.insert(format!("temp_{}", slugify(&t.id)), r3(t.celsius));
    }
    for sw in &s.switches {
        m.insert(format!("sw_{}", slugify(&sw.id)), onoff(sw.on));
    }
    for p in &s.ports {
        let key = slugify(&p.id);
        if let Some(on) = p.on {
            m.insert(format!("port_{key}"), onoff(on));
        }
        if let Some(w) = p.watts {
            m.insert(format!("port_{key}_w"), r3(w));
        }
    }
    for c in &s.cells {
        if let Some(v) = c.voltage {
            m.insert(format!("cell_{}", c.index), r3(v));
        }
    }
    m.insert(
        "alarms".into(),
        json!(if s.alarms.is_empty() { "none".to_string() } else { s.alarms.join(", ") }),
    );
    m.insert("alarm_active".into(), onoff(!s.alarms.is_empty()));

    Value::Object(m)
}

/// Build all MQTT-discovery config payloads for one device, based on its
/// capabilities and a first status snapshot (fields absent from the snapshot
/// get no entity).
pub fn discovery_configs(
    prefix: &str,
    base: &str,
    slug: &str,
    label: &str,
    info: &DeviceInfo,
    caps: Capabilities,
    s: &BatteryStatus,
) -> Vec<(String, Value)> {
    let state_t = format!("{base}/{slug}/state");
    let device = json!({
        "identifiers": [format!("bc_{slug}")],
        "name": label,
        "manufacturer": info.backend,
        "model": info.model,
        "serial_number": info.serial,
        "sw_version": info.firmware,
    });
    let availability = json!([
        { "topic": format!("{base}/bridge/availability") },
        { "topic": format!("{base}/{slug}/availability") },
    ]);

    let mut out: Vec<(String, Value)> = Vec::new();
    let mut add = |component: &str, key: &str, mut cfg: Value| {
        let o = cfg.as_object_mut().unwrap();
        o.insert("unique_id".into(), json!(format!("bc_{slug}_{key}")));
        o.insert("object_id".into(), json!(format!("{slug}_{key}")));
        o.insert("state_topic".into(), json!(state_t));
        o.insert("availability".into(), availability.clone());
        o.insert("availability_mode".into(), json!("all"));
        o.insert("device".into(), device.clone());
        out.push((format!("{prefix}/{component}/bc_{slug}/{key}/config"), cfg));
    };

    let sensor = |name: &str,
                  key: &str,
                  device_class: Option<&str>,
                  unit: Option<&str>,
                  state_class: Option<&str>| {
        let mut c = json!({
            "name": name,
            "value_template": format!("{{{{ value_json.{key} }}}}"),
        });
        let o = c.as_object_mut().unwrap();
        if let Some(dc) = device_class {
            o.insert("device_class".into(), json!(dc));
        }
        if let Some(u) = unit {
            o.insert("unit_of_measurement".into(), json!(u));
        }
        if let Some(sc) = state_class {
            o.insert("state_class".into(), json!(sc));
        }
        c
    };
    let binary = |name: &str, key: &str, device_class: Option<&str>| {
        let mut c = json!({
            "name": name,
            "value_template": format!("{{{{ value_json.{key} }}}}"),
            "payload_on": "ON",
            "payload_off": "OFF",
        });
        if let Some(dc) = device_class {
            c.as_object_mut().unwrap().insert("device_class".into(), json!(dc));
        }
        c
    };
    let switch = |name: &str, key: &str, cmd_id: &str| {
        json!({
            "name": name,
            "value_template": format!("{{{{ value_json.{key} }}}}"),
            "command_topic": format!("{base}/{slug}/set/{cmd_id}"),
            "payload_on": "ON",
            "payload_off": "OFF",
            "state_on": "ON",
            "state_off": "OFF",
        })
    };

    // --- basic numeric sensors -------------------------------------------
    let meas = Some("measurement");
    if s.soc.is_some() {
        add("sensor", "soc", sensor("Battery", "soc", Some("battery"), Some("%"), meas));
    }
    if s.soh.is_some() {
        add("sensor", "soh", sensor("State of health", "soh", None, Some("%"), meas));
    }
    if s.voltage.is_some() {
        add("sensor", "voltage", sensor("Voltage", "voltage", Some("voltage"), Some("V"), meas));
    }
    if s.current.is_some() {
        add("sensor", "current", sensor("Current", "current", Some("current"), Some("A"), meas));
    }
    if s.power_in.is_some() {
        add("sensor", "power_in", sensor("Power in", "power_in", Some("power"), Some("W"), meas));
    }
    if s.power_out.is_some() {
        add("sensor", "power_out", sensor("Power out", "power_out", Some("power"), Some("W"), meas));
    }
    if s.time_remaining_h.is_some() {
        add(
            "sensor",
            "time_remaining_h",
            sensor("Time remaining", "time_remaining_h", Some("duration"), Some("h"), meas),
        );
    }
    if s.capacity_remaining_ah.is_some() {
        add(
            "sensor",
            "capacity_remaining_ah",
            sensor("Capacity remaining", "capacity_remaining_ah", None, Some("Ah"), meas),
        );
    }
    if s.capacity_full_ah.is_some() {
        add(
            "sensor",
            "capacity_full_ah",
            sensor("Capacity full", "capacity_full_ah", None, Some("Ah"), meas),
        );
    }
    if s.cycles.is_some() {
        add("sensor", "cycles", sensor("Cycles", "cycles", None, None, Some("total_increasing")));
    }
    if s.charge_current_limit_a.is_some() {
        add(
            "sensor",
            "charge_current_limit_a",
            sensor("Charge current limit", "charge_current_limit_a", Some("current"), Some("A"), meas),
        );
    }
    if s.discharge_current_limit_a.is_some() {
        add(
            "sensor",
            "discharge_current_limit_a",
            sensor("Discharge current limit", "discharge_current_limit_a", Some("current"), Some("A"), meas),
        );
    }

    // --- temperatures ------------------------------------------------------
    if s.temperature_c().is_some() {
        add(
            "sensor",
            "temperature",
            sensor("Temperature", "temperature", Some("temperature"), Some("°C"), meas),
        );
    }
    for t in &s.temperatures {
        let key = format!("temp_{}", slugify(&t.id));
        let name = t.label.clone().unwrap_or_else(|| format!("Temperature {}", t.id));
        let mut c = sensor(&name, &key, Some("temperature"), Some("°C"), meas);
        c.as_object_mut().unwrap().insert("enabled_by_default".into(), json!(false));
        add("sensor", &key.clone(), c);
    }

    // --- cells ---------------------------------------------------------------
    if !s.cells.is_empty() {
        add("sensor", "cell_min", sensor("Cell min", "cell_min", Some("voltage"), Some("V"), meas));
        add("sensor", "cell_max", sensor("Cell max", "cell_max", Some("voltage"), Some("V"), meas));
        add("sensor", "cell_delta", sensor("Cell delta", "cell_delta", Some("voltage"), Some("V"), meas));
        for c in &s.cells {
            let key = format!("cell_{}", c.index);
            let mut cfg =
                sensor(&format!("Cell {}", c.index + 1), &key, Some("voltage"), Some("V"), meas);
            cfg.as_object_mut().unwrap().insert("enabled_by_default".into(), json!(false));
            add("sensor", &key.clone(), cfg);
        }
    }

    // --- alarms -----------------------------------------------------------
    add("binary_sensor", "alarm_active", binary("Alarm", "alarm_active", Some("problem")));
    add("sensor", "alarms", sensor("Alarms", "alarms", None, None, None));

    // --- charge / discharge MOSFETs ----------------------------------------
    if s.charging.is_some() {
        if caps.contains(Capabilities::TOGGLE_CHARGE) {
            add("switch", "charging", switch("Charging", "charging", "charging"));
        } else {
            add(
                "binary_sensor",
                "charging",
                binary("Charging", "charging", Some("battery_charging")),
            );
        }
    }
    if s.discharging.is_some() {
        if caps.contains(Capabilities::TOGGLE_DISCHARGE) {
            add("switch", "discharging", switch("Discharging", "discharging", "discharging"));
        } else {
            add("binary_sensor", "discharging", binary("Discharging", "discharging", None));
        }
    }

    // --- free-form device switches (heater, balancer, precharge, ...) ------
    for sw in &s.switches {
        let key = format!("sw_{}", slugify(&sw.id));
        let name = sw.label.clone().unwrap_or_else(|| sw.id.clone());
        if caps.is_controllable() {
            add("switch", &key.clone(), switch(&name, &key, &sw.id));
        } else {
            add("binary_sensor", &key.clone(), binary(&name, &key, None));
        }
    }

    // --- station ports ------------------------------------------------------
    for p in &s.ports {
        let key = slugify(&p.id);
        let name = p.label.clone().unwrap_or_else(|| p.id.clone());
        if p.on.is_some() {
            if caps.contains(Capabilities::TOGGLE_PORTS) {
                add("switch", &format!("port_{key}"), switch(&name, &format!("port_{key}"), &p.id));
            } else {
                add(
                    "binary_sensor",
                    &format!("port_{key}"),
                    binary(&name, &format!("port_{key}"), Some("power")),
                );
            }
        }
        if p.watts.is_some() {
            add(
                "sensor",
                &format!("port_{key}_w"),
                sensor(&format!("{name} power"), &format!("port_{key}_w"), Some("power"), Some("W"), meas),
            );
        }
    }

    // --- charge limit number ------------------------------------------------
    if caps.contains(Capabilities::SET_CHARGE_LIMIT) {
        add(
            "number",
            "charge_limit",
            json!({
                "name": "Charge limit",
                "command_topic": format!("{base}/{slug}/setv/charge_limit"),
                "value_template": "{{ value_json.soc_limit_max }}",
                "min": 0,
                "max": 100,
                "step": 1,
                "unit_of_measurement": "%",
                "mode": "slider",
                "icon": "mdi:battery-charging-80",
            }),
        );
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs() {
        assert_eq!(slugify("ble:AA:BB:CC"), "ble_aa_bb_cc");
        assert_eq!(slugify("serial:/dev/ttyUSB0"), "serial_dev_ttyusb0");
        assert_eq!(slugify("usb_c1"), "usb_c1");
    }

    #[test]
    fn flatten_basics() {
        let s = BatteryStatus { soc: Some(87.129), charging: Some(true), ..Default::default() };
        let v = flatten(&s);
        assert_eq!(v["soc"], 87.129);
        assert_eq!(v["charging"], "ON");
        assert_eq!(v["alarms"], "none");
        assert_eq!(v["alarm_active"], "OFF");
    }

    #[test]
    fn configs_gate_on_snapshot_and_caps() {
        let s = BatteryStatus { soc: Some(50.0), charging: Some(true), ..Default::default() };
        let cfgs = discovery_configs(
            "homeassistant",
            "battery_control",
            "ble_aa",
            "Test",
            &DeviceInfo::default(),
            Capabilities::READ_BASIC,
            &s,
        );
        // soc sensor exists, voltage doesn't; charging is a binary_sensor (no TOGGLE_CHARGE).
        assert!(cfgs.iter().any(|(t, _)| t.contains("/sensor/bc_ble_aa/soc/")));
        assert!(!cfgs.iter().any(|(t, _)| t.contains("/voltage/")));
        assert!(cfgs.iter().any(|(t, _)| t.contains("/binary_sensor/bc_ble_aa/charging/")));
    }
}
