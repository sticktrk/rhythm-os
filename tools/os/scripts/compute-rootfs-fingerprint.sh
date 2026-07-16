#!/bin/bash
# Compute the rpiz rootfs fingerprint: a content hash of everything that bakes
# into the rootfs image. The CI release gate compares this against the
# fingerprint of the newest published image in the OTA feed to decide whether
# a release can be binary-only or must chain a full Buildroot image build.
#
# Inputs hashed:
#   - hash= field of os/install/rpiz/builder-image.lock
#     (transitively covers the builder Dockerfile, CHIP rev + local diff,
#      libCHIP.a, args.gn, the ARMv6 musl toolchain, and the Buildroot
#      checkout — see tools/os/scripts/build/compute-image-hash.sh)
#   - os/install/rpiz/buildroot/**  (defconfig, fragments, rootfs overlay,
#      post-build.sh, S41bootstate, packages)
#   - tools/os/scripts/build-rpiz-image.sh (dev fragment injection, gzip step)
#   - the image posture (dev|prod) — posture changes rootfs contents
#     (dropbear, root password, /etc/default/rhythm vs rhythm-dev)
#
# Inputs deliberately NOT hashed:
#   - rust/** sources and Cargo.{toml,lock} — the shipped binaries are
#     refreshed on top of any rootfs by the OTA package overlay
#     (customize_inactive_rootfs), so they don't define the rootfs base
#   - RHYTHM_IMAGE_VERSION / RHYTHM_IMAGE_FINGERPRINT (stamped at build time)
#   - Wi-Fi embed envs (RHYTHM_WIFI_*) — bench-only convenience
#   - check-rpiz-image-mode.sh (a post-build assertion, not an input)
#
# File ordering is canonicalized to the C locale so macOS and Linux compute
# the same digest regardless of the caller's locale.
#
# Output: v2-<mode>-<12 hex chars>, e.g. v2-prod-3fa9c2d4e1b0

set -euo pipefail

# sort order is part of the fingerprint contract. Locale-sensitive collation
# previously produced different hashes for the same checkout on macOS and CI.
export LC_ALL=C

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"

IMAGE_MODE="dev"

usage() {
    cat <<EOF
Usage: $0 [--image-mode dev|prod]

Print the rpiz rootfs fingerprint for the working tree at the given image
posture (default: dev).
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --image-mode)
            if [ $# -lt 2 ]; then
                echo "Error: --image-mode requires a value" >&2
                exit 1
            fi
            IMAGE_MODE="$2"
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

case "$IMAGE_MODE" in
    dev|prod)
        ;;
    *)
        echo "Error: --image-mode must be dev or prod (got '$IMAGE_MODE')" >&2
        exit 1
        ;;
esac

# Portable sha256 over stdin (Linux sha256sum / macOS shasum).
sha_stream() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum | awk '{print $1}'
    else
        shasum -a 256 | awk '{print $1}'
    fi
}

sha_file() {
    if [ -f "$1" ]; then
        sha_stream < "$1"
    else
        echo "missing-$(basename "$1")"
    fi
}

sha_dir_tree() {
    # Deterministic directory hash: path + content, sorted.
    if [ ! -d "$1" ]; then
        echo "missing-dir"
        return
    fi
    (cd "$1" && find . -type f -print0 2>/dev/null | sort -z | while IFS= read -r -d '' path; do
        printf '%s\0' "$path"
        sha_file "$path"
    done) | sha_stream
}

builder_hash() {
    awk -F= '/^hash=/ {print $2}' "$PROJECT_ROOT/install/rpiz/builder-image.lock" 2>/dev/null \
        || echo "missing-builder-lock"
}

digest="$({
    echo "schema v2"
    echo "builder $(builder_hash)"
    echo "br-external $(sha_dir_tree "$PROJECT_ROOT/install/rpiz/buildroot")"
    echo "image-script $(sha_file "$SCRIPT_DIR/build-rpiz-image.sh")"
    echo "mode $IMAGE_MODE"
} | sha_stream | cut -c1-12)"

echo "v2-$IMAGE_MODE-$digest"
