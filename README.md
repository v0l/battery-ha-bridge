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

## Home Assistant Container / Core (docker)

No Supervisor, no add-ons — run the bridge as a sibling container pointing at
the same MQTT broker HA uses. See
[`docker-compose.example.yml`](docker-compose.example.yml):

```sh
docker run -d --name battery-bridge --restart unless-stopped \
  --network host \
  -v /var/run/dbus:/var/run/dbus:ro \
  -e MQTT_HOST=127.0.0.1 -e MQTT_USERNAME=mqtt -e MQTT_PASSWORD=... \
  ghcr.io/v0l/battery-bridge:latest
```

Add `--device /dev/ttyUSB0` for serial BMSes. The MQTT integration must be
configured in HA (Settings → Devices & services → Add integration → MQTT);
entities then appear automatically.

## Standalone

[battery-control] is pulled automatically as a cargo git dependency:

```sh
git clone https://github.com/v0l/battery-ha-bridge
cd battery-ha-bridge
cargo run --release -- --mqtt-host 192.168.1.10 --device c1000
```

To pick up new upstream lib changes, run `cargo update -p battery_control`.

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
