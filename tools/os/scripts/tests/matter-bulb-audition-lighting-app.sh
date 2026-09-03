#!/bin/bash

# Exercise the headless Bulb Audition control journey against Project CHIP's
# Linux lighting-app. The focused Rust tests prove that these scenario steps
# are emitted by the same builder as runtime control; this test supplies the
# independent reference-device boundary and verifies ExecuteIfOff behavior,
# readback, and subscription establishment.

set -euo pipefail

LIGHTING_APP="${RHYTHM_LIGHTING_APP_BIN:-}"
CHIP_TOOL="${RHYTHM_CHIP_TOOL_BIN:-}"

if [ ! -x "$LIGHTING_APP" ]; then
    echo "RHYTHM_LIGHTING_APP_BIN must name an executable chip-lighting-app" >&2
    exit 64
fi
if [ ! -x "$CHIP_TOOL" ]; then
    echo "RHYTHM_CHIP_TOOL_BIN must name an executable chip-tool" >&2
    exit 64
fi

TEST_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-bulb-audition.XXXXXX")"
APP_LOG="$TEST_DIR/lighting-app.log"
STORAGE_DIR="$TEST_DIR/chip-tool-storage"
mkdir -p "$STORAGE_DIR"
APP_PID=""

cleanup() {
    if [ -n "$APP_PID" ] && kill -0 "$APP_PID" 2>/dev/null; then
        kill "$APP_PID" 2>/dev/null || true
        wait "$APP_PID" 2>/dev/null || true
    fi
    rm -rf "$TEST_DIR"
}
trap cleanup EXIT INT TERM

fail() {
    echo "BULB AUDITION LIGHTING-APP: $*" >&2
    echo "--- lighting-app tail ---" >&2
    tail -80 "$APP_LOG" >&2 || true
    exit 1
}

run_tool() {
    "$CHIP_TOOL" "$@" --storage-directory "$STORAGE_DIR" --timeout 15
}

read_attribute() {
    output_file="$1"
    shift
    run_tool "$@" >"$output_file" 2>&1 || {
        sed -n '1,160p' "$output_file" >&2
        fail "attribute read failed: $*"
    }
}

expect_attribute() {
    output_file="$1"
    pattern="$2"
    if ! LC_ALL=C sed 's/\x1b\[[0-9;]*m//g' "$output_file" | grep -Eq "$pattern"; then
        sed -n '1,160p' "$output_file" >&2
        fail "readback did not match $pattern"
    fi
}

"$LIGHTING_APP" \
    --KVS "$TEST_DIR/lighting-app.kvs" \
    --discriminator 3840 \
    --passcode 20202021 \
    >"$APP_LOG" 2>&1 &
APP_PID=$!

ready=false
attempt=0
while [ "$attempt" -lt 30 ]; do
    if ! kill -0 "$APP_PID" 2>/dev/null; then
        fail "lighting-app exited before opening its commissioning window"
    fi
    if grep -Eq 'SetupQRCode|Commissioning window is now open|CHIP:SVR.*Server Listening' "$APP_LOG"; then
        ready=true
        break
    fi
    sleep 1
    attempt=$((attempt + 1))
done
[ "$ready" = true ] || fail "lighting-app did not become ready"

echo "Audition preflight: commission the reference light"
run_tool pairing onnetwork 1 20202021 --bypass-attestation-verifier true \
    >"$TEST_DIR/pairing.log" 2>&1 || {
    sed -n '1,200p' "$TEST_DIR/pairing.log" >&2
    fail "on-network commissioning failed"
}

echo "Audition scenario: turn on from off with the runtime's staged plan"
run_tool onoff off 1 1 >"$TEST_DIR/off.log" 2>&1
run_tool colorcontrol move-to-color-temperature 370 0 1 1 1 1 \
    >"$TEST_DIR/warm.log" 2>&1
read_attribute "$TEST_DIR/off-readback.log" onoff read on-off 1 1
expect_attribute "$TEST_DIR/off-readback.log" 'OnOff: (FALSE|false|0)'
read_attribute "$TEST_DIR/staged-color-readback.log" \
    colorcontrol read color-temperature-mireds 1 1
expect_attribute "$TEST_DIR/staged-color-readback.log" 'ColorTemperatureMireds: 370'
run_tool levelcontrol move-to-level-with-on-off 76 0 0 0 1 1 \
    >"$TEST_DIR/level-on.log" 2>&1
read_attribute "$TEST_DIR/on-readback.log" onoff read on-off 1 1
expect_attribute "$TEST_DIR/on-readback.log" 'OnOff: (TRUE|true|1)'
read_attribute "$TEST_DIR/level-readback.log" levelcontrol read current-level 1 1
expect_attribute "$TEST_DIR/level-readback.log" 'CurrentLevel: 76'

echo "Audition scenario: tick while on"
run_tool colorcontrol move-to-color-temperature 167 0 1 1 1 1 \
    >"$TEST_DIR/cool.log" 2>&1
run_tool levelcontrol move-to-level-with-on-off 203 0 0 0 1 1 \
    >"$TEST_DIR/bright.log" 2>&1
read_attribute "$TEST_DIR/cool-readback.log" \
    colorcontrol read color-temperature-mireds 1 1
expect_attribute "$TEST_DIR/cool-readback.log" 'ColorTemperatureMireds: 167'
read_attribute "$TEST_DIR/bright-readback.log" levelcontrol read current-level 1 1
expect_attribute "$TEST_DIR/bright-readback.log" 'CurrentLevel: 203'

echo "Audition scenario: plain On restores the prior level"
run_tool levelcontrol move-to-level-with-on-off 127 0 0 0 1 1 \
    >"$TEST_DIR/restore-level.log" 2>&1
run_tool onoff off 1 1 >"$TEST_DIR/restore-off.log" 2>&1
run_tool onoff on 1 1 >"$TEST_DIR/restore-on.log" 2>&1
read_attribute "$TEST_DIR/restore-on-readback.log" onoff read on-off 1 1
expect_attribute "$TEST_DIR/restore-on-readback.log" 'OnOff: (TRUE|true|1)'
read_attribute "$TEST_DIR/restore-level-readback.log" levelcontrol read current-level 1 1
expect_attribute "$TEST_DIR/restore-level-readback.log" 'CurrentLevel: 127'

echo "Audition scenario: subscription establishment and priming truth"
# A healthy subscription is intentionally open-ended. Some chip-tool versions
# return their timeout status after already delivering valid reports, so the
# report itself—not process lifetime—is the evidence boundary here.
run_tool colorcontrol subscribe color-temperature-mireds 0 2 1 1 \
    >"$TEST_DIR/subscription.log" 2>&1 || true
expect_attribute "$TEST_DIR/subscription.log" 'ColorTemperatureMireds: 167'

echo "Bulb Audition passed against connectedhomeip/examples/lighting-app/linux"
