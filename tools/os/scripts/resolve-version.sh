#!/bin/bash
#
# Resolve the current version for a shipped Rhythm artifact.
#
# Usage:
#   ./tools/os/scripts/resolve-version.sh [workspace|server|addon]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

usage() {
    cat <<EOF
Usage: $0 [workspace|server|addon|core|release] [version]

Targets:
  workspace  Resolve Git-derived workspace version from release tags
  server     Alias for workspace (workspace crates inherit this version)
  addon      Read version from install/addon/config.yaml
  core       Strip prerelease/build metadata from a semver-like version
  release    Normalize a version onto the current release channel
EOF
}

semver_core() {
    local value="${1#v}"
    value="${value%%+*}"
    value="${value%%-*}"

    if ! printf '%s' "$value" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        echo "Error: Version core must be semver X.Y.Z (got '$1')" >&2
        exit 1
    fi

    echo "$value"
}

release_version() {
    local core
    core="$(semver_core "$1")"
    echo "${core}-beta"
}

read_workspace_version() {
    awk -F'"' '
        /^\[workspace\.package\]/ { in_workspace = 1; next }
        /^\[/ && in_workspace { exit }
        in_workspace && $0 ~ /^version[[:space:]]*=/ { print $2; exit }
    ' "$REPO_ROOT/Cargo.toml"
}

resolve_workspace_version() {
    local exact_tag latest_tag latest_tag_core next_tag_version base_version base_core commit_count sha describe_output dirty_suffix

    describe_output="$(git -C "$REPO_ROOT" describe --tags --match 'v[0-9]*' --always --dirty 2>/dev/null || true)"
    dirty_suffix=""
    if [[ "$describe_output" == *-dirty ]]; then
        dirty_suffix=".dirty"
    fi

    exact_tag="$(git -C "$REPO_ROOT" describe --tags --exact-match --match 'v[0-9]*' HEAD 2>/dev/null || true)"
    if [ -n "$exact_tag" ]; then
        echo "${exact_tag#v}${dirty_suffix}"
        return
    fi

    base_version="$(release_version "$(read_workspace_version)")"
    base_core="$(semver_core "$base_version")"
    latest_tag="$(git -C "$REPO_ROOT" describe --tags --abbrev=0 --match 'v[0-9]*' HEAD 2>/dev/null || true)"
    if [ -n "$latest_tag" ]; then
        latest_tag_core="$(semver_core "${latest_tag#v}")"
        next_tag_version="$(bump_patch "$latest_tag_core")"
        if semver_gte "$next_tag_version" "$base_core"; then
            base_version="$(release_version "$next_tag_version")"
        fi
        commit_count="$(git -C "$REPO_ROOT" rev-list --count "${latest_tag}..HEAD")"
    else
        commit_count="$(git -C "$REPO_ROOT" rev-list --count HEAD)"
    fi

    sha="$(git -C "$REPO_ROOT" rev-parse --short=8 HEAD)"

    VERSION="${base_version}.dev.${commit_count}.g${sha}"
    VERSION="${VERSION}${dirty_suffix}"

    echo "$VERSION"
}

semver_gte() {
    local left="$1" right="$2"
    local left_major left_minor left_patch right_major right_minor right_patch

    IFS=. read -r left_major left_minor left_patch <<EOF
$left
EOF
    IFS=. read -r right_major right_minor right_patch <<EOF
$right
EOF

    left_major="${left_major:-0}"
    left_minor="${left_minor:-0}"
    left_patch="${left_patch:-0}"
    right_major="${right_major:-0}"
    right_minor="${right_minor:-0}"
    right_patch="${right_patch:-0}"

    if [ "$left_major" -ne "$right_major" ]; then
        [ "$left_major" -gt "$right_major" ]
        return
    fi

    if [ "$left_minor" -ne "$right_minor" ]; then
        [ "$left_minor" -gt "$right_minor" ]
        return
    fi

    [ "$left_patch" -ge "$right_patch" ]
}

bump_patch() {
    local version="$1"
    local major minor patch

    IFS=. read -r major minor patch <<EOF
$version
EOF

    major="${major:-0}"
    minor="${minor:-0}"
    patch="${patch:-0}"

    echo "${major}.${minor}.$((patch + 1))"
}

resolve_addon_version() {
    awk '
        /^version:/ {
            gsub(/"/, "", $2);
            print $2;
            exit;
        }
    ' "$PROJECT_ROOT/install/addon/config.yaml"
}

TARGET="${1:-workspace}"

case "$TARGET" in
    workspace|server)
        VERSION="$(resolve_workspace_version)"
        ;;
    addon)
        VERSION="$(resolve_addon_version)"
        ;;
    core)
        if [ $# -lt 2 ]; then
            echo "Error: core target requires a version argument" >&2
            usage >&2
            exit 1
        fi
        VERSION="$(semver_core "$2")"
        ;;
    release)
        if [ $# -lt 2 ]; then
            echo "Error: release target requires a version argument" >&2
            usage >&2
            exit 1
        fi
        VERSION="$(release_version "$2")"
        ;;
    -h|--help)
        usage
        exit 0
        ;;
    *)
        echo "Unknown target: $TARGET" >&2
        usage >&2
        exit 1
        ;;
esac

if [ -z "${VERSION:-}" ]; then
    echo "Error: Could not resolve version for target '$TARGET'" >&2
    exit 1
fi

echo "$VERSION"
