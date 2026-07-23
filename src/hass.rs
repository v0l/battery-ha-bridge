//! Home Assistant MQTT Discovery payloads and state flattening.
//!
//! The bridge publishes one flattened JSON document per device to
//! `<base>/<slug>/state`; every entity is a discovery config under
//! `<prefix>/<component>/bc_<slug>/<key>/config` with a `value_template`
//! into that document.
//!
//! Built against `battery_control`'s id-addressed model: sensors, switches,
//! ports, cells and settings are free-form collections keyed by id.

use battery_control::{
    BatteryStatus, Capabilities, DeviceInfo, SettingKind, SettingValue, Unit,
};
use serde_json::{Map, Value, json};

/// Round to 3 decimals to keep MQTT payloads stable/readable.
fn r3(v: f64) -> Value {
    json!((v * 1000.0).round() / 1000.0)
}

fn onoff(b: bool) -> Value {
    json!(if b { "ON" } else { "OFF" })
}

/// Sanitize a hardware id (`ble:AA:BB…`, `serial:/dev/ttyUSB0`) or free-form
/// element id (`temp.t1`) into an MQTT/object-id-safe slug.
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

/// The two MOSFET switches get top-level state keys and dedicated entities;
/// all other switches are prefixed `sw_`.
fn switch_key(id: &str) -> String {
    match id {
        "charging" | "discharging" => id.to_string(),
        other => format!("sw_{}", slugify(other)),
    }
}

/// Flatten a [`BatteryStatus`] into a single-level JSON object used as the
/// entity state document.
pub fn flatten(s: &BatteryStatus) -> Value {
    let mut m = Map::new();

    // Every scalar sensor by slug ("soc", "voltage", "temp_t1", ...).
    for sen in &s.sensors {
        m.insert(slugify(&sen.id), r3(sen.value));
    }
    // Aggregate "temperature": first Celsius probe, for a stable primary entity.
    if let Some(t) = s.sensors.iter().find(|x| x.unit == Unit::Celsius) {
        m.insert("temperature".into(), r3(t.value));
    }

    for sw in &s.switches {
        m.insert(switch_key(&sw.id), onoff(sw.on));
    }

    for p in &s.ports {
        let key = slugify(&p.id);
        if let Some(on) = p.on {
            m.insert(format!("port_{key}"), onoff(on));
        }
        if let Some(w) = p.watts {
            m.insert(format!("port_{key}_w"), r3(w as f64));
        }
    }

    for c in &s.cells {
        if let Some(v) = c.voltage {
            m.insert(format!("cell_{}", c.index), r3(v as f64));
        }
    }
    if let Some(v) = s.cell_min() {
        m.insert("cell_min".into(), r3(v as f64));
    }
    if let Some(v) = s.cell_max() {
        m.insert("cell_max".into(), r3(v as f64));
    }
    if let Some(v) = s.cell_delta() {
        m.insert("cell_delta".into(), r3(v as f64));
    }

    for st in &s.settings {
        let key = format!("set_{}", slugify(&st.id));
        let v = match &st.value {
            SettingValue::Bool(b) => onoff(*b),
            SettingValue::Number(n) => r3(*n),
            SettingValue::Text(t) => json!(t),
        };
        m.insert(key, v);
    }

    m.insert(
        "alarms".into(),
        json!(if s.alarms.is_empty() { "none".to_string() } else { s.alarms.join(", ") }),
    );
    m.insert("alarm_active".into(), onoff(!s.alarms.is_empty()));

    Value::Object(m)
}

/// Home Assistant metadata for the standard readings.
/// `(id, name, device_class, unit, state_class)`
const READINGS: &[(&str, &str, Option<&str>, Option<&str>, Option<&str>)] = &[
    ("soc", "Battery", Some("battery"), Some("%"), Some("measurement")),
    ("soh", "State of health", None, Some("%"), Some("measurement")),
    ("voltage", "Voltage", Some("voltage"), Some("V"), Some("measurement")),
    ("current", "Current", Some("current"), Some("A"), Some("measurement")),
    ("power_in", "Power in", Some("power"), Some("W"), Some("measurement")),
    ("power_out", "Power out", Some("power"), Some("W"), Some("measurement")),
    ("time_remaining_h", "Time remaining", Some("duration"), Some("h"), Some("measurement")),
    ("capacity_remaining_ah", "Capacity remaining", None, Some("Ah"), Some("measurement")),
    ("capacity_full_ah", "Capacity full", None, Some("Ah"), Some("measurement")),
    ("cycles", "Cycles", None, None, Some("total_increasing")),
    ("charge_current_limit_a", "Charge current limit", Some("current"), Some("A"), Some("measurement")),
    ("discharge_current_limit_a", "Discharge current limit", Some("current"), Some("A"), Some("measurement")),
    ("soc_limit_max", "SOC limit max", None, Some("%"), Some("measurement")),
    ("soc_limit_min", "SOC limit min", None, Some("%"), Some("measurement")),
];

/// Build all MQTT-discovery config payloads for one device, based on its
/// capabilities and a first status snapshot (elements absent from the snapshot
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

    // --- standard numeric sensors ------------------------------------------
    for (id, name, dc, unit, sc) in READINGS {
        if s.reading(id).is_some() {
            add("sensor", id, sensor(name, id, *dc, *unit, *sc));
        }
    }

    // --- temperatures ------------------------------------------------------
    let meas = Some("measurement");
    if s.sensors.iter().any(|x| x.unit == Unit::Celsius) {
        add(
            "sensor",
            "temperature",
            sensor("Temperature", "temperature", Some("temperature"), Some("°C"), meas),
        );
    }
    for t in s.sensors.iter().filter(|x| x.id.starts_with("temp.")) {
        let key = slugify(&t.id);
        let name = t.label.clone().unwrap_or_else(|| key.clone());
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

    // --- switches -----------------------------------------------------------
    for sw in &s.switches {
        let key = switch_key(&sw.id);
        let name = sw.label.clone().unwrap_or_else(|| sw.id.clone());
        let togglable = match sw.id.as_str() {
            "charging" => caps.contains(Capabilities::TOGGLE_CHARGE),
            "discharging" => caps.contains(Capabilities::TOGGLE_DISCHARGE),
            "balancer" => caps.contains(Capabilities::TOGGLE_BALANCER),
            _ => caps.is_controllable(),
        };
        let dc = match sw.id.as_str() {
            "charging" => Some("battery_charging"),
            _ => None,
        };
        if togglable {
            add("switch", &key.clone(), switch(&name, &key, &sw.id));
        } else {
            add("binary_sensor", &key.clone(), binary(&name, &key, dc));
        }
    }

    // --- station ports (controllability is per-port) -------------------------
    for p in &s.ports {
        let key = slugify(&p.id);
        let name = p.label.clone().unwrap_or_else(|| p.id.clone());
        if p.on.is_some() {
            if p.settable {
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

    // --- device settings (config category, disabled by default) -------------
    let can_write = caps.contains(Capabilities::WRITE_SETTINGS);
    for st in &s.settings {
        let key = format!("set_{}", slugify(&st.id));
        let name = st.label.clone().unwrap_or_else(|| st.id.clone());
        let mut cfg = match (&st.kind, st.writable && can_write) {
            (SettingKind::Bool, true) => ("switch", switch(&name, &key, &st.id)),
            (SettingKind::Bool, false) => ("binary_sensor", binary(&name, &key, None)),
            (SettingKind::Number { min, max, step, unit }, true) => (
                "number",
                json!({
                    "name": name,
                    "command_topic": format!("{base}/{slug}/setv/{}", st.id),
                    "value_template": format!("{{{{ value_json.{key} }}}}"),
                    "min": min.unwrap_or(0.0),
                    "max": max.unwrap_or(10_000.0),
                    "step": step.unwrap_or(0.001),
                    "unit_of_measurement": unit.map(|u| u.symbol()).unwrap_or(""),
                    "mode": "box",
                }),
            ),
            (SettingKind::Number { unit, .. }, false) => (
                "sensor",
                sensor(&name, &key, None, unit.map(|u| u.symbol()), None),
            ),
            // Enum/Text: read-only representation for now.
            (_, _) => ("sensor", sensor(&name, &key, None, None, None)),
        };
        let (component, payload) = (&mut cfg.0, &mut cfg.1);
        let o = payload.as_object_mut().unwrap();
        o.insert("entity_category".into(), json!("config"));
        o.insert("enabled_by_default".into(), json!(false));
        add(component, &key.clone(), payload.clone());
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
    use battery_control::{Reading, SwitchId};

    fn status(soc: Option<f64>, charging: Option<bool>) -> BatteryStatus {
        let mut s = BatteryStatus::default();
        s.set(Reading::Soc, soc);
        s.set_switch(SwitchId::Charging, charging);
        s
    }

    #[test]
    fn slugs() {
        assert_eq!(slugify("ble:AA:BB:CC"), "ble_aa_bb_cc");
        assert_eq!(slugify("serial:/dev/ttyUSB0"), "serial_dev_ttyusb0");
        assert_eq!(slugify("usb_c1"), "usb_c1");
        assert_eq!(slugify("temp.t1"), "temp_t1");
    }

    #[test]
    fn flatten_basics() {
        let v = flatten(&status(Some(87.129), Some(true)));
        assert_eq!(v["soc"], 87.129);
        assert_eq!(v["charging"], "ON");
        assert_eq!(v["alarms"], "none");
        assert_eq!(v["alarm_active"], "OFF");
    }

    #[test]
    fn configs_gate_on_snapshot_and_caps() {
        let cfgs = discovery_configs(
            "homeassistant",
            "battery_control",
            "ble_aa",
            "Test",
            &DeviceInfo::default(),
            Capabilities::READ_BASIC,
            &status(Some(50.0), Some(true)),
        );
        // soc sensor exists, voltage doesn't; charging is a binary_sensor (no TOGGLE_CHARGE).
        assert!(cfgs.iter().any(|(t, _)| t.contains("/sensor/bc_ble_aa/soc/")));
        assert!(!cfgs.iter().any(|(t, _)| t.contains("/voltage/")));
        assert!(cfgs.iter().any(|(t, _)| t.contains("/binary_sensor/bc_ble_aa/charging/")));
    }

    #[test]
    fn settings_become_config_entities() {
        use battery_control::{Setting, SettingKind, SettingValue, Unit};
        let mut s = status(Some(50.0), None);
        s.settings.push(Setting {
            id: "cell_ovp".into(),
            label: Some("Cell OVP".into()),
            value: SettingValue::Number(3.55),
            kind: SettingKind::Number { min: None, max: None, step: None, unit: Some(Unit::Volt) },
            writable: true,
        });
        let v = flatten(&s);
        assert_eq!(v["set_cell_ovp"], 3.55);

        let cfgs = discovery_configs(
            "homeassistant",
            "battery_control",
            "ble_aa",
            "Test",
            &DeviceInfo::default(),
            Capabilities::WRITE_SETTINGS,
            &s,
        );
        let (topic, cfg) = cfgs
            .iter()
            .find(|(t, _)| t.contains("/set_cell_ovp/"))
            .expect("setting entity");
        assert!(topic.starts_with("homeassistant/number/"));
        assert_eq!(cfg["command_topic"], "battery_control/ble_aa/setv/cell_ovp");
        assert_eq!(cfg["entity_category"], "config");
    }
}
