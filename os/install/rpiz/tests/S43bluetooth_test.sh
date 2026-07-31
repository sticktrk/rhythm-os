#!/bin/sh
set -eu

TEST_DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"
INIT_SCRIPT="$TEST_DIR/../buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d/S43bluetooth"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-bluetooth-migration.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

# Source function definitions without selecting an init action.
set -- sourced-for-test
. "$INIT_SCRIPT"

BLUETOOTH_STATE_SOURCE="$TEST_ROOT/persistent"
BLUETOOTH_STATE_TARGET="$TEST_ROOT/legacy"
BLUETOOTH_MIGRATION_MARKER="$TEST_ROOT/migrated"
LOGFILE="$TEST_ROOT/bluetooth.log"

mkdir -p \
    "$BLUETOOTH_STATE_SOURCE/adapter/device-one" \
    "$BLUETOOTH_STATE_TARGET/adapter/device-one" \
    "$BLUETOOTH_STATE_TARGET/adapter/device-two"
printf '%s\n' "partial" >"$BLUETOOTH_STATE_SOURCE/adapter/device-one/info"
printf '%s\n' "device-one-complete" >"$BLUETOOTH_STATE_TARGET/adapter/device-one/info"
printf '%s\n' "device-two-complete" >"$BLUETOOTH_STATE_TARGET/adapter/device-two/info"

migrate_legacy_bluetooth_state

cmp \
    "$BLUETOOTH_STATE_TARGET/adapter/device-one/info" \
    "$BLUETOOTH_STATE_SOURCE/adapter/device-one/info"
cmp \
    "$BLUETOOTH_STATE_TARGET/adapter/device-two/info" \
    "$BLUETOOTH_STATE_SOURCE/adapter/device-two/info"
test -e "$BLUETOOTH_MIGRATION_MARKER"

# Once the durable marker exists, stale rootfs state must never be imported
# again (factory reset intentionally leaves the marker in place).
printf '%s\n' "stale-rootfs-value" >"$BLUETOOTH_STATE_TARGET/adapter/device-one/info"
migrate_legacy_bluetooth_state
grep -qx "device-one-complete" "$BLUETOOTH_STATE_SOURCE/adapter/device-one/info"
