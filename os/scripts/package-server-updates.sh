#!/bin/bash
# Package rhythm-server binaries into a static OTA feed.
#
# Output layout:
#   <output>/<target>/manifest.json
#   <output>/<target>/v<version>/rhythm-server-<target>
#   <output>/<target>/v<version>/rhythm-server-<target>.tar.gz
#   <output>/<target>/v<version>/SHA256SUMS.txt

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
SERVER_CRATE_DIR="$PROJECT_ROOT/rust/bins/rhythm-server"

ARTIFACT_ROOT="$PROJECT_ROOT/dist/bin"
OUTPUT_DIR="$PROJECT_ROOT/out/server-updates"
VERSION=""
DRY_RUN=false

sha256_file() {
    local file="$1"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$file" | awk '{print $1}'
    else
        shasum -a 256 "$file" | awk '{print $1}'
    fi
}

file_size() {
    local file="$1"
    stat -f%z "$file" 2>/dev/null || stat -c%s "$file" 2>/dev/null
}

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Package rhythm-server binaries into a static OTA feed.

Options:
  --artifact-root PATH  Root containing dist/bin/<target>/ (default: $ARTIFACT_ROOT)
  --output-dir PATH     Output directory for packaged feed (default: $OUTPUT_DIR)
  --version VERSION     Version to package (default: read from rhythm-server Cargo.toml)
  --dry-run             Print planned outputs without writing files
  -h, --help            Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --artifact-root)
            ARTIFACT_ROOT="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            shift 2
            ;;
        --version)
            VERSION="$2"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

if [ -z "$VERSION" ]; then
    VERSION=$(grep '^version' "$SERVER_CRATE_DIR/Cargo.toml" | head -1 | sed 's/.*"\(.*\)".*/\1/')
fi

if [ -z "$VERSION" ]; then
    echo "Error: Could not determine rhythm-server version"
    exit 1
fi

TARGETS="macos-arm64 macos-x86_64 linux-amd64 linux-aarch64 rpiz"
TIMESTAMP="$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
PACKAGED=0

if [ "$DRY_RUN" = false ]; then
    rm -rf "$OUTPUT_DIR"
    mkdir -p "$OUTPUT_DIR"
fi

for target in $TARGETS; do
    target_dir="$ARTIFACT_ROOT/$target"
    server_bin="$target_dir/rhythm-server"
    cli_bin="$target_dir/rhythm-cli"

    if [ ! -f "$server_bin" ]; then
        echo "Skipping $target: missing $server_bin"
        continue
    fi

    ota_name="rhythm-server-$target"
    version_dir="$OUTPUT_DIR/$target/v$VERSION"
    output_bin="$version_dir/$ota_name"
    archive_path="$version_dir/$ota_name.tar.gz"
    sums_path="$version_dir/SHA256SUMS.txt"
    manifest_path="$OUTPUT_DIR/$target/manifest.json"

    if [ "$DRY_RUN" = true ]; then
        echo "Would package:"
        echo "  $server_bin -> $output_bin"
        if [ -f "$cli_bin" ]; then
            echo "  $server_bin + $cli_bin -> $archive_path"
        else
            echo "  $server_bin -> $archive_path"
        fi
        continue
    fi

    mkdir -p "$version_dir"
    cp "$server_bin" "$output_bin"
    chmod 755 "$output_bin"

    if [ -f "$cli_bin" ]; then
        tar -czf "$archive_path" -C "$target_dir" rhythm-server rhythm-cli
    else
        tar -czf "$archive_path" -C "$target_dir" rhythm-server
    fi

    bin_sha="$(sha256_file "$output_bin")"
    bin_size="$(file_size "$output_bin")"
    archive_sha="$(sha256_file "$archive_path")"

    cat > "$sums_path" <<EOF
$bin_sha  $ota_name
$archive_sha  $(basename "$archive_path")
EOF

    cat > "$manifest_path" <<EOF
{
  "version": "$VERSION",
  "url": "v$VERSION/$ota_name",
  "sha256": "$bin_sha",
  "size": $bin_size,
  "published_at": "$TIMESTAMP"
}
EOF

    PACKAGED=$((PACKAGED + 1))
    echo "Packaged $target -> $version_dir"
done

if [ "$PACKAGED" -eq 0 ]; then
    echo "Error: No targets were packaged from $ARTIFACT_ROOT"
    exit 1
fi

echo ""
echo "Packaged $PACKAGED target(s) into $OUTPUT_DIR"
