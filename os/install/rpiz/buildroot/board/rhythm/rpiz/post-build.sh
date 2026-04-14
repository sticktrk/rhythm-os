#!/bin/sh

set -eu

TARGET_DIR="$1"
WPA_CONF="${TARGET_DIR}/etc/wpa_supplicant.conf"
WIFI_SSID="${RHYTHM_WIFI_SSID:-}"
WIFI_PSK="${RHYTHM_WIFI_PSK:-}"
WIFI_COUNTRY="${RHYTHM_WIFI_COUNTRY:-US}"
WIFI_COUNTRY="$(printf '%s' "$WIFI_COUNTRY" | tr '[:lower:]' '[:upper:]')"

escape_wpa_string() {
    printf '%s' "$1" | sed 's/[\\"]/\\&/g'
}

mkdir -p "$(dirname "$WPA_CONF")"

{
    printf 'ctrl_interface=/var/run/wpa_supplicant\n'
    printf 'update_config=0\n'

    if [ -n "$WIFI_SSID" ] && [ -n "$WIFI_PSK" ]; then
        printf 'country=%s\n' "$WIFI_COUNTRY"
        printf '\nnetwork={\n'
        printf '  ssid="%s"\n' "$(escape_wpa_string "$WIFI_SSID")"
        printf '  psk="%s"\n' "$(escape_wpa_string "$WIFI_PSK")"
        printf '}\n'
    else
        printf '\n# Wi-Fi credentials were not embedded into this image.\n'
        printf '# Rebuild with --wifi-ssid and --wifi-psk to enable auto-join.\n'
    fi
} > "$WPA_CONF"

chmod 600 "$WPA_CONF"
