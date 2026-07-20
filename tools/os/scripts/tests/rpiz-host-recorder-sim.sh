#!/bin/sh

set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../../.." && pwd)
INITTAB="$REPO_ROOT/os/install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/etc/inittab"
BOOT_SCRIPT="$REPO_ROOT/os/install/rpiz/buildroot/board/rhythm/rpiz/rootfs-overlay/etc/init.d/S42hostrecorder"
RECORDER="$REPO_ROOT/target/debug/rhythm-host-recorder"
TMP_DIR=$(mktemp -d "${TMPDIR:-/tmp}/rhythm-host-recorder-test.XXXXXX")
RECORDER_PID=""
SERVER_PID=""

cleanup() {
    if [ -n "$SERVER_PID" ] && kill -0 "$SERVER_PID" 2>/dev/null; then
        kill "$SERVER_PID" 2>/dev/null || true
    fi
    if [ -n "$RECORDER_PID" ] && kill -0 "$RECORDER_PID" 2>/dev/null; then
        kill -TERM "$RECORDER_PID" 2>/dev/null || true
        wait "$RECORDER_PID" 2>/dev/null || true
    fi
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT HUP INT TERM

test -x "$BOOT_SCRIPT"
sh -n "$BOOT_SCRIPT"

rcs_line=$(grep -nF '::sysinit:/etc/init.d/rcS' "$INITTAB" | cut -d: -f1)
recorder_line=$(grep -nF '::respawn:/usr/bin/rhythm-host-recorder run --data-dir /data' "$INITTAB" | cut -d: -f1)
watchdog_line=$(grep -nF '::respawn:/usr/bin/rhythm-hardware-watchdog' "$INITTAB" | cut -d: -f1)
server_line=$(grep -nF '::respawn:/usr/bin/rhythm-launch' "$INITTAB" | cut -d: -f1)
test "$rcs_line" -lt "$recorder_line"
test "$recorder_line" -lt "$watchdog_line"
test "$watchdog_line" -lt "$server_line"
grep -Fq 'boot-capture --data-dir' "$BOOT_SCRIPT"

if [ ! -x "$RECORDER" ]; then
    (cd "$REPO_ROOT" && cargo build -p rhythm-host-recorder >/dev/null)
fi

DATA_DIR="$TMP_DIR/data"
RUN_DIR="$TMP_DIR/run"
mkdir -p "$DATA_DIR" "$RUN_DIR"

"$RECORDER" run \
    --data-dir "$DATA_DIR" \
    --run-root "$RUN_DIR" \
    --summary-secs 1 \
    --detail-secs 60 \
    --sync-secs 1 \
    >/dev/null 2>&1 &
RECORDER_PID=$!

sleep 30 &
SERVER_PID=$!
sleep 1
kill "$SERVER_PID"
wait "$SERVER_PID" 2>/dev/null || true
SERVER_PID=""

before=$(find "$DATA_DIR/boot-diagnostics/host-flight-recorder/current" -name 'segment-*.ndjson' -type f -exec wc -l {} + | awk '{sum += $1} END {print sum + 0}')
sleep 2
after=$(find "$DATA_DIR/boot-diagnostics/host-flight-recorder/current" -name 'segment-*.ndjson' -type f -exec wc -l {} + | awk '{sum += $1} END {print sum + 0}')
if [ "$after" -le "$before" ] || ! kill -0 "$RECORDER_PID" 2>/dev/null; then
    echo "recorder did not remain alive and sampling after independent server exit" >&2
    exit 1
fi

kill -TERM "$RECORDER_PID"
wait "$RECORDER_PID"
RECORDER_PID=""

test -s "$DATA_DIR/boot-diagnostics/host-flight-recorder/current/manifest.json"
echo "rpiz-host-recorder-sim: passed"
