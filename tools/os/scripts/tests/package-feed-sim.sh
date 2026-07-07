#!/bin/bash
# Local simulation of the OTA feed packaging lifecycle. No network, no builds.
#
# Exercises package-server-updates.sh through the sequence CI produces:
#   1. image release          -> fresh image entries carry the fingerprint
#   2. binary-only follow-up  -> previous image entries carried forward
#      (asserted for BOTH channels — the carry-forward is channel-symmetric)
#   3. [no-image] release     -> neither image flag -> manifest ships images: []
#   4. --dry-run              -> writes nothing
#
# Usage: ./tools/os/scripts/tests/package-feed-sim.sh

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PACKAGER="$SCRIPT_DIR/../package-server-updates.sh"

command -v jq >/dev/null 2>&1 || { echo "SKIP: jq is required"; exit 0; }

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-feed-sim.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

FAILURES=0

assert_eq() {
    local label="$1" expected="$2" actual="$3"
    if [ "$expected" = "$actual" ]; then
        echo "ok   $label"
    else
        echo "FAIL $label: expected '$expected', got '$actual'"
        FAILURES=$((FAILURES + 1))
    fi
}

# Fake artifact root: stub binaries are enough for tar + sha256.
ARTIFACT_ROOT="$WORK_DIR/dist/bin"
mkdir -p "$ARTIFACT_ROOT/rpiz"
printf 'stub rhythm-server' > "$ARTIFACT_ROOT/rpiz/rhythm-server"
printf 'stub rhythm-chipd' > "$ARTIFACT_ROOT/rpiz/rhythm-chipd"

# Fake image root.
IMAGE_ROOT="$WORK_DIR/images"
mkdir -p "$IMAGE_ROOT"
printf 'stub rootfs' | gzip > "$IMAGE_ROOT/rootfs.ext2.gz"
printf 'stub sdcard' | gzip > "$IMAGE_ROOT/sdcard.img.gz"

FINGERPRINT="v1-dev-cafe0123beef"

for channel in beta stable; do
    feed="rpiz"
    [ "$channel" = "stable" ] && feed="rpiz-stable"

    image_version="1.0.0"
    binary_version="1.0.1"
    [ "$channel" = "stable" ] && image_version="1.0.0-stable" && binary_version="1.0.1-stable"

    out_image="$WORK_DIR/out-image-$channel"
    out_binary="$WORK_DIR/out-binary-$channel"

    # --- 1. Image release: fresh entries carry the fingerprint --------------
    bash "$PACKAGER" \
        --artifact-root "$ARTIFACT_ROOT" \
        --output-dir "$out_image" \
        --version "$image_version" \
        --channel "$channel" \
        --image-root "$IMAGE_ROOT" \
        --image-fingerprint "$FINGERPRINT" >/dev/null

    manifest="$out_image/$feed/manifest.json"
    assert_eq "$channel image manifest version" "$image_version" "$(jq -r '.version' "$manifest")"
    assert_eq "$channel fresh rootfs fingerprint" "$FINGERPRINT" \
        "$(jq -r '.images[] | select(.kind == "rootfs_image") | .fingerprint' "$manifest")"
    assert_eq "$channel fresh image count" "2" "$(jq -r '.images | length' "$manifest")"

    # --- 2. Binary-only follow-up: images carried forward --------------------
    bash "$PACKAGER" \
        --artifact-root "$ARTIFACT_ROOT" \
        --output-dir "$out_binary" \
        --version "$binary_version" \
        --channel "$channel" \
        --previous-manifest "$feed=$manifest" >/dev/null

    manifest2="$out_binary/$feed/manifest.json"
    assert_eq "$channel binary manifest version" "$binary_version" "$(jq -r '.version' "$manifest2")"
    assert_eq "$channel carried rootfs version" "$image_version" \
        "$(jq -r '.images[] | select(.kind == "rootfs_image") | .version' "$manifest2")"
    assert_eq "$channel carried rootfs fingerprint" "$FINGERPRINT" \
        "$(jq -r '.images[] | select(.kind == "rootfs_image") | .fingerprint' "$manifest2")"
    assert_eq "$channel carried rootfs url" "v$image_version/rootfs.ext2.gz" \
        "$(jq -r '.images[] | select(.kind == "rootfs_image") | .url' "$manifest2")"
    assert_eq "$channel package version" "$binary_version" "$(jq -r '.package.version' "$manifest2")"

    # --- 3. [no-image] release: no image flags at all -> images: [] ----------
    # (This is what CI runs for a tag carrying the [no-image] marker: the
    # previous manifest is deliberately withheld so nothing carries forward.)
    out_noimage="$WORK_DIR/out-noimage-$channel"
    noimage_version="1.0.2"
    [ "$channel" = "stable" ] && noimage_version="1.0.2-stable"
    bash "$PACKAGER" \
        --artifact-root "$ARTIFACT_ROOT" \
        --output-dir "$out_noimage" \
        --version "$noimage_version" \
        --channel "$channel" >/dev/null

    manifest3="$out_noimage/$feed/manifest.json"
    assert_eq "$channel no-image manifest version" "$noimage_version" "$(jq -r '.version' "$manifest3")"
    assert_eq "$channel no-image image count" "0" "$(jq -r '.images | length' "$manifest3")"
    assert_eq "$channel no-image package version" "$noimage_version" "$(jq -r '.package.version' "$manifest3")"
done

# --- 3. --dry-run writes nothing ---------------------------------------------
dry_out="$WORK_DIR/out-dry"
bash "$PACKAGER" \
    --artifact-root "$ARTIFACT_ROOT" \
    --output-dir "$dry_out" \
    --version "9.9.9" \
    --channel beta \
    --dry-run >/dev/null
if [ -e "$dry_out" ]; then
    echo "FAIL --dry-run created $dry_out"
    FAILURES=$((FAILURES + 1))
else
    echo "ok   --dry-run wrote nothing"
fi

# --- 4. --image-root without --image-fingerprint is rejected ------------------
if bash "$PACKAGER" \
    --artifact-root "$ARTIFACT_ROOT" \
    --output-dir "$WORK_DIR/out-reject" \
    --version "1.0.0" \
    --image-root "$IMAGE_ROOT" >/dev/null 2>&1; then
    echo "FAIL --image-root without --image-fingerprint was accepted"
    FAILURES=$((FAILURES + 1))
else
    echo "ok   --image-root without --image-fingerprint rejected"
fi

echo ""
if [ "$FAILURES" -gt 0 ]; then
    echo "package-feed-sim: $FAILURES failure(s)"
    exit 1
fi
echo "package-feed-sim: all checks passed"
