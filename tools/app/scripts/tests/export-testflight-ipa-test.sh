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
while [ $# -gt 0 ]; do
    case "$1" in
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
grep -q '<string>app-store-connect</string>' "$EXPORT_OPTIONS_PATH"
grep -q '<string>automatic</string>' "$EXPORT_OPTIONS_PATH"
grep -q '<string>X4K998YQ4V</string>' "$EXPORT_OPTIONS_PATH"
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
        --team-id X4K998YQ4V \
        --bundle-id lighting.rhythm.app

test -f "$EXPORT_PATH/Rhythm Lighting.ipa"
AUTH_KEY_TEMP_PATH="$(cat "$CAPTURE_PATH")"
if [ -e "$AUTH_KEY_TEMP_PATH" ]; then
    echo "temporary App Store Connect key was not removed" >&2
    exit 1
fi

printf '{"key_id":"KEY123","issuer_id":"issuer-456"}\n' > "$TEST_ROOT/missing-private-key.json"
if RHYTHM_XCODEBUILD_CMD="$FAKE_XCODEBUILD" \
    "$EXPORT_SCRIPT" \
        --archive "$ARCHIVE_PATH" \
        --export-path "$TEST_ROOT/missing-key-ipa" \
        --api-key "$TEST_ROOT/missing-private-key.json" \
        --team-id X4K998YQ4V \
        --bundle-id lighting.rhythm.app; then
    echo "missing private key should fail before export" >&2
    exit 1
fi

echo "export-testflight-ipa tests passed"
