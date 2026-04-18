#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
TARGET_DIR="$1"
WPA_CONF="${TARGET_DIR}/etc/wpa_supplicant.conf"
WIFI_SSID="${RHYTHM_WIFI_SSID:-}"
WIFI_PSK="${RHYTHM_WIFI_PSK:-}"
WIFI_COUNTRY="${RHYTHM_WIFI_COUNTRY:-US}"
WIFI_COUNTRY="$(printf '%s' "$WIFI_COUNTRY" | tr '[:lower:]' '[:upper:]')"
BOARD_FIRMWARE_DIR="${SCRIPT_DIR}/firmware"
FIRMWARE_ROOT_DIR="${TARGET_DIR}/lib/firmware"
FIRMWARE_DIR="${TARGET_DIR}/lib/firmware/brcm"
CYPRESS_FIRMWARE_DIR="${TARGET_DIR}/lib/firmware/cypress"
RHYTHM_DEFAULTS_DIR="${TARGET_DIR}/etc/default"
RHYTHM_DEV_DEFAULTS="${RHYTHM_DEFAULTS_DIR}/rhythm-dev"

is_truthy() {
    case "${1:-}" in
        1|true|TRUE|yes|YES|on|ON) return 0 ;;
        *) return 1 ;;
    esac
}

escape_wpa_string() {
    printf '%s' "$1" | sed 's/[\\"]/\\&/g'
}

mkdir -p "$(dirname "$WPA_CONF")"
mkdir -p "$FIRMWARE_ROOT_DIR"
mkdir -p "$FIRMWARE_DIR"

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

copy_if_present() {
    src="$1"
    dst="$2"

    if [ -f "$src" ] && [ ! -e "$dst" ]; then
        cp -f "$src" "$dst"
    fi
}

copy_real_if_present() {
    src="$1"
    dst="$2"

    if [ -e "$src" ]; then
        rm -f "$dst"
        cp -L -f "$src" "$dst"
    fi
}

copy_real_if_missing() {
    src="$1"
    dst="$2"

    if [ -e "$src" ] && [ ! -e "$dst" ]; then
        cp -L -f "$src" "$dst"
    fi
}

# Prefer the Raspberry Pi-specific brcmfmac blobs if they were already staged
# by brcmfmac_sdio-firmware-rpi. Only fall back to the Cypress/linux-firmware
# names when the generic 43430 blob names are absent, then normalize the final
# filenames to the ones the Pi Zero W kernel requests at runtime.
copy_real_if_missing \
    "${CYPRESS_FIRMWARE_DIR}/cyfmac43430-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.bin"
copy_real_if_missing \
    "${CYPRESS_FIRMWARE_DIR}/cyfmac43430-sdio.clm_blob" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.clm_blob"
copy_real_if_missing \
    "${FIRMWARE_DIR}/brcmfmac43430a0-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.bin"
copy_real_if_missing \
    "${FIRMWARE_DIR}/brcmfmac43430a0-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.bin"

copy_real_if_present \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.bin"
copy_real_if_present \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.clm_blob" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.clm_blob"
copy_real_if_present \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.txt" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.txt"
copy_real_if_present \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,3-model-b.txt" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.txt"
# Prefer a known-good Zero W board file over the Pi 3 Model B fallback. The
# wrong board NVRAM file can leave the SDIO chip present but unable to switch
# into HT clock mode, which shows up as brcmf_sdio_htclk timeouts.
copy_real_if_present \
    "${BOARD_FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.txt" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.txt"
copy_real_if_present \
    "${BOARD_FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.txt" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.raspberrypi,model-zero-w.txt"

# The Pi Zero W Bluetooth controller expects a Broadcom patch file at
# /lib/firmware/BCM43430A1.hcd or /lib/firmware/brcm/BCM43430A1.hcd depending
# on which attach path is in use. Normalize whatever variant the firmware
# packages provided to both canonical filenames.
for src in \
    "${FIRMWARE_DIR}"/BCM43430A1*.hcd \
    "${FIRMWARE_DIR}"/BCM4343*.hcd \
    "${TARGET_DIR}"/lib/firmware/synaptics/*.hcd
do
    [ -f "$src" ] || continue
    copy_if_present "$src" "${FIRMWARE_DIR}/BCM43430A1.hcd"
    copy_if_present "$src" "${FIRMWARE_ROOT_DIR}/BCM43430A1.hcd"
    if [ -f "${FIRMWARE_DIR}/BCM43430A1.hcd" ] || [ -f "${FIRMWARE_ROOT_DIR}/BCM43430A1.hcd" ]; then
        break
    fi
done

# rhythm-chipd is linked against the x-tools crosstool-NG musl toolchain that
# built libCHIP.a, whose binaries request "/lib/ld-musl-armhf.so.1". Buildroot
# ships the same ARMv6/v7 hard-float loader as "/lib/ld-musl-arm.so.1", so add
# the armhf alias as a symlink to the actual loader.
if [ -e "${TARGET_DIR}/lib/ld-musl-arm.so.1" ] \
    && [ ! -e "${TARGET_DIR}/lib/ld-musl-armhf.so.1" ]; then
    ln -s ld-musl-arm.so.1 "${TARGET_DIR}/lib/ld-musl-armhf.so.1"
fi

mkdir -p "$RHYTHM_DEFAULTS_DIR"
rm -f "$RHYTHM_DEV_DEFAULTS"
if is_truthy "${RHYTHM_DEV_MODE:-}"; then
    cat > "$RHYTHM_DEV_DEFAULTS" <<'EOF'
RHYTHM_DEV_MODE=1
RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1
EOF
fi
