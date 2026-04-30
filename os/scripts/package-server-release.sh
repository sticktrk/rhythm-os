#!/usr/bin/env bash
# Package a rhythm-server desktop release tarball for CDN publishing.
#
# Reads dist/bin/<target>/{rhythm-server,rhythm-cli} and produces a tarball
# laid out for the bootstrap installer at install/pages/install.sh:
#
#   rhythm-server-<VERSION>-<target>/
#     VERSION
#     LICENSE.md
#     README.md
#     bin/{rhythm-server,rhythm-cli}
#     install/install.sh
#     install/macos/com.rhythm.lighting.server.plist
#     install/linux/rhythm-server.service
#
# Output:
#   <output-dir>/rhythm-server-<VERSION>-<target>.tar.gz
#   <output-dir>/rhythm-server-<VERSION>-<target>.tar.gz.sha256
#   <output-dir>/rhythm-server-latest-<target>.tar.gz
#   <output-dir>/rhythm-server-latest-<target>.tar.gz.sha256
#
# Usage:
#   scripts/package-server-release.sh --target macos-arm64 --version v0.4.176-beta --output dist/release-assets
#
# (rpiz is intentionally NOT supported here — its existing release tarball is
# produced inline by the rpiz CI job and has a flat layout for OTA.)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

TARGET=""
VERSION=""
OUTPUT_DIR="$PROJECT_ROOT/dist/release-assets"

usage() {
    cat <<EOF
Usage: $0 --target <target> [--version <version>] [--output <dir>]

Targets: macos-arm64, macos-x86_64, linux-amd64, linux-aarch64
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --target)  TARGET="$2"; shift 2 ;;
        --version) VERSION="$2"; shift 2 ;;
        --output)  OUTPUT_DIR="$2"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

case "$TARGET" in
    macos-arm64|macos-x86_64|linux-amd64|linux-aarch64) ;;
    "") echo "Error: --target is required" >&2; usage >&2; exit 1 ;;
    *)  echo "Error: unsupported target '$TARGET'" >&2; usage >&2; exit 1 ;;
esac

if [ -z "$VERSION" ]; then
    VERSION="$("$SCRIPT_DIR/resolve-version.sh" server)"
fi
VERSION="${VERSION#v}"

BIN_DIR="$PROJECT_ROOT/dist/bin/$TARGET"
SERVER_BIN="$BIN_DIR/rhythm-server"
CLI_BIN="$BIN_DIR/rhythm-cli"

[ -f "$SERVER_BIN" ] || { echo "Error: $SERVER_BIN not found. Build with scripts/build-server.sh --release --target $TARGET" >&2; exit 1; }
[ -f "$CLI_BIN" ]    || { echo "Error: $CLI_BIN not found." >&2; exit 1; }

ARCHIVE_NAME="rhythm-server-${VERSION}-${TARGET}"
ARCHIVE_FILE="${ARCHIVE_NAME}.tar.gz"
LATEST_ARCHIVE_FILE="rhythm-server-latest-${TARGET}.tar.gz"

mkdir -p "$OUTPUT_DIR"

STAGE_ROOT="$(mktemp -d -t rhythm-pkg.XXXXXX)"
trap 'rm -rf "$STAGE_ROOT"' EXIT
STAGE="$STAGE_ROOT/$ARCHIVE_NAME"
mkdir -p "$STAGE/bin" "$STAGE/install/macos" "$STAGE/install/linux"

cp "$SERVER_BIN" "$STAGE/bin/rhythm-server"
cp "$CLI_BIN"    "$STAGE/bin/rhythm-cli"
chmod +x "$STAGE/bin/rhythm-server" "$STAGE/bin/rhythm-cli"

cp "$PROJECT_ROOT/install/install.sh"                           "$STAGE/install/install.sh"
cp "$PROJECT_ROOT/install/macos/com.rhythm.lighting.server.plist" "$STAGE/install/macos/com.rhythm.lighting.server.plist"
cp "$PROJECT_ROOT/install/linux/rhythm-server.service"          "$STAGE/install/linux/rhythm-server.service"
chmod +x "$STAGE/install/install.sh"

cp "$PROJECT_ROOT/LICENSE.md" "$STAGE/LICENSE.md"
printf '%s\n' "v${VERSION}" > "$STAGE/VERSION"

cat > "$STAGE/README.md" <<EOF
# rhythm-server $VERSION ($TARGET)

This tarball is what \`curl -fsSL https://get.rhythm.lighting/install.sh | bash\`
unpacks. To install manually:

    tar -xzf $ARCHIVE_FILE
    cd $ARCHIVE_NAME
    ./install/install.sh --binary ./bin/rhythm-server         # macOS / Linux system
    ./install/install.sh --binary ./bin/rhythm-server --user  # Linux user

Pass \`--no-start\` to install without starting the service. Pass
\`--uninstall\` to remove. See \`./install/install.sh --help\` for options.
EOF

tar -czf "$OUTPUT_DIR/$ARCHIVE_FILE" -C "$STAGE_ROOT" "$ARCHIVE_NAME"

if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUTPUT_DIR" && sha256sum "$ARCHIVE_FILE" > "$ARCHIVE_FILE.sha256")
else
    (cd "$OUTPUT_DIR" && shasum -a 256 "$ARCHIVE_FILE" > "$ARCHIVE_FILE.sha256")
fi

cp "$OUTPUT_DIR/$ARCHIVE_FILE" "$OUTPUT_DIR/$LATEST_ARCHIVE_FILE"
if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUTPUT_DIR" && sha256sum "$LATEST_ARCHIVE_FILE" > "$LATEST_ARCHIVE_FILE.sha256")
else
    (cd "$OUTPUT_DIR" && shasum -a 256 "$LATEST_ARCHIVE_FILE" > "$LATEST_ARCHIVE_FILE.sha256")
fi

echo "Packaged: $OUTPUT_DIR/$ARCHIVE_FILE"
echo "         $OUTPUT_DIR/$ARCHIVE_FILE.sha256"
echo "         $OUTPUT_DIR/$LATEST_ARCHIVE_FILE"
echo "         $OUTPUT_DIR/$LATEST_ARCHIVE_FILE.sha256"
