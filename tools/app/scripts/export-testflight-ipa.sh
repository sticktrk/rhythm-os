#!/bin/bash
# Export an existing iOS archive with App Store Connect API-key authentication.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: export-testflight-ipa.sh --archive PATH --export-path PATH --api-key PATH --team-id ID --bundle-id ID

Exports an xcarchive for App Store Connect while allowing Xcode to create or
download managed distribution signing assets with the supplied API key.
EOF
}

ARCHIVE_PATH=""
EXPORT_PATH=""
API_KEY_PATH=""
TEAM_ID=""
BUNDLE_ID=""

while [ $# -gt 0 ]; do
    case "$1" in
        --archive)
            ARCHIVE_PATH="${2:?--archive requires a path}"
            shift 2
            ;;
        --export-path)
            EXPORT_PATH="${2:?--export-path requires a path}"
            shift 2
            ;;
        --api-key)
            API_KEY_PATH="${2:?--api-key requires a path}"
            shift 2
            ;;
        --team-id)
            TEAM_ID="${2:?--team-id requires an ID}"
            shift 2
            ;;
        --bundle-id)
            BUNDLE_ID="${2:?--bundle-id requires an ID}"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Error: unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [ -z "$ARCHIVE_PATH" ] || [ -z "$EXPORT_PATH" ] || [ -z "$API_KEY_PATH" ] || \
   [ -z "$TEAM_ID" ] || [ -z "$BUNDLE_ID" ]; then
    usage >&2
    exit 1
fi

if [ ! -d "$ARCHIVE_PATH" ]; then
    echo "Error: iOS archive not found: $ARCHIVE_PATH" >&2
    exit 1
fi
if [ ! -f "$API_KEY_PATH" ]; then
    echo "Error: App Store Connect API key not found: $API_KEY_PATH" >&2
    exit 1
fi
if ! [[ "$TEAM_ID" =~ ^[A-Z0-9]+$ ]]; then
    echo "Error: invalid Apple team ID." >&2
    exit 1
fi
if ! [[ "$BUNDLE_ID" =~ ^[A-Za-z0-9.-]+$ ]]; then
    echo "Error: invalid iOS bundle identifier." >&2
    exit 1
fi

for command in jq mktemp; do
    command -v "$command" >/dev/null 2>&1 || {
        echo "Error: required command not found: $command" >&2
        exit 1
    }
done

XCODEBUILD_CMD="${RHYTHM_XCODEBUILD_CMD:-xcodebuild}"
if ! command -v "$XCODEBUILD_CMD" >/dev/null 2>&1; then
    echo "Error: xcodebuild command not found: $XCODEBUILD_CMD" >&2
    exit 1
fi

KEY_ID="$(jq -er '.key_id | select(type == "string" and length > 0)' "$API_KEY_PATH")" || {
    echo "Error: App Store Connect API key is missing key_id." >&2
    exit 1
}
ISSUER_ID="$(jq -er '.issuer_id | select(type == "string" and length > 0)' "$API_KEY_PATH")" || {
    echo "Error: App Store Connect API key is missing issuer_id." >&2
    exit 1
}

umask 077
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-testflight-export.XXXXXX")"
cleanup() {
    rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

AUTH_KEY_FILE="$TEMP_DIR/AuthKey.p8"
EXPORT_OPTIONS_FILE="$TEMP_DIR/ExportOptions.plist"

if ! jq -er '.key | select(type == "string" and length > 0)' "$API_KEY_PATH" > "$AUTH_KEY_FILE"; then
    echo "Error: App Store Connect API key is missing private key content." >&2
    exit 1
fi

cat > "$EXPORT_OPTIONS_FILE" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>destination</key>
    <string>export</string>
    <key>distributionBundleIdentifier</key>
    <string>$BUNDLE_ID</string>
    <key>method</key>
    <string>app-store-connect</string>
    <key>signingStyle</key>
    <string>automatic</string>
    <key>stripSwiftSymbols</key>
    <true/>
    <key>teamID</key>
    <string>$TEAM_ID</string>
    <key>uploadSymbols</key>
    <true/>
</dict>
</plist>
EOF

mkdir -p "$EXPORT_PATH"
"$XCODEBUILD_CMD" -exportArchive \
    -archivePath "$ARCHIVE_PATH" \
    -exportPath "$EXPORT_PATH" \
    -exportOptionsPlist "$EXPORT_OPTIONS_FILE" \
    -allowProvisioningUpdates \
    -authenticationKeyPath "$AUTH_KEY_FILE" \
    -authenticationKeyID "$KEY_ID" \
    -authenticationKeyIssuerID "$ISSUER_ID"

IPA_FILE="$(find "$EXPORT_PATH" -name '*.ipa' -type f | head -1)"
if [ -z "$IPA_FILE" ]; then
    echo "Error: authenticated Xcode export completed without producing an IPA." >&2
    exit 1
fi

echo "Authenticated IPA export complete: $IPA_FILE"
