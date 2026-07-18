#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
PROJECT_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../../.." && pwd)
WATCHDOG="$PROJECT_ROOT/os/install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/usr/bin/rhythm-hardware-watchdog"
INITTAB="$PROJECT_ROOT/os/install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/etc/inittab"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/rhythm-watchdog-test.XXXXXX")
SINK="$TMP_DIR/watchdog"
PID=""

cleanup() {
    if [ -n "$PID" ] && kill -0 "$PID" 2>/dev/null; then
        kill -TERM "$PID" 2>/dev/null || true
        wait "$PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT HUP INT TERM

test -x "$WATCHDOG"
sh -n "$WATCHDOG"
grep -Fq '::respawn:/usr/bin/rhythm-hardware-watchdog' "$INITTAB"

: > "$SINK"
RHYTHM_HARDWARE_WATCHDOG_DEVICE="$SINK" \
RHYTHM_HARDWARE_WATCHDOG_INTERVAL_SECS=0.05 \
RHYTHM_HARDWARE_WATCHDOG_ALLOW_REGULAR=1 \
    "$WATCHDOG" &
PID=$!

attempt=0
while [ ! -s "$SINK" ] && [ "$attempt" -lt 100 ]; do
    sleep 0.01
    attempt=$((attempt + 1))
done

if [ ! -s "$SINK" ]; then
    echo "watchdog did not emit a keepalive" >&2
    exit 1
fi

kill -TERM "$PID"
wait "$PID"
PID=""

last_byte=$(tail -c 1 "$SINK")
if [ "$last_byte" != "V" ]; then
    echo "watchdog did not disarm with magic close" >&2
    exit 1
fi

echo "rpiz-hardware-watchdog-sim: passed"
