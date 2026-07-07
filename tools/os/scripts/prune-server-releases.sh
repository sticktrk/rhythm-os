#!/usr/bin/env bash
# Prune old versioned release directories from the dl.rhythm.lighting server repo.
#
# This intentionally leaves non-versioned pointers such as <feed>/latest and
# <feed>/manifest.json alone.
#
# NOTE: this script is piped to the CDN host over `ssh ... "bash -s"`, so it
# must stay fully self-contained (no sourcing of repo-local helper libs).

set -euo pipefail

BASE_DIR="${RHYTHM_UPDATES_BASE_DIR:-}"
KEEP=5
DRY_RUN=false
RELEASE_ROOTS=(
    rpiz
    rpiz-stable
)

usage() {
    cat <<EOF
Usage: $0 --base-dir PATH [--keep N] [--dry-run]

Prune old versioned release directories from the static server repo. The newest
N v* directories are kept in each release root; older v* directories are removed.

Options:
  --base-dir PATH  Server repo base directory, usually RHYTHM_UPDATES_BASE_DIR
  --keep N         Number of releases to keep per root (default: 5)
  --dry-run        Print what would be removed without deleting anything
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --base-dir)
            BASE_DIR="$2"
            shift 2
            ;;
        --keep)
            KEEP="$2"
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
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

if [ -z "$BASE_DIR" ]; then
    echo "Error: --base-dir is required" >&2
    exit 1
fi

if ! [[ "$KEEP" =~ ^[0-9]+$ ]] || [ "$KEEP" -lt 1 ]; then
    echo "Error: --keep must be a positive integer" >&2
    exit 1
fi

case "$BASE_DIR" in
    /|"")
        echo "Error: refusing to prune unsafe base directory: '$BASE_DIR'" >&2
        exit 1
        ;;
esac

if [ ! -d "$BASE_DIR" ]; then
    echo "Server repo base directory does not exist, nothing to prune: $BASE_DIR"
    exit 0
fi

# Version directories still referenced by any feed manifest must survive
# pruning: binary-only releases carry the base image forward by URL, so the
# newest manifests can point at an older release's v* directory (same feed via
# "vX.Y.Z/..." or a sibling feed via "../rpiz/vX.Y.Z/..."). Collected as
# newline-separated "root/vX.Y.Z" entries. Parsed with grep/sed so the CDN
# host needs no jq.
collect_protected_dirs() {
    local root manifest url rel other_root rest ver_dir

    for root in "${RELEASE_ROOTS[@]}"; do
        manifest="$BASE_DIR/$root/manifest.json"
        [ -f "$manifest" ] || continue
        while IFS= read -r url; do
            [ -n "$url" ] || continue
            case "$url" in
                ../*)
                    rel="${url#../}"
                    other_root="${rel%%/*}"
                    rest="${rel#*/}"
                    ver_dir="${rest%%/*}"
                    printf '%s/%s\n' "$other_root" "$ver_dir"
                    ;;
                v[0-9]*/*)
                    ver_dir="${url%%/*}"
                    printf '%s/%s\n' "$root" "$ver_dir"
                    ;;
            esac
        done < <(grep -o '"url"[[:space:]]*:[[:space:]]*"[^"]*"' "$manifest" \
            | sed 's/.*"url"[[:space:]]*:[[:space:]]*"//; s/"$//')
    done
}

PROTECTED_DIRS="$(collect_protected_dirs || true)"

is_protected() {
    local candidate="$1"
    local entry

    while IFS= read -r entry; do
        [ -n "$entry" ] || continue
        if [ "$entry" = "$candidate" ]; then
            return 0
        fi
    done <<EOF
$PROTECTED_DIRS
EOF
    return 1
}

prune_release_root() {
    local relative_root="$1"
    local root="$BASE_DIR/$relative_root"
    local releases=()
    local stale_count
    local i
    local release
    local path

    if [ ! -d "$root" ]; then
        echo "Skipping $relative_root: missing $root"
        return 0
    fi

    while IFS= read -r release; do
        releases+=("$release")
    done < <(
        for path in "$root"/v[0-9]*; do
            [ -d "$path" ] || continue
            basename "$path"
        done | awk '
            {
                name = $0
                version = name
                sub(/^v/, "", version)
                prerelease = version ~ /-/ ? 0 : 1
                sub(/-.*/, "", version)
                split(version, parts, ".")
                printf "%010d.%010d.%010d.%d %s\n", parts[1], parts[2], parts[3], prerelease, name
            }
        ' | sort | awk '{ print $2 }'
    )

    if [ "${#releases[@]}" -le "$KEEP" ]; then
        echo "Keeping all ${#releases[@]} release(s) in $relative_root"
        return 0
    fi

    stale_count=$((${#releases[@]} - KEEP))
    echo "Pruning $stale_count old release(s) from $relative_root; keeping $KEEP"

    for ((i = 0; i < stale_count; i++)); do
        release="${releases[$i]}"
        path="$root/$release"
        if is_protected "$relative_root/$release"; then
            echo "Keeping $path (referenced by a feed manifest)"
            continue
        fi
        if [ "$DRY_RUN" = true ]; then
            echo "[dry-run] Would remove $path"
        else
            echo "Removing $path"
            rm -rf -- "$path"
        fi
    done
}

for release_root in "${RELEASE_ROOTS[@]}"; do
    prune_release_root "$release_root"
done
