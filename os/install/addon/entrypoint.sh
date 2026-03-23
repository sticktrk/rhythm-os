#!/bin/sh
set -e

# Detect mode and set up environment
if [ -z "${SUPERVISOR_TOKEN:-}" ] && [ -n "${SUPERVISOR_TOKEN_FILE:-}" ] && [ -f "${SUPERVISOR_TOKEN_FILE}" ]; then
    SUPERVISOR_TOKEN="$(cat "${SUPERVISOR_TOKEN_FILE}")"
fi

if [ -n "${SUPERVISOR_TOKEN:-}" ]; then
    echo "[rhythm] Addon mode (SUPERVISOR_TOKEN present)"
    export HA_WEBSOCKET_URL="ws://supervisor/core/api/websocket"
    export HA_TOKEN="${SUPERVISOR_TOKEN}"
    export HA_HOST="supervisor"
    export HA_PORT="80"
    export HA_USE_SSL="false"
else
    echo "[rhythm] Standalone mode"
    export HA_HOST="${HA_HOST:-localhost}"
    export HA_PORT="${HA_PORT:-8123}"
    export HA_USE_SSL="${HA_USE_SSL:-false}"
fi

if [ -z "${HA_TOKEN:-}" ]; then
    echo "[rhythm] ERROR: No Home Assistant token provided. Set HA_TOKEN."
    exit 1
fi

export PORT="${PORT:-39821}"
echo "[rhythm] Starting Rhythm OS — HA=${HA_HOST}:${HA_PORT} port=${PORT}"

exec /usr/local/bin/rhythm-addon
