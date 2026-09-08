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
#   ./tools/os/scripts/promote-stable.sh --version 0.4.261 --with-image   # force a full image build
#   ./tools/os/scripts/promote-stable.sh --dry-run
#   ./tools/os/scripts/promote-stable.sh --no-push             # create the stable tag locally only

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# shellcheck source=lib/version.sh
source "$SCRIPT_DIR/lib/version.sh"
source "$SCRIPT_DIR/lib/publication.sh"

REMOTE="origin"
VERSION=""
PUSH=true
DRY_RUN=false
MESSAGE=""
WITH_IMAGE=false
NO_IMAGE=false
IMAGE_MODE="auto"
IMAGE_MODE_EXPLICIT=false
VERIFY_BETA=true
VERIFY_DEVICE=""
VERIFY_TOKEN_FILE=""
VERIFY_SCENARIO="smoke"
VERIFY_RECEIPT=""
VERIFY_WAIT_SECONDS=0
SKIP_VERIFY_REASON=""

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
  --with-image        Force a full rootfs image build for this stable release
                      by marking the tag. Normally unnecessary: CI auto-builds
                      the image whenever the rootfs fingerprint changed.
  --no-image          Publish a binary-only stable manifest with NO image
                      entries (suppresses the image build and the image
                      carry-forward). Escape hatch for firmware whose image
                      path is broken. The next normal release re-seeds the
                      image automatically.
  --image-mode MODE   Image posture for a forced image build: auto, dev, or
                      prod (default: auto = prod posture for stable)
  --no-push           Create the stable tag locally but do not push it
  --dry-run           Print the planned tag action without changing git state
  --device URL        Verify the beta package on this rpiz before promotion
  --token-file FILE   Read the rpiz bearer token from FILE (never stored)
  --scenario NAME     Device verification: smoke, state, onboarding, or ota
                      (default: smoke)
  --receipt FILE      Beta verification receipt path (default under
                      .release-evidence/)
  --wait-seconds N    Wait up to N seconds for the public beta manifest
  --skip-beta-verification
                      Emergency escape hatch: skip the beta feed/device check
  --reason TEXT       Required explanation with --skip-beta-verification
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
        --no-push)
            PUSH=false
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        --device)
            if [ $# -lt 2 ]; then
                echo "Error: --device requires a value" >&2
                exit 1
            fi
            VERIFY_DEVICE="$2"
            shift 2
            ;;
        --token-file)
            if [ $# -lt 2 ]; then
                echo "Error: --token-file requires a value" >&2
                exit 1
            fi
            VERIFY_TOKEN_FILE="$2"
            shift 2
            ;;
        --scenario)
            if [ $# -lt 2 ]; then
                echo "Error: --scenario requires a value" >&2
                exit 1
            fi
            VERIFY_SCENARIO="$2"
            shift 2
            ;;
        --receipt)
            if [ $# -lt 2 ]; then
                echo "Error: --receipt requires a value" >&2
                exit 1
            fi
            VERIFY_RECEIPT="$2"
            shift 2
            ;;
        --wait-seconds)
            if [ $# -lt 2 ]; then
                echo "Error: --wait-seconds requires a value" >&2
                exit 1
            fi
            VERIFY_WAIT_SECONDS="$2"
            shift 2
            ;;
        --skip-beta-verification)
            VERIFY_BETA=false
            shift
            ;;
        --reason)
            if [ $# -lt 2 ]; then
                echo "Error: --reason requires a value" >&2
                exit 1
            fi
            SKIP_VERIFY_REASON="$2"
            shift 2
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

require_command git

if ! git -C "$REPO_ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "Error: $REPO_ROOT is not a git repository" >&2
    exit 1
fi

case "$IMAGE_MODE" in
    auto|dev|prod)
        ;;
    *)
        echo "Error: --image-mode must be auto, dev, or prod (got '$IMAGE_MODE')" >&2
        exit 1
        ;;
esac

if [ "$IMAGE_MODE_EXPLICIT" = true ] && [ "$WITH_IMAGE" = false ]; then
    echo "Error: --image-mode requires --with-image" >&2
    exit 1
fi

if [ "$WITH_IMAGE" = true ] && [ "$NO_IMAGE" = true ]; then
    echo "Error: --with-image and --no-image are mutually exclusive" >&2
    exit 1
fi

case "$VERIFY_SCENARIO" in
    smoke|state|onboarding|ota)
        ;;
    *)
        echo "Error: --scenario must be smoke, state, onboarding, or ota" >&2
        exit 1
        ;;
esac

if ! printf '%s' "$VERIFY_WAIT_SECONDS" | grep -Eq '^[0-9]+$'; then
    echo "Error: --wait-seconds must be a non-negative integer" >&2
    exit 1
fi

if [ "$VERIFY_BETA" = false ] && [ -z "$SKIP_VERIFY_REASON" ]; then
    echo "Error: --skip-beta-verification requires --reason TEXT" >&2
    exit 1
fi

if [ "$VERIFY_BETA" = false ] && { [ -n "$VERIFY_DEVICE" ] || [ -n "$VERIFY_TOKEN_FILE" ]; }; then
    echo "Error: device verification options cannot be used with --skip-beta-verification" >&2
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

if [ "$WITH_IMAGE" = true ]; then
    MESSAGE="$MESSAGE $(with_image_marker)"
fi

if [ "$NO_IMAGE" = true ]; then
    MESSAGE="$MESSAGE [no-image]"
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

if [ -z "$VERIFY_RECEIPT" ]; then
    VERIFY_RECEIPT="${RHYTHM_RELEASE_EVIDENCE_ROOT:-$REPO_ROOT/.release-evidence}/${SOURCE_TAG}.json"
elif [ "${VERIFY_RECEIPT#/}" = "$VERIFY_RECEIPT" ]; then
    VERIFY_RECEIPT="$REPO_ROOT/$VERIFY_RECEIPT"
fi

if [ "$PUSH" = true ]; then
    validate_release_publish_hook
fi

if [ "$VERIFY_BETA" = true ]; then
    MESSAGE="$MESSAGE [beta-verified]"
    if [ "$DRY_RUN" = false ]; then
        verify_args=(
            --version "$CORE_VERSION"
            --remote "$REMOTE"
            --scenario "$VERIFY_SCENARIO"
            --receipt "$VERIFY_RECEIPT"
            --wait-seconds "$VERIFY_WAIT_SECONDS"
        )
        if [ -n "$VERIFY_DEVICE" ]; then
            verify_args+=(--device "$VERIFY_DEVICE")
        fi
        if [ -n "$VERIFY_TOKEN_FILE" ]; then
            verify_args+=(--token-file "$VERIFY_TOKEN_FILE")
        fi
        "$SCRIPT_DIR/verify-beta-release.sh" "${verify_args[@]}"
    fi
else
    MESSAGE="$MESSAGE [beta-verification-skipped]"
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
if [ "$WITH_IMAGE" = true ]; then
    echo "  Image:   forced via tag marker $(with_image_marker) (CI builds the full image for this release)"
elif [ "$NO_IMAGE" = true ]; then
    echo "  Image:   suppressed via tag marker [no-image] (binary-only manifest, no image entries)"
else
    echo "  Image:   auto — CI builds a full image only when the rootfs fingerprint changed"
fi
if [ "$VERIFY_BETA" = true ]; then
    echo "  Verify:  public beta feed${VERIFY_DEVICE:+ + device $VERIFY_DEVICE}"
    echo "  Receipt: $VERIFY_RECEIPT"
else
    echo "  Verify:  SKIPPED — $SKIP_VERIFY_REASON"
fi
echo ""

if [ "$WITH_IMAGE" = true ] && [ -n "$LOCAL_STABLE_COMMIT" ]; then
    echo "Warning: $STABLE_TAG already exists, so its message cannot gain the $(with_image_marker) marker." >&2
    echo "Use your image publisher to rebuild $STABLE_TAG with image mode $IMAGE_MODE." >&2
fi

if [ "$NO_IMAGE" = true ] && [ -n "$LOCAL_STABLE_COMMIT" ]; then
    echo "Warning: $STABLE_TAG already exists, so its message cannot gain the [no-image] marker." >&2
fi

if [ "$DRY_RUN" = true ]; then
    if [ "$VERIFY_BETA" = true ]; then
        echo "[dry-run] Would verify $SOURCE_TAG in the public beta feed and write $VERIFY_RECEIPT"
    else
        echo "[dry-run] Would skip beta verification: $SKIP_VERIFY_REASON"
    fi
    if [ -z "$LOCAL_STABLE_COMMIT" ]; then
        echo "[dry-run] Would create annotated tag: git tag -a $STABLE_TAG $SOURCE_COMMIT -m \"$MESSAGE\""
    else
        echo "[dry-run] Local stable tag already exists at the source commit."
    fi
    if [ "$PUSH" = true ]; then
        if [ -z "$REMOTE_STABLE_COMMIT" ]; then
            echo "[dry-run] Would push stable tag: git push $REMOTE refs/tags/$STABLE_TAG"
            preview_release_publisher "$STABLE_TAG"
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
        publish_release_tag "$STABLE_TAG"
        if [ -z "${RHYTHM_RELEASE_PUBLISH_HOOK:-}" ]; then
            echo "No publisher is configured; the source tag is published, but no OTA upload was requested."
        fi
    else
        echo "Remote stable tag already exists at $SOURCE_COMMIT"
    fi
else
    echo "Tag created locally only. Push it when ready:"
    echo "  git push $REMOTE refs/tags/$STABLE_TAG"
fi
