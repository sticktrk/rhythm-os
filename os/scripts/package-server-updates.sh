#!/bin/bash
# Package rhythm-server binaries into a static OTA feed.
#
# Output layout:
#   <output>/<target>/manifest.json
#   <output>/<target>/v<version>/rhythm-server-<target>.tar.gz
#   <output>/<target>/v<version>/sdcard.img.gz     (optional rpiz factory image, falls back to sdcard.img)
#   <output>/<target>/v<version>/rootfs.ext2.gz    (optional rpiz OTA image, falls back to rootfs.ext2)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

ARTIFACT_ROOT="$PROJECT_ROOT/dist/bin"
OUTPUT_DIR="$PROJECT_ROOT/out/server-updates"
VERSION=""
IMAGE_ROOT=""
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

json_escape() {
    printf '%s' "$1" | sed -e 's/\\/\\\\/g' -e 's/"/\\"/g'
}

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Package rhythm-server binaries into a static OTA feed.

Options:
  --artifact-root PATH  Root containing dist/bin/<target>/ (default: $ARTIFACT_ROOT)
  --output-dir PATH     Output directory for packaged feed (default: $OUTPUT_DIR)
  --version VERSION     Version to package (default: resolve workspace/server version)
  --image-root PATH     Optional rpiz image directory to publish alongside the OTA manifest
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
        --image-root)
            IMAGE_ROOT="$2"
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
    VERSION="$("$SCRIPT_DIR/resolve-version.sh" server)"
fi

if [ -z "$VERSION" ]; then
    echo "Error: Could not determine rhythm-server version"
    exit 1
fi

if [ -n "$IMAGE_ROOT" ] && [ ! -d "$IMAGE_ROOT" ]; then
    echo "Error: --image-root does not exist: $IMAGE_ROOT"
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
    chipd_bin="$target_dir/rhythm-chipd"

    if [ ! -f "$server_bin" ]; then
        echo "Skipping $target: missing $server_bin"
        continue
    fi

    ota_name="rhythm-server-$target"
    version_dir="$OUTPUT_DIR/$target/v$VERSION"
    archive_path="$version_dir/$ota_name.tar.gz"
    manifest_path="$OUTPUT_DIR/$target/manifest.json"
    image_candidates=()
    if [ "$target" = "rpiz" ] && [ -n "$IMAGE_ROOT" ]; then
        if [ -f "$IMAGE_ROOT/sdcard.img.gz" ]; then
            image_candidates+=("sdcard.img.gz")
        elif [ -f "$IMAGE_ROOT/sdcard.img" ]; then
            image_candidates+=("sdcard.img")
        fi
        if [ -f "$IMAGE_ROOT/rootfs.ext2.gz" ]; then
            image_candidates+=("rootfs.ext2.gz")
        elif [ -f "$IMAGE_ROOT/rootfs.ext2" ]; then
            image_candidates+=("rootfs.ext2")
        fi
    fi

    if [ "$DRY_RUN" = true ]; then
        echo "Would package:"
        archive_members=("rhythm-server")
        [ -f "$cli_bin" ] && archive_members+=("rhythm-cli")
        [ -f "$chipd_bin" ] && archive_members+=("rhythm-chipd")
        echo "  ${archive_members[*]} -> $archive_path"
        if [ "${#image_candidates[@]}" -gt 0 ]; then
            for image_name in "${image_candidates[@]}"; do
                echo "  $IMAGE_ROOT/$image_name -> $version_dir/$image_name"
            done
        fi
        PACKAGED=$((PACKAGED + 1))
        continue
    fi

    mkdir -p "$version_dir"

    archive_members=("rhythm-server")
    [ -f "$cli_bin" ] && archive_members+=("rhythm-cli")
    [ -f "$chipd_bin" ] && archive_members+=("rhythm-chipd")
    tar -czf "$archive_path" -C "$target_dir" "${archive_members[@]}"

    archive_sha="$(sha256_file "$archive_path")"
    archive_size="$(file_size "$archive_path")"

    install_entries=()
    install_entries+=('{"archive_path":"rhythm-server","slot":"self","required":true}')
    [ -f "$chipd_bin" ] && install_entries+=('{"archive_path":"rhythm-chipd","slot":"sibling","path":"rhythm-chipd","required":true}')
    [ -f "$cli_bin" ] && install_entries+=('{"archive_path":"rhythm-cli","slot":"sibling","path":"rhythm-cli","required":false}')
    install_json=""
    if [ "${#install_entries[@]}" -gt 0 ]; then
        install_json="$(printf '%s' "${install_entries[0]}")"
        if [ "${#install_entries[@]}" -gt 1 ]; then
            for ((i=1; i<${#install_entries[@]}; i++)); do
                install_json="$install_json,${install_entries[$i]}"
            done
        fi
    fi

    image_entries=()
    if [ "${#image_candidates[@]}" -gt 0 ]; then
        for image_name in "${image_candidates[@]}"; do
            image_src="$IMAGE_ROOT/$image_name"
            image_dst="$version_dir/$image_name"
            cp "$image_src" "$image_dst"
            image_sha="$(sha256_file "$image_dst")"
            image_size="$(file_size "$image_dst")"
            image_kind="disk_image"
            case "$image_name" in
                rootfs.ext2|rootfs.ext2.gz)
                    image_kind="rootfs_image"
                    ;;
            esac
            image_json="{\"name\":\"$(json_escape "$image_name")\",\"kind\":\"$image_kind\",\"url\":\"v$VERSION/$(json_escape "$image_name")\",\"sha256\":\"$image_sha\",\"size\":$image_size"
            case "$image_name" in
                *.gz)
                    image_json="$image_json,\"compression\":\"gzip\""
                    ;;
            esac
            image_json="$image_json}"
            image_entries+=("$image_json")
        done
    fi

    images_json="[]"
    if [ "${#image_entries[@]}" -gt 0 ]; then
        images_json="["
        for ((i=0; i<${#image_entries[@]}; i++)); do
            [ "$i" -gt 0 ] && images_json="$images_json,"
            images_json="$images_json${image_entries[$i]}"
        done
        images_json="$images_json]"
    fi

    cat > "$manifest_path" <<EOF
{
  "version": "$VERSION",
  "published_at": "$TIMESTAMP",
  "package": {
    "name": "$(basename "$archive_path")",
    "kind": "archive_bundle",
    "url": "v$VERSION/$(basename "$archive_path")",
    "sha256": "$archive_sha",
    "size": $archive_size,
    "install": [$install_json]
  },
  "images": $images_json
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
