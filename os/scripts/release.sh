#!/bin/bash
#
# Create and push a server/embedded release tag so GitHub Actions can publish
# the GitHub release assets.
#
# Usage:
#   ./scripts/release.sh
#   ./scripts/release.sh --minor
#   ./scripts/release.sh --version 0.5.0

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REMOTE="origin"
VERSION=""
BUMP_KIND="patch"
PUSH=true
DRY_RUN=false
MESSAGE=""
WORKSPACE_VERSION_FILES=("Cargo.toml" "Cargo.lock")
WORKSPACE_PACKAGES=(
    rhythm-addon
    rhythm-core
    rhythm-devices
    rhythm-ha
    rhythm-hue
    rhythm-linux-embedded
    rhythm-matter
    rhythm-os
    rhythm-profile
    rhythm-server
)

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Create and push a release tag. The GitHub release workflow then builds and
uploads the release assets for that tag. The script updates the workspace crate
version first so Cargo metadata, runtime version reporting, and the git tag all
match.

Options:
  --version X.Y.Z   Use an explicit version instead of auto-bumping
  --major           Bump the latest vX.Y.Z tag to the next major version
  --minor           Bump the latest vX.Y.Z tag to the next minor version
  --patch           Bump the latest vX.Y.Z tag to the next patch version (default)
  --message TEXT    Annotated tag message (default: "Release vX.Y.Z")
  --remote NAME     Remote to push to (default: origin)
  --no-push         Create the local tag but do not push branch or tag
  --dry-run         Print the planned tag/push actions without changing git state
  -h, --help        Show this help

Examples:
  $0
  $0 --minor
  $0 --version 0.4.1
  $0 --version v0.4.1 --dry-run
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            VERSION="$2"
            shift 2
            ;;
        --major)
            BUMP_KIND="major"
            shift
            ;;
        --minor)
            BUMP_KIND="minor"
            shift
            ;;
        --patch)
            BUMP_KIND="patch"
            shift
            ;;
        --message)
            MESSAGE="$2"
            shift 2
            ;;
        --remote)
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

read_workspace_version() {
    awk -F'"' '
        /^\[workspace\.package\]/ { in_workspace = 1; next }
        /^\[/ && in_workspace { exit }
        in_workspace && $0 ~ /^version[[:space:]]*=/ { print $2; exit }
    ' "$PROJECT_ROOT/Cargo.toml"
}

normalize_version() {
    local value="$1"
    value="${value#v}"
    if ! printf '%s' "$value" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+$'; then
        echo "Error: Version must be semver X.Y.Z or vX.Y.Z" >&2
        exit 1
    fi
    echo "$value"
}

split_version() {
    local version="$1"
    local major minor patch

    IFS=. read -r major minor patch <<EOF
$version
EOF

    echo "${major:-0} ${minor:-0} ${patch:-0}"
}

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

tracked_worktree_dirty() {
    [ -n "$(git -C "$PROJECT_ROOT" status --short --untracked-files=no)" ]
}

github_repo_url() {
    local remote_url
    remote_url="$(git -C "$PROJECT_ROOT" remote get-url "$REMOTE" 2>/dev/null || true)"

    if [[ "$remote_url" =~ ^git@github\.com:(.+)\.git$ ]]; then
        echo "https://github.com/${BASH_REMATCH[1]}"
        return
    fi

    if [[ "$remote_url" =~ ^https://github\.com/(.+)\.git$ ]]; then
        echo "https://github.com/${BASH_REMATCH[1]}"
        return
    fi

    if [[ "$remote_url" =~ ^https://github\.com/(.+)$ ]]; then
        echo "$remote_url"
        return
    fi

    echo ""
}

update_workspace_version_files() {
    local new_version="$1"
    local current_version="$2"
    local pkg

    if [ "$current_version" = "$new_version" ]; then
        return 0
    fi

    NEW_VERSION="$new_version" perl -0pi -e '
        s/(\[workspace\.package\]\n(?:[^\[]*\n)*?version = ")[^"]+(")/$1.$ENV{NEW_VERSION}.$2/se
    ' "$PROJECT_ROOT/Cargo.toml"

    for pkg in "${WORKSPACE_PACKAGES[@]}"; do
        PKG_NAME="$pkg" NEW_VERSION="$new_version" perl -0pi -e '
            s/(\[\[package\]\]\nname = "\Q$ENV{PKG_NAME}\E"\nversion = ")[^"]+(")/$1.$ENV{NEW_VERSION}.$2/se
        ' "$PROJECT_ROOT/Cargo.lock"
    done
}

commit_release_version_update() {
    local tag="$1"
    local commit_message="Release $tag"
    local file
    local changed=false

    for file in "${WORKSPACE_VERSION_FILES[@]}"; do
        if ! git -C "$PROJECT_ROOT" diff --quiet -- "$file"; then
            changed=true
            break
        fi
    done

    if [ "$changed" = false ]; then
        return 0
    fi

    git -C "$PROJECT_ROOT" add "${WORKSPACE_VERSION_FILES[@]}"
    git -C "$PROJECT_ROOT" commit -m "$commit_message"
}

require_command git
require_command perl

if ! git -C "$PROJECT_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "Error: $PROJECT_ROOT is not a git repository" >&2
    exit 1
fi

CURRENT_BRANCH="$(git -C "$PROJECT_ROOT" branch --show-current)"
if [ -z "$CURRENT_BRANCH" ]; then
    echo "Error: Detached HEAD. Check out a branch before tagging a release." >&2
    exit 1
fi

if tracked_worktree_dirty; then
    if [ "$DRY_RUN" = true ]; then
        echo "Warning: tracked worktree is dirty; a real release would fail until you commit or stash these changes." >&2
        git -C "$PROJECT_ROOT" status --short --untracked-files=no >&2
        echo "" >&2
    else
        echo "Error: Refusing to tag from a dirty tracked worktree." >&2
        echo "Commit or stash tracked changes first." >&2
        git -C "$PROJECT_ROOT" status --short --untracked-files=no >&2
        exit 1
    fi
fi

LATEST_TAG="$(git -C "$PROJECT_ROOT" tag --list 'v[0-9]*' --sort=-version:refname | head -n 1)"
LATEST_VERSION=""
if [ -n "$LATEST_TAG" ]; then
    LATEST_VERSION="${LATEST_TAG#v}"
fi

if [ -n "$VERSION" ]; then
    VERSION="$(normalize_version "$VERSION")"
elif [ -n "$LATEST_VERSION" ]; then
    VERSION="$(bump_version "$LATEST_VERSION" "$BUMP_KIND")"
else
    VERSION="0.1.0"
fi

TAG="v$VERSION"

if [ -n "$LATEST_VERSION" ] && ! semver_gt "$VERSION" "$LATEST_VERSION"; then
    echo "Error: $TAG must be greater than the latest release tag v$LATEST_VERSION" >&2
    exit 1
fi

if git -C "$PROJECT_ROOT" rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
    echo "Error: Tag already exists locally: $TAG" >&2
    exit 1
fi

if [ -z "$MESSAGE" ]; then
    MESSAGE="Release $TAG"
fi

REPO_URL="$(github_repo_url)"
CURRENT_WORKSPACE_VERSION="$(read_workspace_version)"

echo "Release plan"
echo "  Branch:  $CURRENT_BRANCH"
echo "  Remote:  $REMOTE"
if [ -n "$LATEST_TAG" ]; then
    echo "  Previous: $LATEST_TAG"
fi
echo "  New tag: $TAG"
echo "  Message: $MESSAGE"
if [ "$CURRENT_WORKSPACE_VERSION" != "$VERSION" ]; then
    echo "  Workspace version: $CURRENT_WORKSPACE_VERSION -> $VERSION"
fi
echo ""

if [ "$DRY_RUN" = true ]; then
    if [ "$CURRENT_WORKSPACE_VERSION" != "$VERSION" ]; then
        echo "[dry-run] Would update workspace version files: Cargo.toml, Cargo.lock"
        echo "[dry-run] Would create release commit: git commit -m \"Release $TAG\""
    fi
    echo "[dry-run] Would create annotated tag: git tag -a $TAG -m \"$MESSAGE\""
    if [ "$PUSH" = true ]; then
        echo "[dry-run] Would push branch: git push $REMOTE HEAD:refs/heads/$CURRENT_BRANCH"
        echo "[dry-run] Would push tag:    git push $REMOTE refs/tags/$TAG"
    fi
    exit 0
fi

update_workspace_version_files "$VERSION" "$CURRENT_WORKSPACE_VERSION"

commit_release_version_update "$TAG"

EXACT_TAG="$(git -C "$PROJECT_ROOT" describe --tags --exact-match --match 'v[0-9]*' HEAD 2>/dev/null || true)"
if [ -n "$EXACT_TAG" ]; then
    echo "Error: HEAD is already tagged with $EXACT_TAG" >&2
    exit 1
fi

git -C "$PROJECT_ROOT" tag -a "$TAG" -m "$MESSAGE"

if [ "$PUSH" = true ]; then
    git -C "$PROJECT_ROOT" push "$REMOTE" "HEAD:refs/heads/$CURRENT_BRANCH"
    git -C "$PROJECT_ROOT" push "$REMOTE" "refs/tags/$TAG"
fi

echo ""
echo "Created $TAG at $(git -C "$PROJECT_ROOT" rev-parse --short HEAD)"

if [ "$PUSH" = true ]; then
    echo "Pushed branch and tag to $REMOTE."
    if [ -n "$REPO_URL" ]; then
        echo "GitHub Actions will publish the release assets after the release workflow finishes:"
        echo "  Actions:  $REPO_URL/actions/workflows/release.yml"
        echo "  Release:  $REPO_URL/releases/tag/$TAG"
    fi
else
    echo "Tag created locally only. Push it when ready:"
    echo "  git push $REMOTE HEAD:refs/heads/$CURRENT_BRANCH"
    echo "  git push $REMOTE refs/tags/$TAG"
fi
