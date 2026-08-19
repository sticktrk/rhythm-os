#!/bin/bash
# Check if the builder image hash derived from the current slow-path inputs is
# already published on Docker Hub. Build + push only if it isn't, then write
# install/rpiz/builder-image.lock so both CI and `build-rpiz-image.sh --docker`
# pin to the exact tag. On --push, also prunes older $TAG_PREFIX* tags on
# Docker Hub so only the 2 newest remain (set RHYTHM_RPIZ_BUILDER_KEEP to
# override). Pruning needs DOCKERHUB_USERNAME + DOCKERHUB_TOKEN in the env;
# missing creds just skip with a warning.
#
# Usage:
#   ./tools/os/scripts/build/refresh-builder-image.sh            # dry-run: check only, no push
#   ./tools/os/scripts/build/refresh-builder-image.sh --push     # build + push if missing + prune
#   ./tools/os/scripts/build/refresh-builder-image.sh --force    # always rebuild/push
#   ./tools/os/scripts/build/refresh-builder-image.sh --no-prune # --push without cleanup
#
# Exit codes:
#   0 = lock is up-to-date, no push needed
#   1 = error (missing inputs, network, etc.)
#   2 = push needed but --push not supplied (dry-run signal)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../../os" && pwd)"
LOCK_FILE="$PROJECT_ROOT/install/rpiz/builder-image.lock"
IMAGE_REPO="${RHYTHM_RPIZ_BUILDER_REPO:-dtconcepts/rhythm-rpiz-builder}"
TAG_PREFIX="${RHYTHM_RPIZ_BUILDER_TAG_PREFIX:-v1-}"
KEEP_TAGS="${RHYTHM_RPIZ_BUILDER_KEEP:-2}"

PUSH=false
FORCE=false
PRUNE=true

while [[ $# -gt 0 ]]; do
    case "$1" in
        --push)     PUSH=true; shift ;;
        --force)    FORCE=true; PUSH=true; shift ;;
        --no-prune) PRUNE=false; shift ;;
        -h|--help)
            sed -n '2,20p' "$0"
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            exit 1
            ;;
    esac
done

for cmd in curl docker python3 sha256sum; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: $cmd is required" >&2
        exit 1
    fi
done

hash="$("$SCRIPT_DIR/compute-image-hash.sh")"
tag="${TAG_PREFIX}${hash}"
image_ref="$IMAGE_REPO:$tag"
# Human-readable connectedhomeip pin (informational, not hashed).
chip_info="$("$SCRIPT_DIR/compute-image-hash.sh" --chip-info)"

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

# Write / update the lock file so callers pin to the exact tag. The baked
# output prefix is NOT recorded here (it is a machine-specific absolute path);
# in-container consumers read the /opt/rpiz-out/.rhythm-baked-prefix sidecar
# baked by build-rpiz-builder-image.sh, and RHYTHM_BAKED_OUTPUT_PREFIX remains
# a manual override.
current_ref=""
current_chip_info=""
if [ -f "$LOCK_FILE" ]; then
    current_ref="$(awk -F= '/^image=/ {print $2}' "$LOCK_FILE" 2>/dev/null || true)"
    current_chip_info="$(grep -E '^chip_(rev|ref|diff)=' "$LOCK_FILE" 2>/dev/null || true)"
fi

if [ "$current_ref" != "$image_ref" ] || [ "$current_chip_info" != "$chip_info" ]; then
    mkdir -p "$(dirname "$LOCK_FILE")"
    {
        echo "# Managed by tools/os/scripts/build/refresh-builder-image.sh — do not edit by hand."
        echo "# Bump by rerunning the refresh script after changing any baked input."
        echo "image=$image_ref"
        echo "hash=$hash"
        echo "# connectedhomeip baked into this image (informational; the SHA and"
        echo "# local diff are already part of hash= above). chip_ref is git-describe"
        echo "# output: pin the SDK to a release tag so it reads as a plain version."
        echo "$chip_info"
    } > "$LOCK_FILE"
    echo "Updated $LOCK_FILE -> $image_ref"
else
    echo "Lock file already pinned to $image_ref"
fi

# Prune older $TAG_PREFIX* tags so only the N newest remain on Docker Hub.
# Runs on every --push so it's idempotent: invariant is "≤ $KEEP_TAGS tags
# with our prefix exist in the repo". Skip cleanly if creds aren't set.
prune_old_tags() {
    local username password hub_token list_json tags_to_delete status
    username="${DOCKERHUB_USERNAME:-${DOCKER_USERNAME:-}}"
    password="${DOCKERHUB_TOKEN:-${DOCKERHUB_PASSWORD:-${DOCKER_PASSWORD:-}}}"

    if [ -z "$username" ] || [ -z "$password" ]; then
        echo "Prune: DOCKERHUB_USERNAME + DOCKERHUB_TOKEN not set — skipping cleanup." >&2
        return 0
    fi

    local login_body
    login_body="$(U="$username" P="$password" python3 -c 'import json,os,sys; json.dump({"username": os.environ["U"], "password": os.environ["P"]}, sys.stdout)')"
    hub_token="$(curl -fsSL \
        -H 'Content-Type: application/json' \
        -d "$login_body" \
        https://hub.docker.com/v2/users/login/ \
        | python3 -c 'import sys,json; print(json.load(sys.stdin).get("token",""))')" || true
    if [ -z "$hub_token" ]; then
        echo "Prune: failed to obtain Docker Hub API token (bad creds?). Skipping." >&2
        return 0
    fi

    list_json="$(curl -fsSL \
        -H "Authorization: JWT $hub_token" \
        "https://hub.docker.com/v2/repositories/$IMAGE_REPO/tags/?page_size=100")"

    tags_to_delete="$(printf '%s' "$list_json" | \
        KEEP="$KEEP_TAGS" PREFIX="$TAG_PREFIX" python3 -c '
import json, os, sys
data = json.load(sys.stdin)
prefix = os.environ["PREFIX"]
keep = int(os.environ["KEEP"])
matching = [t for t in data.get("results", []) if t.get("name", "").startswith(prefix)]
matching.sort(key=lambda t: t.get("last_updated", ""), reverse=True)
for t in matching[keep:]:
    print(t["name"])
')"

    if [ -z "$tags_to_delete" ]; then
        echo "Prune: ≤ $KEEP_TAGS $TAG_PREFIX* tags on Docker Hub; nothing to delete."
        return 0
    fi

    while IFS= read -r old_tag; do
        [ -z "$old_tag" ] && continue
        status="$(curl -s -o /dev/null -w '%{http_code}' -X DELETE \
            -H "Authorization: JWT $hub_token" \
            "https://hub.docker.com/v2/repositories/$IMAGE_REPO/tags/$old_tag/")"
        if [ "$status" = "204" ] || [ "$status" = "200" ]; then
            echo "Pruned $IMAGE_REPO:$old_tag"
        else
            echo "Prune: delete of $IMAGE_REPO:$old_tag failed (HTTP $status)" >&2
        fi
    done <<< "$tags_to_delete"
}

if [ "$PUSH" = true ] && [ "$PRUNE" = true ]; then
    prune_old_tags
fi
