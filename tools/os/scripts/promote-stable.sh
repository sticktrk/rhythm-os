#!/bin/bash
#
# Promote a tested beta release to the stable channel by creating a matching
# stable tag. The tag-driven GitHub Actions workflow then builds a dedicated
# stable binary, creates a stable GitHub release, and publishes
# rpiz-stable/manifest.json for appliance auto-update.
#
# Usage:
#   ./tools/os/scripts/promote-stable.sh                       # latest vX.Y.Z-beta -> vX.Y.Z-stable
#   ./tools/os/scripts/promote-stable.sh --version 0.4.261     # v0.4.261-beta -> v0.4.261-stable
#   ./tools/os/scripts/promote-stable.sh --version v0.4.261-beta
#   ./tools/os/scripts/promote-stable.sh --version v0.4.261-stable
#   ./tools/os/scripts/promote-stable.sh --dry-run
#   ./tools/os/scripts/promote-stable.sh --no-push             # create the stable tag locally only

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

REMOTE="origin"
VERSION=""
PUSH=true
DRY_RUN=false
MESSAGE=""

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Promote a beta release tag to a stable release tag.

Options:
  --version VERSION   Source release to promote. Accepts X.Y.Z, vX.Y.Z,
                      X.Y.Z-beta, vX.Y.Z-beta, X.Y.Z-stable, or
                      vX.Y.Z-stable. Defaults to the latest local
                      vX.Y.Z-beta tag.
  --message TEXT      Annotated stable tag message (default: "Release vX.Y.Z-stable")
  --remote NAME       Remote to push the stable tag to (default: origin)
  --no-push           Create the stable tag locally but do not push it
  --dry-run           Print the planned tag action without changing git state
  -h, --help          Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            if [ $# -lt 2 ]; then
                echo "Error: --version requires a value" >&2
                exit 1
            fi
            VERSION="$2"
            shift 2
            ;;
        --message)
            if [ $# -lt 2 ]; then
                echo "Error: --message requires a value" >&2
                exit 1
            fi
            MESSAGE="$2"
            shift 2
            ;;
        --remote)
            if [ $# -lt 2 ]; then
                echo "Error: --remote requires a value" >&2
                exit 1
            fi
            REMOTE="$2"
            shift 2
            ;;
        --no-push)
            PUSH=false
            shift
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

require_command() {
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: Required command not found: $1" >&2
        exit 1
    fi
}

semver_core() {
    local value="${1#v}"
    value="${value%%+*}"
    value="${value%%-*}"

    if ! printf '%s' "$value" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        echo "Error: Version must be semver X.Y.Z, optionally prefixed with v and suffixed with -beta or -stable (got '$1')" >&2
        exit 1
    fi

    echo "$value"
}

latest_beta_tag() {
    git -C "$REPO_ROOT" tag --list 'v[0-9]*-beta' --sort=-version:refname | head -n 1
}

remote_tag_commit() {
    local tag="$1"
    local commit
    commit="$(git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" "refs/tags/$tag^{}" 2>/dev/null \
        | awk 'NR == 1 { print $1 }')"
    if [ -n "$commit" ]; then
        echo "$commit"
        return
    fi
    git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" "refs/tags/$tag" 2>/dev/null \
        | awk 'NR == 1 { print $1 }'
}

tag_commit() {
    local tag="$1"
    git -C "$REPO_ROOT" rev-list -n 1 "$tag"
}

require_command git

if ! git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "Error: $REPO_ROOT is not a git repository" >&2
    exit 1
fi

if [ -z "$VERSION" ]; then
    SOURCE_TAG="$(latest_beta_tag)"
    if [ -z "$SOURCE_TAG" ]; then
        echo "Error: No local vX.Y.Z-beta tags found. Pass --version or fetch tags first." >&2
        exit 1
    fi
    CORE_VERSION="$(semver_core "$SOURCE_TAG")"
else
    CORE_VERSION="$(semver_core "$VERSION")"
    SOURCE_TAG="v$CORE_VERSION-beta"
fi

STABLE_TAG="v$CORE_VERSION-stable"
SOURCE_COMMIT="$(tag_commit "$SOURCE_TAG" 2>/dev/null || true)"

if [ -z "$SOURCE_COMMIT" ]; then
    echo "Error: Source beta tag not found locally: $SOURCE_TAG" >&2
    echo "Fetch tags or pass a version whose beta tag exists." >&2
    exit 1
fi

if [ -z "$MESSAGE" ]; then
    MESSAGE="Release $STABLE_TAG"
fi

LOCAL_STABLE_COMMIT=""
if git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$STABLE_TAG" >/dev/null 2>&1; then
    LOCAL_STABLE_COMMIT="$(tag_commit "$STABLE_TAG")"
    if [ "$LOCAL_STABLE_COMMIT" != "$SOURCE_COMMIT" ]; then
        echo "Error: Local stable tag $STABLE_TAG already points at $LOCAL_STABLE_COMMIT, not $SOURCE_COMMIT" >&2
        exit 1
    fi
fi

REMOTE_STABLE_COMMIT=""
if [ "$PUSH" = true ]; then
    REMOTE_STABLE_COMMIT="$(remote_tag_commit "$STABLE_TAG")"
    if [ -n "$REMOTE_STABLE_COMMIT" ] && [ "$REMOTE_STABLE_COMMIT" != "$SOURCE_COMMIT" ]; then
        echo "Error: Remote stable tag $STABLE_TAG already exists on $REMOTE at $REMOTE_STABLE_COMMIT" >&2
        exit 1
    fi
fi

echo "Stable promotion plan"
echo "  Source:  $SOURCE_TAG ($SOURCE_COMMIT)"
echo "  Stable:  $STABLE_TAG"
echo "  Message: $MESSAGE"
if [ "$PUSH" = true ]; then
    echo "  Remote:  $REMOTE"
else
    echo "  Remote:  (disabled by --no-push)"
fi
echo ""

if [ "$DRY_RUN" = true ]; then
    if [ -z "$LOCAL_STABLE_COMMIT" ]; then
        echo "[dry-run] Would create annotated tag: git tag -a $STABLE_TAG $SOURCE_COMMIT -m \"$MESSAGE\""
    else
        echo "[dry-run] Local stable tag already exists at the source commit."
    fi
    if [ "$PUSH" = true ]; then
        if [ -z "$REMOTE_STABLE_COMMIT" ]; then
            echo "[dry-run] Would push stable tag: git push $REMOTE refs/tags/$STABLE_TAG"
        else
            echo "[dry-run] Remote stable tag already exists at the source commit."
        fi
    fi
    exit 0
fi

if [ -z "$LOCAL_STABLE_COMMIT" ]; then
    git -C "$REPO_ROOT" tag -a "$STABLE_TAG" "$SOURCE_COMMIT" -m "$MESSAGE"
    echo "Created $STABLE_TAG at $SOURCE_COMMIT"
else
    echo "Local stable tag already exists at $SOURCE_COMMIT"
fi

if [ "$PUSH" = true ]; then
    if [ -z "$REMOTE_STABLE_COMMIT" ]; then
        git -C "$REPO_ROOT" push "$REMOTE" "refs/tags/$STABLE_TAG"
        echo "Pushed $STABLE_TAG to $REMOTE."
        echo "GitHub Actions will build and publish the stable OTA feed."
    else
        echo "Remote stable tag already exists at $SOURCE_COMMIT"
    fi
else
    echo "Tag created locally only. Push it when ready:"
    echo "  git push $REMOTE refs/tags/$STABLE_TAG"
fi
