#!/bin/sh
# Home Assistant add-on entrypoint: no bashio, just options.json + Supervisor API.
set -eu

OPTS=/data/options.json

# MQTT credentials from the Mosquitto add-on via the Supervisor services API.
if [ -n "${SUPERVISOR_TOKEN:-}" ]; then
    if MQTT_JSON=$(curl -sf -H "Authorization: Bearer $SUPERVISOR_TOKEN" \
            http://supervisor/services/mqtt); then
        MQTT_HOST=$(echo "$MQTT_JSON" | jq -r '.data.host')
        MQTT_PORT=$(echo "$MQTT_JSON" | jq -r '.data.port')
        MQTT_USERNAME=$(echo "$MQTT_JSON" | jq -r '.data.username')
        MQTT_PASSWORD=$(echo "$MQTT_JSON" | jq -r '.data.password')
        export MQTT_HOST MQTT_PORT MQTT_USERNAME MQTT_PASSWORD
    else
        echo "WARNING: no MQTT service from Supervisor; using defaults/env" >&2
    fi
fi

if [ -f "$OPTS" ]; then
    POLL_INTERVAL=$(jq -r '.poll_interval // 15' "$OPTS")
    BLE_SECS=$(jq -r '.ble_secs // 6' "$OPTS")
    RUST_LOG=$(jq -r '.log_level // "info"' "$OPTS")
    export POLL_INTERVAL BLE_SECS RUST_LOG
    DEVICES=$(jq -r '.devices // [] | join(",")' "$OPTS")
    if [ -n "$DEVICES" ]; then
        export DEVICES
    fi
fi

echo "Starting battery-ha-bridge (mqtt=${MQTT_HOST:-core-mosquitto}:${MQTT_PORT:-1883})"
exec /usr/local/bin/battery-ha-bridge
