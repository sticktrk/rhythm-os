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

# Buildroot's Raspberry Pi Wi-Fi package only provided the NVRAM text files.
# Use linux-firmware for the actual 43430 blobs, then normalize the filenames
# to the ones the Pi Zero W kernel requests at runtime.
copy_real_if_present \
    "${CYPRESS_FIRMWARE_DIR}/cyfmac43430-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.bin"
copy_real_if_present \
    "${CYPRESS_FIRMWARE_DIR}/cyfmac43430-sdio.clm_blob" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.clm_blob"
copy_real_if_present \
    "${FIRMWARE_DIR}/brcmfmac43430a0-sdio.bin" \
    "${FIRMWARE_DIR}/brcmfmac43430-sdio.bin"
copy_real_if_present \
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
