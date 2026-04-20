#!/bin/bash
#
# Create a server/embedded release tag, and optionally upload the rpiz OTA feed
# locally instead of relying on GitHub Actions.
#
# Usage:
#   ./scripts/release.sh
#   ./scripts/release.sh --minor
#   ./scripts/release.sh --version 0.5.0
#   ./scripts/release.sh --upload

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

REMOTE="origin"
VERSION=""
BUMP_KIND="patch"
PUSH=true
DRY_RUN=false
UPLOAD=false
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

Create a release tag. By default this pushes the release commit and tag so the
GitHub release workflow can publish the assets. With --upload, the script keeps
the release local, builds the rpiz artifact, packages the OTA feed, and uploads
it directly using credentials loaded from .env.

Options:
  --version X.Y.Z   Use an explicit version instead of auto-bumping
  --major           Bump the latest vX.Y.Z tag to the next major version
  --minor           Bump the latest vX.Y.Z tag to the next minor version
  --patch           Bump the latest vX.Y.Z tag to the next patch version (default)
  --upload          Build/package/upload the rpiz OTA feed locally; implies --no-push
  --message TEXT    Annotated tag message (default: "Release vX.Y.Z")
  --remote NAME     Remote to push to (default: origin)
  --no-push         Create the local tag but do not push branch or tag
  --dry-run         Print the planned tag/push actions without changing git state
  -h, --help        Show this help

Examples:
  $0
  $0 --minor
  $0 --version 0.4.1
  $0 --upload
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
        --upload)
            UPLOAD=true
            PUSH=false
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

require_env_value() {
    local name="$1"
    if [ -z "${!name:-}" ]; then
        echo "Error: Required environment variable not set: $name" >&2
        exit 1
    fi
}

load_project_env() {
    local env_file="$PROJECT_ROOT/.env"

    if [ ! -f "$env_file" ]; then
        echo "Error: --upload requires $env_file" >&2
        exit 1
    fi

    echo "Loading environment from .env"
    set -a
    # shellcheck disable=SC1090
    source "$env_file"
    set +a
}

resolve_project_path() {
    local path_value="$1"

    case "$path_value" in
        /*)
            echo "$path_value"
            ;;
        ~/*)
            echo "$HOME/${path_value#~/}"
            ;;
        *)
            echo "$PROJECT_ROOT/$path_value"
            ;;
    esac
}

ensure_upload_env() {
    require_env_value RHYTHM_UPDATES_SSH_HOST
    require_env_value RHYTHM_UPDATES_SSH_USER
    require_env_value RHYTHM_UPDATES_BASE_DIR

    if [ -z "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ] && [ -z "${RHYTHM_UPDATES_SSH_KEY:-}" ]; then
        echo "Error: Set RHYTHM_UPDATES_SSH_KEY_FILE or RHYTHM_UPDATES_SSH_KEY in .env for --upload" >&2
        exit 1
    fi

    if [ -n "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ]; then
        RHYTHM_UPDATES_SSH_KEY_FILE="$(resolve_project_path "$RHYTHM_UPDATES_SSH_KEY_FILE")"
        if [ ! -f "$RHYTHM_UPDATES_SSH_KEY_FILE" ]; then
            echo "Error: RHYTHM_UPDATES_SSH_KEY_FILE does not exist: $RHYTHM_UPDATES_SSH_KEY_FILE" >&2
            exit 1
        fi
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

TEMP_RELEASE_DIR=""

cleanup_temp_release_dir() {
    if [ -n "$TEMP_RELEASE_DIR" ] && [ -d "$TEMP_RELEASE_DIR" ]; then
        rm -rf "$TEMP_RELEASE_DIR"
    fi
}

setup_upload_ssh() {
    local key_path="$TEMP_RELEASE_DIR/id_ed25519"
    local known_hosts_path="$TEMP_RELEASE_DIR/known_hosts"

    mkdir -p "$TEMP_RELEASE_DIR"

    if [ -n "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ]; then
        cp "$RHYTHM_UPDATES_SSH_KEY_FILE" "$key_path"
    else
        printf '%s\n' "$RHYTHM_UPDATES_SSH_KEY" > "$key_path"
    fi

    chmod 600 "$key_path"
    ssh-keyscan -H "$RHYTHM_UPDATES_SSH_HOST" > "$known_hosts_path"
}

upload_rpiz_feed() {
    local version="$1"
    local artifact_root="$TEMP_RELEASE_DIR/dist/bin"
    local output_dir="$PROJECT_ROOT/out/server-updates"
    local ssh_key_path="$TEMP_RELEASE_DIR/id_ed25519"
    local known_hosts_path="$TEMP_RELEASE_DIR/known_hosts"
    local package_args=()

    echo ""
    echo "=== Building rpiz release artifacts locally ==="
    "$SCRIPT_DIR/build-server.sh" --release --target rpiz

    if [ ! -d "$PROJECT_ROOT/dist/bin/rpiz" ]; then
        echo "Error: Missing dist/bin/rpiz after build" >&2
        exit 1
    fi

    echo ""
    echo "=== Staging rpiz artifacts ==="
    mkdir -p "$artifact_root"
    cp -R "$PROJECT_ROOT/dist/bin/rpiz" "$artifact_root/"

    echo ""
    echo "=== Packaging rpiz OTA feed ==="
    if [ -n "${RHYTHM_RELEASE_RPIZ_IMAGE_ROOT:-}" ]; then
        package_args+=(--image-root "$RHYTHM_RELEASE_RPIZ_IMAGE_ROOT")
    fi
    bash "$SCRIPT_DIR/package-server-updates.sh" \
        --artifact-root "$artifact_root" \
        --output-dir "$output_dir" \
        --version "$version" \
        "${package_args[@]}"

    echo ""
    echo "=== Configuring SSH upload ==="
    setup_upload_ssh

    echo ""
    echo "=== Uploading rpiz OTA feed ==="
    ssh -i "$ssh_key_path" -o UserKnownHostsFile="$known_hosts_path" \
        "$RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST" \
        "mkdir -p '$RHYTHM_UPDATES_BASE_DIR'"
    scp -i "$ssh_key_path" -o UserKnownHostsFile="$known_hosts_path" -r \
        "$output_dir/." \
        "$RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$RHYTHM_UPDATES_BASE_DIR/"

    echo ""
    echo "Uploaded rpiz OTA feed for $version to $RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$RHYTHM_UPDATES_BASE_DIR"
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

if [ "$UPLOAD" = true ]; then
    require_command ssh
    require_command scp
    require_command ssh-keyscan
    load_project_env
    ensure_upload_env
fi

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
if [ "$UPLOAD" = true ]; then
    echo "  Remote:  (disabled by --upload)"
else
    echo "  Remote:  $REMOTE"
fi
if [ -n "$LATEST_TAG" ]; then
    echo "  Previous: $LATEST_TAG"
fi
echo "  New tag: $TAG"
echo "  Message: $MESSAGE"
if [ "$CURRENT_WORKSPACE_VERSION" != "$VERSION" ]; then
    echo "  Workspace version: $CURRENT_WORKSPACE_VERSION -> $VERSION"
fi
if [ "$UPLOAD" = true ]; then
    echo "  Upload: rpiz OTA feed -> $RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$RHYTHM_UPDATES_BASE_DIR"
fi
echo ""

if [ "$DRY_RUN" = true ]; then
    if [ "$CURRENT_WORKSPACE_VERSION" != "$VERSION" ]; then
        echo "[dry-run] Would update workspace version files: Cargo.toml, Cargo.lock"
        echo "[dry-run] Would create release commit: git commit -m \"Release $TAG\""
    fi
    echo "[dry-run] Would create annotated tag: git tag -a $TAG -m \"$MESSAGE\""
    if [ "$UPLOAD" = true ]; then
        echo "[dry-run] Would build rpiz release binary: ./scripts/build-server.sh --release --target rpiz"
        echo "[dry-run] Would package rpiz OTA feed from a temporary artifact root"
        echo "[dry-run] Would upload OTA feed to $RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$RHYTHM_UPDATES_BASE_DIR"
        echo "[dry-run] Would not push branch or tag in --upload mode"
    elif [ "$PUSH" = true ]; then
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

if [ "$UPLOAD" = true ]; then
    TEMP_RELEASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-release.XXXXXX")"
    trap cleanup_temp_release_dir EXIT
    upload_rpiz_feed "$VERSION"
elif [ "$PUSH" = true ]; then
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
elif [ "$UPLOAD" = true ]; then
    echo "Built and uploaded the rpiz OTA feed locally."
    echo "Branch and tag were left local only in --upload mode."
else
    echo "Tag created locally only. Push it when ready:"
    echo "  git push $REMOTE HEAD:refs/heads/$CURRENT_BRANCH"
    echo "  git push $REMOTE refs/tags/$TAG"
fi
