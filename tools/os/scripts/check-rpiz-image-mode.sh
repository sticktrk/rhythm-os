#!/bin/bash
# Validate the rpiz Buildroot output matches the requested security posture.

set -euo pipefail

usage() {
    echo "Usage: $0 <buildroot-output-dir> <dev|prod>" >&2
}

fail() {
    echo "Error: $*" >&2
    exit 1
}

if [ "$#" -ne 2 ]; then
    usage
    exit 1
fi

OUTPUT_DIR="$1"
IMAGE_MODE="$2"
CONFIG_FILE="$OUTPUT_DIR/.config"
TARGET_DIR="$OUTPUT_DIR/target"
DEFAULTS_FILE="$TARGET_DIR/etc/default/rhythm"
DEV_DEFAULTS_FILE="$TARGET_DIR/etc/default/rhythm-dev"
IMAGE_VERSION_FILE="$TARGET_DIR/etc/rhythm-image-version"
PROD_PAA_TRUST_STORE_PATH="${RHYTHM_PROD_PAA_TRUST_STORE_PATH:-/data/matter/paa-root-certs}"

[ -f "$CONFIG_FILE" ] || fail "missing Buildroot config: $CONFIG_FILE"
[ -d "$TARGET_DIR" ] || fail "missing Buildroot target directory: $TARGET_DIR"

require_line() {
    local file="$1"
    local line="$2"

    grep -Fxq "$line" "$file" || fail "missing '$line' in $file"
}

reject_config_line() {
    local line="$1"

    if grep -Fxq "$line" "$CONFIG_FILE"; then
        fail "production image contains forbidden Buildroot option: $line"
    fi
}

require_line "$CONFIG_FILE" 'BR2_PACKAGE_RHYTHM_CLOUDFLARED=y'
[ -x "$TARGET_DIR/usr/bin/cloudflared" ] || fail "missing executable /usr/bin/cloudflared"
[ -x "$TARGET_DIR/etc/init.d/rhythm-cloudflared" ] || fail "missing executable /etc/init.d/rhythm-cloudflared"
[ -s "$IMAGE_VERSION_FILE" ] || fail "missing $IMAGE_VERSION_FILE"

case "$IMAGE_MODE" in
    dev)
        require_line "$CONFIG_FILE" 'BR2_TARGET_GENERIC_ROOT_PASSWD="rhythm"'
        require_line "$CONFIG_FILE" 'BR2_PACKAGE_DROPBEAR=y'
        [ -f "$DEV_DEFAULTS_FILE" ] || fail "dev image missing $DEV_DEFAULTS_FILE"
        require_line "$DEV_DEFAULTS_FILE" "RHYTHM_DEV_MODE=1"
        require_line "$DEV_DEFAULTS_FILE" "RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1"
        ;;
    prod)
        reject_config_line 'BR2_TARGET_GENERIC_ROOT_PASSWD="rhythm"'
        reject_config_line 'BR2_PACKAGE_DROPBEAR=y'
        [ ! -e "$DEV_DEFAULTS_FILE" ] || fail "production image contains $DEV_DEFAULTS_FILE"
        [ -f "$DEFAULTS_FILE" ] || fail "production image missing $DEFAULTS_FILE"
        require_line "$DEFAULTS_FILE" "RHYTHM_DEV_MODE=0"
        require_line "$DEFAULTS_FILE" "RHYTHM_MATTER_PAA_TRUST_STORE_PATH=$PROD_PAA_TRUST_STORE_PATH"
        require_line "$DEFAULTS_FILE" "RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=0"
        require_line "$DEFAULTS_FILE" "RHYTHM_MATTER_ALLOW_TEST_PAA=0"
        ;;
    *)
        usage
        fail "unknown image mode: $IMAGE_MODE"
        ;;
esac

echo "rpiz image mode validation passed: $IMAGE_MODE"
