#!/usr/bin/with-contenv bashio
set -e

# MQTT credentials from the Mosquitto add-on service.
if bashio::services.available mqtt; then
    export MQTT_HOST="$(bashio::services mqtt 'host')"
    export MQTT_PORT="$(bashio::services mqtt 'port')"
    export MQTT_USERNAME="$(bashio::services mqtt 'username')"
    export MQTT_PASSWORD="$(bashio::services mqtt 'password')"
else
    bashio::log.warning "No MQTT service found; falling back to defaults/env"
fi

export POLL_INTERVAL="$(bashio::config 'poll_interval')"
export BLE_SECS="$(bashio::config 'ble_secs')"
export RUST_LOG="$(bashio::config 'log_level')"

# Comma-join the devices list (empty = bridge everything discovered).
DEVICES="$(bashio::config 'devices' | paste -sd, -)"
if [ -n "$DEVICES" ] && [ "$DEVICES" != "null" ]; then
    export DEVICES
fi

bashio::log.info "Starting battery-ha-bridge (mqtt=${MQTT_HOST}:${MQTT_PORT})"
exec /usr/local/bin/battery-ha-bridge
