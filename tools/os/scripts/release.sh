#!/bin/bash
#
# Create a server/appliance release tag, and optionally upload the rpiz OTA feed
# locally instead of relying on GitHub Actions.
#
# Usage:
#   ./tools/os/scripts/release.sh
#   ./tools/os/scripts/release.sh --minor
#   ./tools/os/scripts/release.sh --version 0.5.0
#   ./tools/os/scripts/release.sh --with-image
#   ./tools/os/scripts/release.sh --upload
#   ./tools/os/scripts/release.sh --promote-stable [vX.Y.Z] --with-image

set -euo pipefail

ORIGINAL_ARGS=("$@")

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# shellcheck source=lib/version.sh
source "$SCRIPT_DIR/lib/version.sh"
source "$SCRIPT_DIR/lib/publication.sh"

REMOTE="origin"
VERSION=""
BUMP_KIND="patch"
PUSH=true
DRY_RUN=false
UPLOAD=false
PROMOTE_STABLE=false
PROMOTE_STABLE_VERSION=""
SKIP_BUILDER_REFRESH=false
MESSAGE=""
WITH_IMAGE=false
NO_IMAGE=false
IMAGE_MODE="auto"
IMAGE_MODE_EXPLICIT=false
VERIFY_DEVICE=""
VERIFY_TOKEN_FILE=""
VERIFY_SCENARIO="smoke"
VERIFY_RECEIPT=""
VERIFY_WAIT_SECONDS=""
SKIP_BETA_VERIFICATION=false
SKIP_VERIFY_REASON=""
WORKSPACE_LOCK_FILES=("Cargo.lock")
WORKSPACE_VERSION_FILES=("Cargo.toml" "${WORKSPACE_LOCK_FILES[@]}" "os/install/rpiz/builder-image.lock")
BUILDER_LOCK_FILE="os/install/rpiz/builder-image.lock"
SERVER_RELEASES_TO_KEEP=2

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Create a release tag. By default this pushes the release commit and tag so the
configured publisher can publish the assets. With --upload, the script keeps
the release local, builds the rpiz artifact, packages the OTA feed, and uploads
it directly using the external release credential profile.

Options:
  --version X.Y.Z   Use an explicit base version instead of auto-bumping
  --major           Bump the latest vX.Y.Z tag to the next major version
  --minor           Bump the latest vX.Y.Z tag to the next minor version
  --patch           Bump the latest vX.Y.Z tag to the next patch version (default)
  --upload          Build/package/upload the rpiz OTA feed locally; implies --no-push
  --promote-stable [VERSION]
                    Create and push vX.Y.Z-stable from the matching
                    vX.Y.Z-beta tag so CI builds/publishes the stable feed
  --with-image      Force a full rootfs image build for this release by
                    marking the tag. Normally unnecessary: CI auto-builds the
                    image whenever the rootfs fingerprint changed.
  --no-image        Publish a binary-only manifest with NO image entries
                    (suppresses both the image build and the image
                    carry-forward). Escape hatch for firmware whose image
                    path is broken: devices then take the package-only
                    update. The next normal release re-seeds the image
                    automatically.
  --image-mode MODE Image posture for a forced image build: auto, dev, or
                    prod (default: auto = dev posture for beta, prod for
                    stable)
  --message TEXT    Annotated tag message (default: "Release vX.Y.Z-beta")
  --remote NAME     Remote to push to (default: origin)
  --no-push         Create the local tag but do not push branch or tag
  --dry-run         Print the planned tag/push actions without changing git state
  --device URL      With --promote-stable, verify this rpiz before promotion
  --token-file FILE With --promote-stable, read the rpiz bearer token from FILE
  --scenario NAME   With --promote-stable: smoke, state, onboarding, or ota
  --receipt FILE    With --promote-stable, write the verification receipt here
  --wait-seconds N  With --promote-stable, wait for the public beta manifest
  --skip-beta-verification
                    Emergency stable-promotion escape hatch; requires --reason
  --reason TEXT     Explanation for --skip-beta-verification
  --skip-builder-refresh
                    Do not invoke tools/os/scripts/build/refresh-builder-image.sh. Use
                    this when cutting a release from a non-Linux machine or
                    when you know the lock file is already correct.
  -h, --help        Show this help

Full rootfs image builds (sdcard.img.gz + rootfs.ext2.gz) are auto-detected:
CI compares the release's rootfs fingerprint against the published feed and
chains the Buildroot image build only when the rootfs inputs changed. Use
--with-image only to force an image rebuild despite an unchanged fingerprint.

Examples:
  $0
  $0 --minor
  $0 --version 0.4.1
  $0 --with-image
  $0 --upload
  $0 --promote-stable v0.4.219 --with-image
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
        --promote-stable)
            PROMOTE_STABLE=true
            if [ $# -gt 1 ] && [[ "${2:-}" != --* ]]; then
                PROMOTE_STABLE_VERSION="$2"
                shift 2
            else
                shift
            fi
            ;;
        --with-image)
            WITH_IMAGE=true
            shift
            ;;
        --no-image)
            NO_IMAGE=true
            shift
            ;;
        --image-mode)
            if [ $# -lt 2 ]; then
                echo "Error: --image-mode requires a value" >&2
                exit 1
            fi
            IMAGE_MODE="$2"
            IMAGE_MODE_EXPLICIT=true
            shift 2
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
        --device)
            VERIFY_DEVICE="${2:?--device requires a value}"
            shift 2
            ;;
        --token-file)
            VERIFY_TOKEN_FILE="${2:?--token-file requires a value}"
            shift 2
            ;;
        --scenario)
            VERIFY_SCENARIO="${2:?--scenario requires a value}"
            shift 2
            ;;
        --receipt)
            VERIFY_RECEIPT="${2:?--receipt requires a value}"
            shift 2
            ;;
        --wait-seconds)
            VERIFY_WAIT_SECONDS="${2:?--wait-seconds requires a value}"
            shift 2
            ;;
        --skip-beta-verification)
            SKIP_BETA_VERIFICATION=true
            shift
            ;;
        --reason)
            SKIP_VERIFY_REASON="${2:?--reason requires a value}"
            shift 2
            ;;
        --skip-builder-refresh)
            SKIP_BUILDER_REFRESH=true
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
    local config_dir="${RHYTHM_CONFIG_DIR:-$HOME/.config/rhythm}"
    local env_file="${RHYTHM_RELEASE_ENV_FILE:-$config_dir/release.env}"

    if [ "${RHYTHM_RELEASE_PROFILE_LOADED:-}" != "1" ]; then
        exec env \
            RHYTHM_RELEASE_ENV_FILE="$env_file" \
            RHYTHM_RELEASE_PROFILE_LOADED=1 \
            python3 "$REPO_ROOT/tools/config/run.py" --profile release -- \
                "$REPO_ROOT/tools/os/scripts/release.sh" "${ORIGINAL_ARGS[@]}"
    fi
}

ensure_upload_env() {
    require_env_value CLOUDFLARE_ACCESS_KEY
    require_env_value CLOUDFLARE_SECRET_ACCESS_KEY
    require_env_value CLOUDFLARE_S3_API_ENDPOINT
    require_env_value CLOUDFLARE_R2_BUCKET
}

normalize_release_version() {
    local value="$1"
    value="${value#v}"
    if ! printf '%s' "$value" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+(-beta)?$'; then
        echo "Error: Version must be semver X.Y.Z, X.Y.Z-beta, vX.Y.Z, or vX.Y.Z-beta" >&2
        exit 1
    fi
    release_version "$value"
}

tracked_worktree_dirty() {
    [ -n "$(git -C "$REPO_ROOT" status --short --untracked-files=no)" ]
}

github_repo_url() {
    local remote_url
    remote_url="$(git -C "$REPO_ROOT" remote get-url "$REMOTE" 2>/dev/null || true)"

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

# Tag-message marker consumed by the CI release gate. Normally CI decides
# binary-only vs full-image from the rootfs fingerprint; the marker forces an
# image build for this release.
with_image_marker() {
    if [ "$IMAGE_MODE" = "auto" ]; then
        echo "[with-image]"
    else
        echo "[with-image image_mode=$IMAGE_MODE]"
    fi
}

TEMP_RELEASE_DIR=""

cleanup_temp_release_dir() {
    if [ -n "$TEMP_RELEASE_DIR" ] && [ -d "$TEMP_RELEASE_DIR" ]; then
        rm -rf "$TEMP_RELEASE_DIR"
    fi
}

# Fetch the currently published manifest for a feed from the public CDN URL —
# the same URL appliances poll, so no publish credentials are needed.
fetch_rpiz_manifest() {
    local feed="$1"
    local base_url="${RHYTHM_UPDATES_PUBLIC_BASE_URL:-https://dl.rhythm.lighting/server}"
    local previous_manifest="$TEMP_RELEASE_DIR/$feed-manifest.json"

    curl -fsSL --max-time 30 "$base_url/$feed/manifest.json" \
        -o "$previous_manifest" 2>/dev/null || true

    if [ -s "$previous_manifest" ]; then
        echo "$feed=$previous_manifest"
    fi
}

upload_rpiz_feed() {
    local version="$1"
    local artifact_root="$TEMP_RELEASE_DIR/dist/bin"
    local output_dir="$PROJECT_ROOT/out/server-updates"
    local package_args=()
    local release_channel release_feed
    local manifest_spec=""

    release_channel="$(channel_for_version "$version")"
    release_feed="$(feed_for_channel "$release_channel")"

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

    # Both channels carry the current base image forward in their manifests,
    # so always offer the previous manifest to the packager — unless this is
    # a --no-image release, whose manifest must ship without image entries.
    if [ "$NO_IMAGE" = false ]; then
        manifest_spec="$(fetch_rpiz_manifest "$release_feed")"
        if [ -n "$manifest_spec" ]; then
            package_args+=(--previous-rpiz-manifest "$manifest_spec")
        fi
    fi

    echo ""
    echo "=== Packaging rpiz OTA feed ==="
    if [ "$NO_IMAGE" = true ] && [ -n "${RHYTHM_RELEASE_RPIZ_IMAGE_ROOT:-}" ]; then
        echo "Error: --no-image conflicts with RHYTHM_RELEASE_RPIZ_IMAGE_ROOT" >&2
        exit 1
    fi
    if [ -n "${RHYTHM_RELEASE_RPIZ_IMAGE_ROOT:-}" ]; then
        local image_mode image_fingerprint
        image_mode="dev"
        [ "$release_channel" = "stable" ] && image_mode="prod"
        image_fingerprint="${RHYTHM_RELEASE_RPIZ_IMAGE_FINGERPRINT:-$("$SCRIPT_DIR/compute-rootfs-fingerprint.sh" --image-mode "$image_mode")}"
        package_args+=(--image-root "$RHYTHM_RELEASE_RPIZ_IMAGE_ROOT" --image-fingerprint "$image_fingerprint")
    fi
    bash "$SCRIPT_DIR/package-server-updates.sh" \
        --artifact-root "$artifact_root" \
        --output-dir "$output_dir" \
        --version "$version" \
        "${package_args[@]}"

    echo ""
    echo "=== Uploading rpiz OTA feed to Cloudflare R2 ==="
    AWS_ACCESS_KEY_ID="$CLOUDFLARE_ACCESS_KEY" \
    AWS_SECRET_ACCESS_KEY="$CLOUDFLARE_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION=auto \
    AWS_PAGER='' \
    CLOUDFLARE_S3_API_ENDPOINT="$CLOUDFLARE_S3_API_ENDPOINT" \
    CLOUDFLARE_R2_BUCKET="$CLOUDFLARE_R2_BUCKET" \
        "$SCRIPT_DIR/publish-server-updates-r2.sh" --source-dir "$output_dir"

    echo ""
    echo "=== Pruning old Cloudflare R2 releases ==="
    AWS_ACCESS_KEY_ID="$CLOUDFLARE_ACCESS_KEY" \
    AWS_SECRET_ACCESS_KEY="$CLOUDFLARE_SECRET_ACCESS_KEY" \
    AWS_DEFAULT_REGION=auto \
    AWS_PAGER='' \
    CLOUDFLARE_S3_API_ENDPOINT="$CLOUDFLARE_S3_API_ENDPOINT" \
    CLOUDFLARE_R2_BUCKET="$CLOUDFLARE_R2_BUCKET" \
        "$SCRIPT_DIR/prune-r2-releases.sh" --keep "$SERVER_RELEASES_TO_KEEP"

    echo ""
    echo "Uploaded rpiz OTA feed for $version to Cloudflare R2 bucket $CLOUDFLARE_R2_BUCKET"
}

update_workspace_version_files() {
    local new_version="$1"
    local current_version="$2"
    local lock_file

    if [ "$current_version" != "$new_version" ]; then
        NEW_VERSION="$new_version" perl -0pi -e '
            s/(\[workspace\.package\]\n(?:[^\[]*\n)*?version = ")[^"]+(")/$1.$ENV{NEW_VERSION}.$2/se
        ' "$REPO_ROOT/Cargo.toml"
    fi

    for lock_file in "${WORKSPACE_LOCK_FILES[@]}"; do
        if [ ! -f "$REPO_ROOT/$lock_file" ]; then
            continue
        fi

        NEW_VERSION="$new_version" perl -0pi -e '
            s{(\[\[package\]\]\n.*?)(?=\n\[\[package\]\]\n|\z)}{
                my $block = $1;
                if ($block =~ /^name = "rhythm-[^"]+"$/m && $block !~ /^source = /m) {
                    $block =~ s/^version = "[^"]+"/version = "$ENV{NEW_VERSION}"/m;
                }
                $block;
            }gse;
        ' "$REPO_ROOT/$lock_file"
    done
}

commit_release_version_update() {
    local tag="$1"
    local commit_message="Release $tag"
    local file
    local changed=false

    for file in "${WORKSPACE_VERSION_FILES[@]}"; do
        if ! git -C "$REPO_ROOT" diff --quiet -- "$file"; then
            changed=true
            break
        fi
    done

    if [ "$changed" = false ]; then
        return 0
    fi

    git -C "$REPO_ROOT" add "${WORKSPACE_VERSION_FILES[@]}"
    git -C "$REPO_ROOT" commit -m "$commit_message"
}

require_command git
require_command perl
if [ "$PUSH" = true ]; then
    validate_release_publish_hook
fi

case "$IMAGE_MODE" in
    auto|dev|prod)
        ;;
    *)
        echo "Error: --image-mode must be auto, dev, or prod (got '$IMAGE_MODE')" >&2
        exit 1
        ;;
esac

if [ "$WITH_IMAGE" = true ] && [ "$NO_IMAGE" = true ]; then
    echo "Error: --with-image and --no-image are mutually exclusive" >&2
    exit 1
fi

if [ "$IMAGE_MODE_EXPLICIT" = true ] && [ "$WITH_IMAGE" = false ]; then
    echo "Error: --image-mode requires --with-image" >&2
    exit 1
fi

if [ "$PROMOTE_STABLE" = true ]; then
    if [ "$UPLOAD" = true ]; then
        echo "Error: --promote-stable cannot be combined with --upload" >&2
        exit 1
    fi

    promote_version="${PROMOTE_STABLE_VERSION:-$VERSION}"
    promote_args=()
    if [ "$DRY_RUN" = true ]; then
        promote_args+=(--dry-run)
    fi
    if [ "$PUSH" = false ]; then
        promote_args+=(--no-push)
    fi
    if [ "$REMOTE" != "origin" ]; then
        promote_args+=(--remote "$REMOTE")
    fi
    if [ -n "$MESSAGE" ]; then
        promote_args+=(--message "$MESSAGE")
    fi
    if [ "$WITH_IMAGE" = true ]; then
        promote_args+=(--with-image)
    fi
    if [ "$NO_IMAGE" = true ]; then
        promote_args+=(--no-image)
    fi
    if [ "$IMAGE_MODE" != "auto" ]; then
        promote_args+=(--image-mode "$IMAGE_MODE")
    fi
    if [ -n "$promote_version" ]; then
        promote_args+=(--version "$promote_version")
    fi
    if [ -n "$VERIFY_DEVICE" ]; then
        promote_args+=(--device "$VERIFY_DEVICE")
    fi
    if [ -n "$VERIFY_TOKEN_FILE" ]; then
        promote_args+=(--token-file "$VERIFY_TOKEN_FILE")
    fi
    if [ "$VERIFY_SCENARIO" != "smoke" ]; then
        promote_args+=(--scenario "$VERIFY_SCENARIO")
    fi
    if [ -n "$VERIFY_RECEIPT" ]; then
        promote_args+=(--receipt "$VERIFY_RECEIPT")
    fi
    if [ -n "$VERIFY_WAIT_SECONDS" ]; then
        promote_args+=(--wait-seconds "$VERIFY_WAIT_SECONDS")
    fi
    if [ "$SKIP_BETA_VERIFICATION" = true ]; then
        promote_args+=(--skip-beta-verification)
    fi
    if [ -n "$SKIP_VERIFY_REASON" ]; then
        promote_args+=(--reason "$SKIP_VERIFY_REASON")
    fi

    # ${arr[@]+...} expansion: bash 3.2 (macOS) errors on "${arr[@]}" when the
    # array is empty under set -u.
    exec "$SCRIPT_DIR/promote-stable.sh" ${promote_args[@]+"${promote_args[@]}"}
fi

if [ -n "$VERIFY_DEVICE" ] \
    || [ -n "$VERIFY_TOKEN_FILE" ] \
    || [ "$VERIFY_SCENARIO" != "smoke" ] \
    || [ -n "$VERIFY_RECEIPT" ] \
    || [ -n "$VERIFY_WAIT_SECONDS" ] \
    || [ "$SKIP_BETA_VERIFICATION" = true ] \
    || [ -n "$SKIP_VERIFY_REASON" ]; then
    echo "Error: beta verification options are only valid with --promote-stable" >&2
    exit 1
fi

if [ "$WITH_IMAGE" = true ] && [ "$UPLOAD" = true ]; then
    echo "Error: --with-image marks the release tag for the CI image build; it has no effect in --upload mode." >&2
    echo "For a local image publish, set RHYTHM_RELEASE_RPIZ_IMAGE_ROOT to a built image directory instead." >&2
    exit 1
fi

if [ "$UPLOAD" = true ]; then
    require_command aws
    require_command jq
    load_project_env
    ensure_upload_env
fi

if ! git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "Error: $REPO_ROOT is not a git repository" >&2
    exit 1
fi

CURRENT_BRANCH="$(git -C "$REPO_ROOT" branch --show-current)"
if [ -z "$CURRENT_BRANCH" ]; then
    echo "Error: Detached HEAD. Check out a branch before tagging a release." >&2
    exit 1
fi

if tracked_worktree_dirty; then
    if [ "$DRY_RUN" = true ]; then
        echo "Warning: tracked worktree is dirty; a real release would fail until you commit or stash these changes." >&2
        git -C "$REPO_ROOT" status --short --untracked-files=no >&2
        echo "" >&2
    else
        echo "Error: Refusing to tag from a dirty tracked worktree." >&2
        echo "Commit or stash tracked changes first." >&2
        git -C "$REPO_ROOT" status --short --untracked-files=no >&2
        exit 1
    fi
fi

CURRENT_WORKSPACE_VERSION="$(read_workspace_version)"

LATEST_TAG="$(git -C "$REPO_ROOT" tag --list 'v[0-9]*' --sort=-version:refname | head -n 1)"
LATEST_VERSION=""
if [ -n "$LATEST_TAG" ]; then
    LATEST_VERSION="${LATEST_TAG#v}"
fi

if [ -n "$VERSION" ]; then
    VERSION="$(normalize_release_version "$VERSION")"
elif [ -n "$CURRENT_WORKSPACE_VERSION" ]; then
    WORKSPACE_RELEASE_VERSION="$(normalize_release_version "$CURRENT_WORKSPACE_VERSION")"
    if [ -n "$LATEST_VERSION" ]; then
        TAG_BUMP_VERSION="$(normalize_release_version "$(bump_version "$LATEST_VERSION" "$BUMP_KIND")")"
        if semver_gt "$WORKSPACE_RELEASE_VERSION" "$TAG_BUMP_VERSION"; then
            VERSION="$WORKSPACE_RELEASE_VERSION"
        else
            VERSION="$TAG_BUMP_VERSION"
        fi
    else
        VERSION="$WORKSPACE_RELEASE_VERSION"
    fi
elif [ -n "$LATEST_VERSION" ]; then
    VERSION="$(normalize_release_version "$(bump_version "$LATEST_VERSION" "$BUMP_KIND")")"
else
    VERSION="$(normalize_release_version "0.1.0")"
fi

TAG="v$VERSION"

if [ -n "$LATEST_VERSION" ] && ! semver_gt "$VERSION" "$LATEST_VERSION"; then
    echo "Error: $TAG must be greater than the latest release tag v$LATEST_VERSION" >&2
    exit 1
fi

if git -C "$REPO_ROOT" rev-parse -q --verify "refs/tags/$TAG" >/dev/null 2>&1; then
    echo "Error: Tag already exists locally: $TAG" >&2
    exit 1
fi

if [ -z "$MESSAGE" ]; then
    MESSAGE="Release $TAG"
fi

if [ "$WITH_IMAGE" = true ]; then
    MESSAGE="$MESSAGE $(with_image_marker)"
fi

if [ "$NO_IMAGE" = true ]; then
    MESSAGE="$MESSAGE [no-image]"
fi

REPO_URL="$(github_repo_url)"

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
if [ "$SKIP_BUILDER_REFRESH" = true ]; then
    echo "  Builder image: refresh skipped (--skip-builder-refresh)"
elif [ "$(uname -s)" != "Linux" ]; then
    echo "  Builder image: refresh skipped (not on Linux; image packaging needs it)"
    SKIP_BUILDER_REFRESH=true
else
    echo "  Builder image: will refresh dtconcepts/rhythm-rpiz-builder if inputs changed"
fi
if [ "$WITH_IMAGE" = true ]; then
    echo "  rpiz sd image: forced via tag marker $(with_image_marker) (CI builds the full image for this release)"
elif [ "$NO_IMAGE" = true ]; then
    echo "  rpiz sd image: suppressed via tag marker [no-image] (binary-only manifest, no image entries)"
else
    echo "  rpiz sd image: auto — CI builds a full image only when the rootfs fingerprint changed"
fi
if [ "$UPLOAD" = true ]; then
    echo "  Upload: rpiz OTA feed -> Cloudflare R2 bucket $CLOUDFLARE_R2_BUCKET/server"
    echo "  Retention: keep the latest $SERVER_RELEASES_TO_KEEP server release(s)"
fi
echo ""

if [ "$DRY_RUN" = true ]; then
    if [ "$SKIP_BUILDER_REFRESH" = false ]; then
        echo "[dry-run] Would run: tools/os/scripts/build/refresh-builder-image.sh --push"
    fi
    if [ "$CURRENT_WORKSPACE_VERSION" != "$VERSION" ]; then
        echo "[dry-run] Would update workspace version files"
    fi
    echo "[dry-run] Would commit any changes to: ${WORKSPACE_VERSION_FILES[*]}"
    echo "[dry-run] Would create annotated tag: git tag -a $TAG -m \"$MESSAGE\""
    if [ "$UPLOAD" = true ]; then
        echo "[dry-run] Would build rpiz release binary: ./tools/os/scripts/build-server.sh --release --target rpiz"
        echo "[dry-run] Would package rpiz OTA feed from a temporary artifact root"
        echo "[dry-run] Would upload OTA feed to Cloudflare R2 bucket $CLOUDFLARE_R2_BUCKET/server"
        echo "[dry-run] Would prune old R2 releases, keeping the latest $SERVER_RELEASES_TO_KEEP per release root"
        echo "[dry-run] Would not push branch or tag in --upload mode"
    elif [ "$PUSH" = true ]; then
        echo "[dry-run] Would push branch: git push $REMOTE HEAD:refs/heads/$CURRENT_BRANCH"
        echo "[dry-run] Would push tag:    git push $REMOTE refs/tags/$TAG"
        preview_release_publisher "$TAG"
    fi
    exit 0
fi

if [ "$SKIP_BUILDER_REFRESH" = false ]; then
    echo "Refreshing builder image ..."
    "$SCRIPT_DIR/build/refresh-builder-image.sh" --push
fi

update_workspace_version_files "$VERSION" "$CURRENT_WORKSPACE_VERSION"

commit_release_version_update "$TAG"

EXACT_TAG="$(git -C "$REPO_ROOT" describe --tags --exact-match --match 'v[0-9]*' HEAD 2>/dev/null || true)"
if [ -n "$EXACT_TAG" ]; then
    echo "Error: HEAD is already tagged with $EXACT_TAG" >&2
    exit 1
fi

git -C "$REPO_ROOT" tag -a "$TAG" -m "$MESSAGE"

if [ "$UPLOAD" = true ]; then
    TEMP_RELEASE_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-release.XXXXXX")"
    trap cleanup_temp_release_dir EXIT
    upload_rpiz_feed "$VERSION"
elif [ "$PUSH" = true ]; then
    git -C "$REPO_ROOT" push "$REMOTE" "HEAD:refs/heads/$CURRENT_BRANCH"
    git -C "$REPO_ROOT" push "$REMOTE" "refs/tags/$TAG"
    publish_release_tag "$TAG"
fi

echo ""
echo "Created $TAG at $(git -C "$REPO_ROOT" rev-parse --short HEAD)"

if [ "$PUSH" = true ]; then
    echo "Pushed branch and tag to $REMOTE."
    if [ -n "$REPO_URL" ]; then
        echo "Source: $REPO_URL/tree/$TAG"
    fi
    if [ -z "${RHYTHM_RELEASE_PUBLISH_HOOK:-}" ]; then
        echo "No publisher is configured; the source tag is published, but no OTA upload was requested."
    fi
elif [ "$UPLOAD" = true ]; then
    echo "Built and uploaded the rpiz OTA feed locally."
    echo "Branch and tag were left local only in --upload mode."
else
    echo "Tag created locally only. Push it when ready:"
    echo "  git push $REMOTE HEAD:refs/heads/$CURRENT_BRANCH"
    echo "  git push $REMOTE refs/tags/$TAG"
fi
