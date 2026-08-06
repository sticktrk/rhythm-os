#!/usr/bin/env bash
# Prune old versioned OTA directories from Cloudflare R2 while preserving any
# directory referenced by the current beta or stable manifest.

set -euo pipefail

BUCKET="${RHYTHM_R2_BUCKET:-${CLOUDFLARE_R2_BUCKET:-}}"
PREFIX="${RHYTHM_R2_PREFIX:-server}"
ENDPOINT="${RHYTHM_R2_ENDPOINT:-${CLOUDFLARE_S3_API_ENDPOINT:-}}"
KEEP=5
DRY_RUN=false
RELEASE_ROOTS=(rpiz rpiz-stable)

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --bucket NAME    R2 bucket (default: CLOUDFLARE_R2_BUCKET)
  --prefix PREFIX  Object key prefix (default: RHYTHM_R2_PREFIX or server)
  --endpoint URL   R2 S3 endpoint (default: CLOUDFLARE_S3_API_ENDPOINT)
  --keep N         Number of releases to keep per feed (default: 5)
  --dry-run        Print objects that would be removed
  -h, --help       Show this help
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --bucket) BUCKET="${2:?--bucket requires a value}"; shift 2 ;;
        --prefix) PREFIX="${2:?--prefix requires a value}"; shift 2 ;;
        --endpoint) ENDPOINT="${2:?--endpoint requires a value}"; shift 2 ;;
        --keep) KEEP="${2:?--keep requires a value}"; shift 2 ;;
        --dry-run) DRY_RUN=true; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

PREFIX="${PREFIX#/}"
PREFIX="${PREFIX%/}"

if [ -z "$BUCKET" ] || [ -z "$ENDPOINT" ]; then
    echo "Error: R2 bucket and endpoint are required" >&2
    exit 1
fi
if [ -z "$PREFIX" ]; then
    echo "Error: refusing to prune with an empty object prefix" >&2
    exit 1
fi
if ! [[ "$KEEP" =~ ^[0-9]+$ ]] || [ "$KEEP" -lt 1 ]; then
    echo "Error: --keep must be a positive integer" >&2
    exit 1
fi
for command_name in aws jq; do
    command -v "$command_name" >/dev/null 2>&1 || {
        echo "Error: Required command not found: $command_name" >&2
        exit 1
    }
done

AWS_ARGS=(--endpoint-url "$ENDPOINT" --no-cli-pager)
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-r2-prune.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

collect_protected_dirs() {
    local root manifest key urls url rel other_root rest ver_dir

    for root in "${RELEASE_ROOTS[@]}"; do
        manifest="$TEMP_DIR/$root-manifest.json"
        key="$PREFIX/$root/manifest.json"
        if ! aws "${AWS_ARGS[@]}" s3 cp "s3://$BUCKET/$key" "$manifest" \
            --no-progress --only-show-errors; then
            echo "Error: refusing to prune without live manifest s3://$BUCKET/$key" >&2
            return 1
        fi
        if ! jq -e '
            type == "object"
            and (.package | type == "object")
            and (.package.url | type == "string")
            and ((.images // []) | type == "array")
            and all((.images // [])[]; type == "object" and (.url | type == "string"))
        ' "$manifest" >/dev/null; then
            echo "Error: refusing to prune with invalid live manifest s3://$BUCKET/$key" >&2
            return 1
        fi
        urls="$(jq -r '[.package.url?, .images[]?.url?] | .[] | select(type == "string")' "$manifest")"
        while IFS= read -r url; do
            [ -n "$url" ] || continue
            case "$url" in
                ../*)
                    rel="${url#../}"
                    other_root="${rel%%/*}"
                    rest="${rel#*/}"
                    ver_dir="${rest%%/*}"
                    ;;
                v[0-9]*/*)
                    other_root="$root"
                    ver_dir="${url%%/*}"
                    ;;
                *)
                    continue
                    ;;
            esac
            case "$other_root/$ver_dir" in
                rpiz/v[0-9]*|rpiz-stable/v[0-9]*)
                    printf '%s/%s\n' "$other_root" "$ver_dir"
                    ;;
            esac
        done <<<"$urls"
    done
}

PROTECTED_DIRS="$(collect_protected_dirs | LC_ALL=C sort -u)"

is_protected() {
    local candidate="$1"
    grep -Fqx "$candidate" <<<"$PROTECTED_DIRS"
}

list_release_dirs() {
    local root="$1"
    local root_prefix="$PREFIX/$root/"

    aws "${AWS_ARGS[@]}" s3api list-objects-v2 \
        --bucket "$BUCKET" \
        --prefix "$root_prefix" \
        --output json \
        | jq -r --arg prefix "$root_prefix" '
            .Contents[]?.Key
            | select(startswith($prefix))
            | ltrimstr($prefix)
            | split("/")[0]
            | select(test("^v[0-9]"))
        ' \
        | LC_ALL=C sort -u \
        | awk '
            {
                name = $0
                version = name
                sub(/^v/, "", version)
                prerelease = version ~ /-/ ? 0 : 1
                sub(/-.*/, "", version)
                split(version, parts, ".")
                printf "%010d.%010d.%010d.%d %s\n", parts[1], parts[2], parts[3], prerelease, name
            }
        ' \
        | sort \
        | awk '{ print $2 }'
}

prune_release_root() {
    local root="$1"
    local releases=()
    local stale_count i release key_prefix

    while IFS= read -r release; do
        [ -n "$release" ] && releases+=("$release")
    done < <(list_release_dirs "$root")

    if [ "${#releases[@]}" -le "$KEEP" ]; then
        echo "Keeping all ${#releases[@]} release(s) in $root"
        return
    fi

    stale_count=$((${#releases[@]} - KEEP))
    echo "Pruning $stale_count old release(s) from $root; keeping $KEEP"
    for ((i = 0; i < stale_count; i++)); do
        release="${releases[$i]}"
        if is_protected "$root/$release"; then
            echo "Keeping $root/$release (referenced by a feed manifest)"
            continue
        fi
        key_prefix="$PREFIX/$root/$release/"
        if [ "$DRY_RUN" = true ]; then
            echo "[dry-run] Would remove s3://$BUCKET/$key_prefix"
        else
            echo "Removing s3://$BUCKET/$key_prefix"
            aws "${AWS_ARGS[@]}" s3 rm "s3://$BUCKET/$key_prefix" \
                --recursive --only-show-errors
        fi
    done
}

for release_root in "${RELEASE_ROOTS[@]}"; do
    prune_release_root "$release_root"
done
