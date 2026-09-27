#!/bin/sh
set -eu

TEST_DIR="$(CDPATH= cd "$(dirname "$0")" && pwd)"
INIT_SCRIPT="$TEST_DIR/../buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d/S43bluetooth"
DEFCONFIG="$TEST_DIR/../buildroot/configs/rhythm_rpiz_defconfig"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-bluetooth-migration.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT HUP INT TERM

# Source function definitions without selecting an init action.
set -- sourced-for-test
. "$INIT_SCRIPT"

BLUETOOTH_STATE_SOURCE="$TEST_ROOT/persistent"
BLUETOOTH_STATE_TARGET="$TEST_ROOT/legacy"
BLUETOOTH_MIGRATION_MARKER="$TEST_ROOT/migrated"
LOGFILE="$TEST_ROOT/bluetooth.log"

# With no /dev/serial1 symlink the Bluetooth UART is resolved from the board
# alias through sysfs, whatever number the kernel gave the port.
PL011=/soc/serial@7e201000
MINI=/soc/serial@7e215040
DEV_DIR="$TEST_ROOT/dev"
SYSFS_TTY_DIR="$TEST_ROOT/sys/class/tty"
DT_BASE="$TEST_ROOT/dt"
mkdir -p "$DEV_DIR" "$SYSFS_TTY_DIR" "$DT_BASE$PL011" "$DT_BASE$MINI"

fake_tty() {
    # $1 tty name, $2 device-tree node, $3 levels between tty and UART device
    mkdir -p "$TEST_ROOT/devices/$1$3"
    ln -s "$DT_BASE$2" "$TEST_ROOT/devices/$1/of_node"
    mkdir -p "$SYSFS_TTY_DIR/$1"
    ln -s "$TEST_ROOT/devices/$1$3" "$SYSFS_TTY_DIR/$1/device"
    ln -s /dev/null "$DEV_DIR/$1"
}

# No tty at all (for example the kernel serdev driver owns the port).
if bluetooth_uart_device "$PL011" "$PL011" >/dev/null; then
    echo "resolved a Bluetooth UART that does not exist" >&2
    exit 1
fi

# Legacy names are used only when sysfs cannot answer, and only for the
# matching UART type.
ln -s /dev/null "$DEV_DIR/ttyS0"
if bluetooth_uart_device "$PL011" "$PL011" >/dev/null; then
    echo "attached the PL011 route to the mini UART" >&2
    exit 1
fi
test "$(bluetooth_uart_device "$PL011" "$MINI")" = "$DEV_DIR/ttyS0"
test "$(bluetooth_uart_device '' '')" = "$DEV_DIR/ttyS0"
ln -s /dev/null "$DEV_DIR/ttyAMA0"
test "$(bluetooth_uart_device "$PL011" "$PL011")" = "$DEV_DIR/ttyAMA0"
rm -f "$DEV_DIR/ttyS0" "$DEV_DIR/ttyAMA0"

# Alias-numbered PL011 behind serial-core port devices, mini UART directly.
fake_tty ttyAMA1 "$PL011" /port/port0
fake_tty ttyS0 "$MINI" ""
test "$(resolve_uart_tty "$PL011")" = ttyAMA1
test "$(bluetooth_uart_device "$PL011" "$PL011")" = "$DEV_DIR/ttyAMA1"
test "$(bluetooth_uart_device "$PL011" "$MINI")" = "$DEV_DIR/ttyS0"
if resolve_uart_tty /soc/serial@missing >/dev/null; then
    echo "resolved a UART node that does not exist" >&2
    exit 1
fi

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

# Buildroot only installs deprecated hciconfig when its tools package is also
# selected. The init script's active readiness check depends on that binary.
grep -qx 'BR2_PACKAGE_BLUEZ5_UTILS_TOOLS=y' "$DEFCONFIG"
grep -qx 'BR2_PACKAGE_BLUEZ5_UTILS_DEPRECATED=y' "$DEFCONFIG"

# A controller that appears late must be polled until it can be brought fully
# UP before bluetoothd starts probing it.
FAKE_HCICONFIG="$TEST_ROOT/hciconfig"
FAKE_HCI_STATE="$TEST_ROOT/hciconfig-calls"
export FAKE_HCI_STATE
cat >"$FAKE_HCICONFIG" <<'EOF'
#!/bin/sh
if [ "${2:-}" = "up" ]; then
    exit 0
fi
calls=0
if [ -f "$FAKE_HCI_STATE" ]; then
    calls="$(cat "$FAKE_HCI_STATE")"
fi
calls=$((calls + 1))
printf '%s\n' "$calls" >"$FAKE_HCI_STATE"
if [ "$calls" -lt 3 ]; then
    exit 1
fi
printf '%s\n' 'hci0: Type: Primary  Bus: UART' '        UP RUNNING'
EOF
chmod +x "$FAKE_HCICONFIG"

HCICONFIG="$FAKE_HCICONFIG"
HCI_READY_ATTEMPTS=4
HCI_READY_SLEEP_SECONDS=0
wait_for_hci_ready
test "$(cat "$FAKE_HCI_STATE")" -ge 3

HCICONFIG="$TEST_ROOT/missing-hciconfig"
if wait_for_hci_ready; then
    echo "wait_for_hci_ready unexpectedly accepted a missing hciconfig" >&2
    exit 1
else
    test "$?" -eq 2
fi

# Even if a future image loses the verifier, the board startup must still
# restart bluetoothd instead of leaving all BLE integrations permanently down.
START_EVENTS="$TEST_ROOT/start-events"
stop_bluetoothd() {
    printf '%s\n' stop >>"$START_EVENTS"
}
modprobe() {
    :
}
sleep() {
    :
}
attach_controller() {
    :
}
log_kernel_excerpt() {
    :
}
start_bluetoothd() {
    printf '%s\n' start >>"$START_EVENTS"
}

start
test "$(sed -n '1p' "$START_EVENTS")" = stop
test "$(sed -n '2p' "$START_EVENTS")" = start
