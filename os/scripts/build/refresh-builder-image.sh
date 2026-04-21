#!/bin/bash
# Check if the builder image hash derived from the current slow-path inputs is
# already published on Docker Hub. Build + push only if it isn't, then write
# install/rpiz/builder-image.lock so both CI and `build-rpiz-image.sh --docker`
# pin to the exact tag.
#
# Usage:
#   ./scripts/build/refresh-builder-image.sh            # dry-run: check only, no push
#   ./scripts/build/refresh-builder-image.sh --push     # build + push if missing
#   ./scripts/build/refresh-builder-image.sh --force    # always rebuild/push
#
# Exit codes:
#   0 = lock is up-to-date, no push needed
#   1 = error (missing inputs, network, etc.)
#   2 = push needed but --push not supplied (dry-run signal)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
LOCK_FILE="$PROJECT_ROOT/install/rpiz/builder-image.lock"
IMAGE_REPO="${RHYTHM_RPIZ_BUILDER_REPO:-dtconcepts/rhythm-rpiz-builder}"
TAG_PREFIX="${RHYTHM_RPIZ_BUILDER_TAG_PREFIX:-v1-}"

PUSH=false
FORCE=false

while [[ $# -gt 0 ]]; do
    case "$1" in
        --push)  PUSH=true; shift ;;
        --force) FORCE=true; PUSH=true; shift ;;
        -h|--help)
            sed -n '2,16p' "$0"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

for cmd in curl docker sha256sum; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: $cmd is required" >&2
        exit 1
    fi
done

hash="$("$SCRIPT_DIR/compute-image-hash.sh")"
tag="${TAG_PREFIX}${hash}"
image_ref="$IMAGE_REPO:$tag"

echo "Input hash: $hash"
echo "Target image: $image_ref"

tag_exists() {
    # Public Docker Hub: anonymous pull token + HEAD on the manifest endpoint.
    local token manifest_status
    token="$(curl -fsSL \
        "https://auth.docker.io/token?service=registry.docker.io&scope=repository:$IMAGE_REPO:pull" \
        | sed -E 's/.*"token":"([^"]+)".*/\1/')"
    if [ -z "$token" ]; then
        return 2
    fi
    manifest_status="$(curl -s -o /dev/null -w '%{http_code}' \
        -H "Authorization: Bearer $token" \
        -H 'Accept: application/vnd.oci.image.index.v1+json' \
        -H 'Accept: application/vnd.oci.image.manifest.v1+json' \
        -H 'Accept: application/vnd.docker.distribution.manifest.list.v2+json' \
        -H 'Accept: application/vnd.docker.distribution.manifest.v2+json' \
        "https://registry-1.docker.io/v2/$IMAGE_REPO/manifests/$1")"
    case "$manifest_status" in
        200) return 0 ;;
        404) return 1 ;;
        *)   echo "Unexpected HTTP $manifest_status from Docker Hub" >&2; return 2 ;;
    esac
}

need_build=true
if [ "$FORCE" = false ]; then
    if tag_exists "$tag"; then
        echo "Image $image_ref already exists on Docker Hub."
        need_build=false
    else
        echo "Image $image_ref not present on Docker Hub."
    fi
fi

if [ "$need_build" = true ]; then
    if [ "$PUSH" != true ]; then
        echo "Build + push required. Re-run with --push to proceed." >&2
        exit 2
    fi
    echo "Building and pushing $image_ref ..."
    "$SCRIPT_DIR/build-rpiz-builder-image.sh" --image-tag "$image_ref" --push
fi

# The baked output prefix (absolute path the baked Buildroot output was built
# against) is recorded in the lock file so scripts/build-rpiz-image.sh --docker
# can forward it as RHYTHM_BAKED_OUTPUT_PREFIX without the user having to set
# anything. Value comes from the same env overrides the packager honors.
baked_prefix_src="${RHYTHM_RPIZ_OUT_DIR:-$PROJECT_ROOT/out/rpiz}"
if [ -d "$baked_prefix_src" ]; then
    baked_prefix="$(cd "$baked_prefix_src" && pwd)"
else
    baked_prefix=""
fi

# Write / update the lock file so callers pin to the exact tag + know the
# absolute prefix the baked Buildroot output was built against.
current_ref=""
current_baked_prefix=""
if [ -f "$LOCK_FILE" ]; then
    current_ref="$(awk -F= '/^image=/ {print $2}' "$LOCK_FILE" 2>/dev/null || true)"
    current_baked_prefix="$(awk -F= '/^baked_prefix=/ {print $2}' "$LOCK_FILE" 2>/dev/null || true)"
fi

if [ "$current_ref" != "$image_ref" ] || [ "$current_baked_prefix" != "$baked_prefix" ]; then
    mkdir -p "$(dirname "$LOCK_FILE")"
    {
        echo "# Managed by scripts/build/refresh-builder-image.sh — do not edit by hand."
        echo "# Bump by rerunning the refresh script after changing any baked input."
        echo "image=$image_ref"
        echo "hash=$hash"
        if [ -n "$baked_prefix" ]; then
            echo "baked_prefix=$baked_prefix"
        fi
    } > "$LOCK_FILE"
    echo "Updated $LOCK_FILE -> $image_ref"
else
    echo "Lock file already pinned to $image_ref"
fi
