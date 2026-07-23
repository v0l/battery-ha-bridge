# Battery Control Bridge

Bridges batteries, BMSes and power stations to Home Assistant via
[MQTT Discovery](https://www.home-assistant.io/integrations/mqtt/#mqtt-discovery).
Entities appear automatically under one HA device per battery.

Supported backends: Anker SOLIX (BLE), JK BMS (serial/BLE), Daly BMS (serial),
Victron (BLE), Pylontech CAN (EG4/SOK/... via SocketCAN).

## Requirements

- The **Mosquitto broker** add-on (or another broker configured in the HA MQTT
  integration). Credentials are wired automatically via the `mqtt` service.
- For BLE devices: a Bluetooth adapter available to the host.
- For serial BMSes: the USB adapter plugged into the HA host.
- For Pylontech CAN: a SocketCAN interface (e.g. `can0`) up on the host.

## Options

| Option | Default | Description |
|--------|---------|-------------|
| `devices` | `[]` | Hardware ids (`ble:...`, `serial:/dev/ttyUSB0`) or label queries to bridge. Empty bridges **everything discovered**. |
| `poll_interval` | `15` | Seconds between status polls. |
| `ble_secs` | `6` | BLE scan duration at startup. |
| `log_level` | `info` | `trace` / `debug` / `info` / `warn` / `error`. |

## Entities

Per device, based on what it reports and its capabilities:

- **Sensors**: SOC, SOH, voltage, current, power in/out, temperature(s),
  capacity, cycles, current limits, cell min/max/delta (per-cell sensors are
  created disabled), alarms, per-port power.
- **Switches**: output ports, charge/discharge MOSFETs, heater/balancer/etc. —
  only when the backend advertises the matching control capability; otherwise
  they appear as binary sensors.
- **Number**: charge limit slider (backends with `SET_CHARGE_LIMIT`).

State is published to `battery_control/<device>/state`; commands are accepted
on `battery_control/<device>/set/<id>` (`ON`/`OFF`) and
`battery_control/<device>/setv/<id>` (value).
