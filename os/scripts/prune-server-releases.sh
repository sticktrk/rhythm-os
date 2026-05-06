#!/usr/bin/env bash
# Prune old versioned release directories from the dl.rhythm.lighting server repo.
#
# This intentionally leaves non-versioned pointers such as install/latest,
# install/latest.txt, <target>/latest, and <target>/manifest.json alone.

set -euo pipefail

BASE_DIR="${RHYTHM_UPDATES_BASE_DIR:-}"
KEEP=5
DRY_RUN=false
RELEASE_ROOTS=(
    install
    macos-arm64
    macos-x86_64
    linux-amd64
    linux-aarch64
    rpiz
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
