#!/bin/bash
# Deploy ESP32-C6 firmware binary to dl.rhythm.lighting
#
# Usage: ./scripts/deploy-esp32.sh [OPTIONS]
#
# Options:
#   --changelog TEXT  Changelog text for the manifest
#   --dry-run         Show what would be done without uploading
#   -h, --help        Show this help
#
# Prerequisites:
#   - SSH access to root@ssh.dtconcepts.net
#   - ESP-IDF environment (see build-esp32.sh)
#
# URL structure:
#   dl.rhythm.lighting/esp32/
#   ├── manifest.json                 ← latest version metadata
#   ├── v0.2.0/rhythm-esp32.bin       ← versioned binaries
#   └── v0.3.0/rhythm-esp32.bin

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ESP32_DIR="$PROJECT_ROOT/rust/bins/rhythm-esp32"

# Defaults
CHANGELOG=""
DRY_RUN=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --changelog)
            CHANGELOG="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Deploy ESP32-C6 firmware to dl.rhythm.lighting"
            echo ""
            echo "Options:"
            echo "  --changelog TEXT  Changelog text for the manifest"
            echo "  --dry-run         Show what would be done without uploading"
            echo "  -h, --help        Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Resolve current firmware version
VERSION=$("$SCRIPT_DIR/resolve-version.sh" esp32)
if [ -z "$VERSION" ]; then
    echo "Error: Could not resolve ESP32 version"
    exit 1
fi
echo "Firmware version: $VERSION"

# Build release binary
echo ""
echo "=== Building release firmware ==="
"$SCRIPT_DIR/build-esp32.sh" --release

# Re-read version after build so packaging matches the built artifact
VERSION=$("$SCRIPT_DIR/resolve-version.sh" esp32)

# Find the built ELF
RELEASE_DIR="$ESP32_DIR/target/riscv32imac-esp-espidf/release"
APP_ELF="$RELEASE_DIR/rhythm-esp32"
APP_BIN="$RELEASE_DIR/rhythm-esp32.bin"

if [ ! -f "$APP_ELF" ]; then
    echo "Error: ELF binary not found at $APP_ELF"
    exit 1
fi

# Convert ELF to binary
echo ""
echo "=== Converting ELF to OTA binary ==="
espflash save-image --chip esp32c6 "$APP_ELF" "$APP_BIN"

if [ ! -f "$APP_BIN" ]; then
    echo "Error: Binary not created at $APP_BIN"
    exit 1
fi

BIN_SIZE=$(stat -f%z "$APP_BIN" 2>/dev/null || stat -c%s "$APP_BIN" 2>/dev/null)
echo "Binary size: $BIN_SIZE bytes ($(( BIN_SIZE / 1024 )) KB)"

# Generate manifest.json
MANIFEST=$(cat <<EOF
{
  "version": "$VERSION",
  "url": "v$VERSION/rhythm-esp32.bin",
  "size": $BIN_SIZE,
  "changelog": "$CHANGELOG"
}
EOF
)

echo ""
echo "=== Manifest ==="
echo "$MANIFEST"

if [ "$DRY_RUN" = true ]; then
    echo ""
    echo "=== Dry run — would upload: ==="
    echo "  $APP_BIN → root@ssh.dtconcepts.net:/var/www/dl.rhythm.lighting/esp32/v$VERSION/rhythm-esp32.bin"
    echo "  manifest.json → root@ssh.dtconcepts.net:/var/www/dl.rhythm.lighting/esp32/manifest.json"
    exit 0
fi

# Upload
REMOTE_HOST="root@ssh.dtconcepts.net"
REMOTE_DIR="/var/www/dl.rhythm.lighting/esp32"

echo ""
echo "=== Uploading firmware ==="

# Create version directory
ssh "$REMOTE_HOST" "mkdir -p $REMOTE_DIR/v$VERSION"

# Upload binary
scp "$APP_BIN" "$REMOTE_HOST:$REMOTE_DIR/v$VERSION/rhythm-esp32.bin"
echo "Uploaded: v$VERSION/rhythm-esp32.bin ($BIN_SIZE bytes)"

# Upload manifest
echo "$MANIFEST" | ssh "$REMOTE_HOST" "cat > $REMOTE_DIR/manifest.json"
echo "Uploaded: manifest.json"

echo ""
echo "=== Deploy complete ==="
echo "  Binary:   https://dl.rhythm.lighting/esp32/v$VERSION/rhythm-esp32.bin"
echo "  Manifest: https://dl.rhythm.lighting/esp32/manifest.json"
