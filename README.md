# battery-ha-bridge

Expose batteries, BMSes and power stations to **Home Assistant** via
[MQTT Discovery], built on [battery-control].

Anker SOLIX (BLE) · JK BMS (serial/BLE) · Daly BMS (serial) · Victron (BLE) ·
Pylontech CAN (EG4/SOK/… via SocketCAN)

```
battery-control ──> battery-ha-bridge ──> MQTT broker ──> Home Assistant
```

Per device (based on what it reports and its `Capabilities`):

- **Sensors** — SOC, SOH, voltage, current, power in/out, temperatures
  (per-probe, disabled by default), capacity, cycles, current limits,
  cell min/max/delta (+ per-cell, disabled by default), alarms, per-port power.
- **Switches** — output ports, charge/discharge MOSFETs, heater/balancer/… —
  only when the backend can control them; otherwise binary sensors.
- **Number** — charge-limit slider (`SET_CHARGE_LIMIT` backends).

Entities appear automatically under one HA device per battery. Discovery is
republished when HA restarts; availability is tracked per device and for the
bridge (MQTT LWT).

## Home Assistant OS add-on

This repo doubles as an add-on repository:

1. **Settings → Add-ons → Add-on store → ⋮ → Repositories** → add
   `https://github.com/v0l/battery-ha-bridge`
2. Install **Battery Control Bridge**. MQTT credentials (Mosquitto add-on),
   BLE (D-Bus), serial (UART) and CAN access are wired up automatically.

See [`battery_bridge/DOCS.md`](battery_bridge/DOCS.md) for options.

## Standalone

Requires [battery-control] checked out as a sibling directory:

```sh
git clone https://github.com/v0l/battery-control
git clone https://github.com/v0l/battery-ha-bridge
cd battery-ha-bridge
cargo run --release -- --mqtt-host 192.168.1.10 --device c1000
```

Empty `--device` bridges everything discovered. On Linux, add
`--features can` for Pylontech over SocketCAN. All flags are also settable via
env (`MQTT_HOST`, `DEVICES`, `POLL_INTERVAL`, …) — see `--help`.

## Topics

| Topic | Direction | Payload |
|-------|-----------|---------|
| `battery_control/<dev>/state` | publish | flattened JSON status |
| `battery_control/<dev>/availability` | publish | `online` / `offline` |
| `battery_control/bridge/availability` | publish (LWT) | `online` / `offline` |
| `battery_control/<dev>/set/<id>` | subscribe | `ON` / `OFF` → `Command::Toggle` |
| `battery_control/<dev>/setv/<id>` | subscribe | value → `Command::Set` |
| `homeassistant/<component>/bc_<dev>/<key>/config` | publish | discovery config |

[MQTT Discovery]: https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery
[battery-control]: https://github.com/v0l/battery-control

## License

MIT
