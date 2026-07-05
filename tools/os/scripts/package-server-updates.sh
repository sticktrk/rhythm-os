#!/bin/bash
# Package rhythm-server binaries into a static OTA feed.
#
# Output layout:
#   <output>/<target>[-stable]/manifest.json
#   <output>/<target>[-stable]/v<version>/rhythm-server-<target>.tar.gz
#   <output>/<target>[-stable]/v<version>/sdcard.img.gz     (optional rpiz factory image, falls back to sdcard.img)
#   <output>/<target>[-stable]/v<version>/rootfs.ext2.gz    (optional rpiz OTA image, falls back to rootfs.ext2)
#   <output>/<target>[-stable]/latest/sdcard.img.gz         (optional latest alias)
#   <output>/<target>[-stable]/latest/rootfs.ext2.gz        (optional latest alias)
#
# Binary-only rpiz releases reuse the newest rootfs image from supplied
# previous manifests. Full image entries are published directly only when
# --image-root is supplied.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"

ARTIFACT_ROOT="$PROJECT_ROOT/dist/bin"
OUTPUT_DIR="$PROJECT_ROOT/out/server-updates"
VERSION=""
IMAGE_ROOT=""
PREVIOUS_RPIZ_MANIFESTS=()
CHANNEL=""
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

parse_rpiz_manifest_spec() {
    local spec="$1"
    local source_feed=""
    local manifest_path="$spec"

    case "$spec" in
        rpiz=*|rpiz-stable=*)
            source_feed="${spec%%=*}"
            manifest_path="${spec#*=}"
            ;;
    esac

    if [ -z "$source_feed" ]; then
        case "$(basename "$(dirname "$manifest_path")")" in
            rpiz|rpiz-stable)
                source_feed="$(basename "$(dirname "$manifest_path")")"
                ;;
        esac
    fi

    printf '%s|%s\n' "$source_feed" "$manifest_path"
}

parse_release_version_parts() {
    local value="${1#v}"
    local core pre major minor patch extra

    value="${value%%+*}"
    core="${value%%-*}"
    pre=""
    if [[ "$value" == *-* ]]; then
        pre="${value#*-}"
    fi

    IFS=. read -r major minor patch extra <<EOF
$core
EOF

    if [ -n "${extra:-}" ] \
        || ! [[ "${major:-}" =~ ^[0-9]+$ ]] \
        || ! [[ "${minor:-}" =~ ^[0-9]+$ ]] \
        || ! [[ "${patch:-}" =~ ^[0-9]+$ ]]; then
        return 1
    fi

    printf '%s %s %s %s\n' "$major" "$minor" "$patch" "$pre"
}

release_version_gt() {
    local left="$1"
    local right="$2"
    local left_parts right_parts
    local l_major l_minor l_patch l_pre
    local r_major r_minor r_patch r_pre

    [ -n "$left" ] || return 1
    if [ -z "$right" ]; then
        return 0
    fi

    left_parts="$(parse_release_version_parts "$left")" || return 1
    right_parts="$(parse_release_version_parts "$right")" || return 0
    read -r l_major l_minor l_patch l_pre <<EOF
$left_parts
EOF
    read -r r_major r_minor r_patch r_pre <<EOF
$right_parts
EOF

    if (( 10#$l_major != 10#$r_major )); then
        (( 10#$l_major > 10#$r_major ))
        return
    fi
    if (( 10#$l_minor != 10#$r_minor )); then
        (( 10#$l_minor > 10#$r_minor ))
        return
    fi
    if (( 10#$l_patch != 10#$r_patch )); then
        (( 10#$l_patch > 10#$r_patch ))
        return
    fi

    if [ -z "$l_pre" ] && [ -n "$r_pre" ]; then
        return 0
    fi
    if [ -n "$l_pre" ] && [ -z "$r_pre" ]; then
        return 1
    fi
    [[ "$l_pre" > "$r_pre" ]]
}

manifest_latest_rootfs_version() {
    local manifest_path="$1"
    local version best_version=""

    while IFS= read -r version; do
        [ -n "$version" ] || continue
        if release_version_gt "$version" "$best_version"; then
            best_version="$version"
        fi
    done < <(jq -r '.images[]? | select(.kind == "rootfs_image") | .version // empty' "$manifest_path")

    echo "$best_version"
}

manifest_images_json() {
    local manifest_path="$1"
    local source_feed="$2"
    local target_feed="$3"

    if [ -n "$source_feed" ] && [ "$source_feed" != "$target_feed" ]; then
        jq -c --arg source_feed "$source_feed" '
            (.images // [])
            | map(
                if ((.url? | type) == "string"
                    and ((.url | test("^[A-Za-z][A-Za-z0-9+.-]*:"))
                        or (.url | startswith("/"))
                        or (.url | startswith("../"))))
                then .
                else . + {"url": ("../" + $source_feed + "/" + (.url // ""))}
                end
            )
        ' "$manifest_path"
    else
        jq -c '.images // []' "$manifest_path"
    fi
}

select_previous_rpiz_images_json() {
    local target_feed="$1"
    local spec source_feed manifest_path version
    local best_feed="" best_manifest="" best_version=""

    for spec in "${PREVIOUS_RPIZ_MANIFESTS[@]}"; do
        source_feed="${spec%%|*}"
        manifest_path="${spec#*|}"
        [ -n "$source_feed" ] || source_feed="$target_feed"

        version="$(manifest_latest_rootfs_version "$manifest_path")"
        [ -n "$version" ] || continue

        if release_version_gt "$version" "$best_version"; then
            best_feed="$source_feed"
            best_manifest="$manifest_path"
            best_version="$version"
        fi
    done

    if [ -n "$best_manifest" ]; then
        echo "Carried forward rpiz image entries from $best_manifest (rootfs v$best_version)" >&2
        manifest_images_json "$best_manifest" "$best_feed" "$target_feed"
    fi
}

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Package rhythm-server binaries into a static OTA feed.

Options:
  --artifact-root PATH  Root containing dist/bin/<target>/ (default: $ARTIFACT_ROOT)
  --output-dir PATH     Output directory for packaged feed (default: $OUTPUT_DIR)
  --version VERSION     Version to package (default: resolve workspace/server version)
  --channel CHANNEL     OTA channel to package: beta or stable (default: infer from VERSION)
  --image-root PATH     Optional rpiz image directory to publish alongside the OTA manifest
  --previous-rpiz-manifest PATH
                       Existing rpiz manifest whose images may be carried
                       forward when this package has no --image-root. Can be
                       repeated. Prefix with rpiz= or rpiz-stable= when PATH
                       does not live under a feed-named directory.
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
        --channel)
            CHANNEL="$2"
            shift 2
            ;;
        --image-root)
            IMAGE_ROOT="$2"
            shift 2
            ;;
        --previous-rpiz-manifest)
            PREVIOUS_RPIZ_MANIFESTS+=("$(parse_rpiz_manifest_spec "$2")")
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
VERSION="${VERSION#v}"

if [ -z "$CHANNEL" ]; then
    case "$VERSION" in
        *-stable*)
            CHANNEL="stable"
            ;;
        *)
            CHANNEL="beta"
            ;;
    esac
fi

case "$CHANNEL" in
    beta|stable)
        ;;
    *)
        echo "Error: --channel must be beta or stable (got '$CHANNEL')" >&2
        exit 1
        ;;
esac

if [ -n "$IMAGE_ROOT" ] && [ ! -d "$IMAGE_ROOT" ]; then
    echo "Error: --image-root does not exist: $IMAGE_ROOT"
    exit 1
fi
if [ "${#PREVIOUS_RPIZ_MANIFESTS[@]}" -gt 0 ]; then
    if ! command -v jq >/dev/null 2>&1; then
        echo "Error: --previous-rpiz-manifest requires jq" >&2
        exit 1
    fi
    for manifest_spec in "${PREVIOUS_RPIZ_MANIFESTS[@]}"; do
        manifest_path="${manifest_spec#*|}"
        if [ ! -f "$manifest_path" ]; then
            echo "Error: --previous-rpiz-manifest does not exist: $manifest_path"
            exit 1
        fi
    done
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
    feed_target="$target"
    if [ "$CHANNEL" = "stable" ]; then
        feed_target="${target}-stable"
    fi
    version_dir="$OUTPUT_DIR/$feed_target/v$VERSION"
    archive_path="$version_dir/$ota_name.tar.gz"
    manifest_path="$OUTPUT_DIR/$feed_target/manifest.json"
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
            latest_dir="$OUTPUT_DIR/$feed_target/latest"
            mkdir -p "$latest_dir"
            cp "$image_src" "$latest_dir/$image_name"
            image_sha="$(sha256_file "$image_dst")"
            image_size="$(file_size "$image_dst")"
            image_kind="disk_image"
            case "$image_name" in
                rootfs.ext2|rootfs.ext2.gz)
                    image_kind="rootfs_image"
                    ;;
            esac
            image_json="{\"name\":\"$(json_escape "$image_name")\",\"kind\":\"$image_kind\",\"url\":\"v$VERSION/$(json_escape "$image_name")\",\"version\":\"$VERSION\",\"sha256\":\"$image_sha\",\"size\":$image_size"
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
    elif [ "$target" = "rpiz" ] && [ "${#PREVIOUS_RPIZ_MANIFESTS[@]}" -gt 0 ]; then
        carried_images_json="$(select_previous_rpiz_images_json "$feed_target")"
        if [ -n "$carried_images_json" ]; then
            images_json="$carried_images_json"
        fi
    fi

    cat > "$manifest_path" <<EOF
{
  "version": "$VERSION",
  "channel": "$CHANNEL",
  "published_at": "$TIMESTAMP",
  "package": {
    "name": "$(basename "$archive_path")",
    "kind": "archive_bundle",
    "url": "v$VERSION/$(basename "$archive_path")",
    "version": "$VERSION",
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
