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

# shellcheck source=lib/version.sh
source "$SCRIPT_DIR/lib/version.sh"

usage() {
    cat <<EOF
Usage: $0 [workspace|server|addon|core|release] [version]

Targets:
  workspace  Resolve Git-derived workspace version from release tags
  server     Alias for workspace (workspace crates inherit this version)
  addon      Read version from install/addon/config.yaml
  core       Strip prerelease/build metadata from a semver-like version
  release    Normalize a version onto the beta release channel
EOF
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
