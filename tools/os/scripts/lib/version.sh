# Shared version/semver helpers for the Rhythm release scripts.
#
# Source this file; do not execute it. Callers are expected to run under
# `set -euo pipefail` and to have REPO_ROOT pointing at the CROSS repo root
# (or to pass an explicit path where a function accepts one).
#
# This is the single home for semver parsing/comparison in the release
# tooling. Exception: prune-server-releases.sh is piped to the CDN host over
# `ssh bash -s` and must stay self-contained, so its sort stays inline there.

# Strip a leading v and any prerelease/build metadata; validate X.Y.Z.
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

# Normalize a version onto the beta release channel: core + "-beta".
release_version() {
    local core
    core="$(semver_core "$1")"
    echo "${core}-beta"
}

# Read [workspace.package] version from the root Cargo.toml.
# Usage: read_workspace_version [repo_root]  (defaults to $REPO_ROOT)
read_workspace_version() {
    local repo_root="${1:-$REPO_ROOT}"
    awk -F'"' '
        /^\[workspace\.package\]/ { in_workspace = 1; next }
        /^\[/ && in_workspace { exit }
        in_workspace && $0 ~ /^version[[:space:]]*=/ { print $2; exit }
    ' "$repo_root/Cargo.toml"
}

# Echo "major minor patch" for a semver-like version.
split_version() {
    local version major minor patch
    version="$(semver_core "$1")"

    IFS=. read -r major minor patch <<EOF
$version
EOF

    echo "${major:-0} ${minor:-0} ${patch:-0}"
}

# Bump a version core by kind: major|minor|patch.
bump_version() {
    local version="$1"
    local kind="$2"
    local major minor patch

    read -r major minor patch <<<"$(split_version "$version")"

    case "$kind" in
        major)
            echo "$((major + 1)).0.0"
            ;;
        minor)
            echo "${major}.$((minor + 1)).0"
            ;;
        patch)
            echo "${major}.${minor}.$((patch + 1))"
            ;;
        *)
            echo "Error: Unknown bump kind: $kind" >&2
            exit 1
            ;;
    esac
}

bump_patch() {
    bump_version "$1" patch
}

# Core-only comparison (prerelease ignored): left > right.
semver_gt() {
    local left="$1" right="$2"
    local l_major l_minor l_patch r_major r_minor r_patch

    read -r l_major l_minor l_patch <<<"$(split_version "$left")"
    read -r r_major r_minor r_patch <<<"$(split_version "$right")"

    if [ "$l_major" -ne "$r_major" ]; then
        [ "$l_major" -gt "$r_major" ]
        return
    fi

    if [ "$l_minor" -ne "$r_minor" ]; then
        [ "$l_minor" -gt "$r_minor" ]
        return
    fi

    [ "$l_patch" -gt "$r_patch" ]
}

# Core-only comparison (prerelease ignored): left >= right.
semver_gte() {
    local left="$1" right="$2"

    if semver_gt "$left" "$right"; then
        return 0
    fi
    [ "$(semver_core "$left")" = "$(semver_core "$right")" ]
}

# Echo "major minor patch prerelease" for a release version, or fail (return 1)
# when the value is not well-formed. Unlike semver_core this preserves the
# prerelease part and never exits the caller.
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

# Prerelease-aware comparison: left > right, where a release outranks its own
# prereleases (0.5.0 > 0.5.0-beta) and prereleases compare lexically.
# An empty right always loses; an unparseable right loses to a parseable left.
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

# Map a version string to its OTA channel: *-stable* -> stable, else beta.
channel_for_version() {
    case "$1" in
        *-stable*)
            echo "stable"
            ;;
        *)
            echo "beta"
            ;;
    esac
}

# Map an OTA channel to its rpiz feed directory name.
feed_for_channel() {
    case "$1" in
        beta)
            echo "rpiz"
            ;;
        stable)
            echo "rpiz-stable"
            ;;
        *)
            echo "Error: Unknown OTA channel: $1 (expected beta or stable)" >&2
            exit 1
            ;;
    esac
}
