#!/bin/sh
set -e

# Read simple string options from Home Assistant's /data/options.json when present.
read_json_option() {
    option_name="$1"
    options_path="${2:-/data/options.json}"

    if [ ! -f "${options_path}" ]; then
        return 0
    fi

    tr -d '\n' < "${options_path}" | sed -n "s/.*\"${option_name}\"[[:space:]]*:[[:space:]]*\"\\([^\"]*\\)\".*/\\1/p"
}

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

requested_log_level="${LOG_LEVEL:-$(read_json_option log_level)}"
case "${requested_log_level:-info}" in
    trace|debug|info|warn|error)
        export LOG_LEVEL="${requested_log_level:-info}"
        ;;
    warning)
        export LOG_LEVEL="warn"
        echo "[rhythm] Translating log level 'warning' to 'warn'"
        ;;
    notice)
        export LOG_LEVEL="info"
        echo "[rhythm] Translating log level 'notice' to 'info'"
        ;;
    fatal)
        export LOG_LEVEL="error"
        echo "[rhythm] Translating log level 'fatal' to 'error'"
        ;;
    *)
        export LOG_LEVEL="info"
        echo "[rhythm] Invalid log level '${requested_log_level}', defaulting to info"
        ;;
esac

export PORT="${PORT:-54448}"
echo "[rhythm] Starting Rhythm OS — HA=${HA_HOST}:${HA_PORT} port=${PORT} log=${LOG_LEVEL}"

exec /usr/local/bin/rhythm-addon
