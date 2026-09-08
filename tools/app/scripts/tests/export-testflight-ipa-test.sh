#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
EXPORT_SCRIPT="$SCRIPT_DIR/../export-testflight-ipa.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-testflight-export-test.XXXXXX")"
cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

ARCHIVE_PATH="$TEST_ROOT/Runner.xcarchive"
EXPORT_PATH="$TEST_ROOT/ipa"
API_KEY_PATH="$TEST_ROOT/asc-api-key.json"
FAKE_XCODEBUILD="$TEST_ROOT/xcodebuild"
CAPTURE_PATH="$TEST_ROOT/capture"
mkdir -p "$ARCHIVE_PATH"

cat > "$API_KEY_PATH" <<'EOF'
{
  "key_id": "KEY123",
  "issuer_id": "issuer-456",
  "key": "TEST PRIVATE KEY",
  "in_house": false
}
EOF

cat > "$FAKE_XCODEBUILD" <<'EOF'
#!/bin/bash
set -euo pipefail

AUTH_KEY_PATH=""
EXPORT_OPTIONS_PATH=""
EXPORT_PATH=""
ALLOW_UPDATES=false
ARCHIVE_PATH=""
ARCHIVING=false
printf '%s\n' "$@" > "$RHYTHM_TEST_CAPTURE_PATH.args"
if [ "$1" = '-workspace' ]; then
    ARCHIVING=true
    grep -Fxq 'DEVELOPMENT_TEAM=TEAM123456' "$RHYTHM_TEST_CAPTURE_PATH.args"
    grep -Fxq 'CODE_SIGN_STYLE=Automatic' "$RHYTHM_TEST_CAPTURE_PATH.args"
    grep -Fxq 'generic/platform=iOS' "$RHYTHM_TEST_CAPTURE_PATH.args"
    grep -Fxq 'Release' "$RHYTHM_TEST_CAPTURE_PATH.args"
    grep -Fxq 'archive' "$RHYTHM_TEST_CAPTURE_PATH.args"
fi
while [ $# -gt 0 ]; do
    case "$1" in
        -archivePath)
            ARCHIVE_PATH="$2"
            shift 2
            ;;
        -authenticationKeyPath)
            AUTH_KEY_PATH="$2"
            shift 2
            ;;
        -exportOptionsPlist)
            EXPORT_OPTIONS_PATH="$2"
            shift 2
            ;;
        -exportPath)
            EXPORT_PATH="$2"
            shift 2
            ;;
        -allowProvisioningUpdates)
            ALLOW_UPDATES=true
            shift
            ;;
        *)
            shift
            ;;
    esac
done

test "$ALLOW_UPDATES" = true
test -f "$AUTH_KEY_PATH"
test "$(cat "$AUTH_KEY_PATH")" = "TEST PRIVATE KEY"
python3 - "$AUTH_KEY_PATH" <<'PY'
import pathlib, stat, sys
assert stat.S_IMODE(pathlib.Path(sys.argv[1]).stat().st_mode) == 0o600
PY
printf '%s\n' "$AUTH_KEY_PATH" > "$RHYTHM_TEST_CAPTURE_PATH"
if [ "$ARCHIVING" = true ]; then
    touch "$RHYTHM_TEST_CAPTURE_PATH.archive"
    if [ "${RHYTHM_TEST_ARCHIVE_STATUS:-0}" -ne 0 ]; then
        exit "$RHYTHM_TEST_ARCHIVE_STATUS"
    fi
    mkdir -p "$ARCHIVE_PATH"
    exit 0
fi
grep -q '<string>app-store-connect</string>' "$EXPORT_OPTIONS_PATH"
grep -q '<string>automatic</string>' "$EXPORT_OPTIONS_PATH"
grep -q '<string>TEAM123456</string>' "$EXPORT_OPTIONS_PATH"
grep -q '<string>lighting.rhythm.app</string>' "$EXPORT_OPTIONS_PATH"
printf '%s\n' "$AUTH_KEY_PATH" > "$RHYTHM_TEST_CAPTURE_PATH"
mkdir -p "$EXPORT_PATH"
touch "$EXPORT_PATH/Rhythm Lighting.ipa"
EOF
chmod +x "$FAKE_XCODEBUILD"

RHYTHM_XCODEBUILD_CMD="$FAKE_XCODEBUILD" \
RHYTHM_TEST_CAPTURE_PATH="$CAPTURE_PATH" \
    "$EXPORT_SCRIPT" \
        --archive "$ARCHIVE_PATH" \
        --export-path "$EXPORT_PATH" \
        --api-key "$API_KEY_PATH" \
        --team-id TEAM123456 \
        --bundle-id lighting.rhythm.app

test -f "$EXPORT_PATH/Rhythm Lighting.ipa"
AUTH_KEY_TEMP_PATH="$(cat "$CAPTURE_PATH")"
if [ -e "$AUTH_KEY_TEMP_PATH" ]; then
    echo "temporary App Store Connect key was not removed" >&2
    exit 1
fi

mkdir -p "$TEST_ROOT/Runner.xcworkspace"
RHYTHM_XCODEBUILD_CMD="$FAKE_XCODEBUILD" \
RHYTHM_TEST_CAPTURE_PATH="$CAPTURE_PATH" \
    "$EXPORT_SCRIPT" \
        --workspace "$TEST_ROOT/Runner.xcworkspace" \
        --archive "$TEST_ROOT/new archive/Runner.xcarchive" \
        --export-path "$TEST_ROOT/new ipa" \
        --api-key "$API_KEY_PATH" \
        --team-id TEAM123456 \
        --bundle-id lighting.rhythm.app
test -f "$CAPTURE_PATH.archive"
test -f "$TEST_ROOT/new ipa/Rhythm Lighting.ipa"
test ! -e "$(cat "$CAPTURE_PATH")"

if RHYTHM_XCODEBUILD_CMD="$FAKE_XCODEBUILD" \
RHYTHM_TEST_CAPTURE_PATH="$CAPTURE_PATH" RHYTHM_TEST_ARCHIVE_STATUS=65 \
    "$EXPORT_SCRIPT" \
        --workspace "$TEST_ROOT/Runner.xcworkspace" \
        --archive "$TEST_ROOT/failed archive/Runner.xcarchive" \
        --export-path "$TEST_ROOT/failed ipa" \
        --api-key "$API_KEY_PATH" \
        --team-id TEAM123456 \
        --bundle-id lighting.rhythm.app; then
    echo 'archive failure should stop before export' >&2
    exit 1
fi
test ! -f "$TEST_ROOT/failed ipa/Rhythm Lighting.ipa"
test ! -e "$(cat "$CAPTURE_PATH")"

printf '{"key_id":"KEY123","issuer_id":"issuer-456"}\n' > "$TEST_ROOT/missing-private-key.json"
if RHYTHM_XCODEBUILD_CMD="$FAKE_XCODEBUILD" \
    "$EXPORT_SCRIPT" \
        --archive "$ARCHIVE_PATH" \
        --export-path "$TEST_ROOT/missing-key-ipa" \
        --api-key "$TEST_ROOT/missing-private-key.json" \
        --team-id TEAM123456 \
        --bundle-id lighting.rhythm.app; then
    echo "missing private key should fail before export" >&2
    exit 1
fi

echo "export-testflight-ipa tests passed"
