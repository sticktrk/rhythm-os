#!/bin/bash
# Package the current Linux host's rpiz prerequisites into the builder image
# dtconcepts/rhythm-rpiz-builder:latest and (optionally) push it to Docker Hub.
#
# Inputs (resolved relative to PROJECT_ROOT unless an env override is set):
#   connectedhomeip/                        - CHIP source + out/rpiz-arm-musl
#   ~/x-tools/arm-unknown-linux-musleabihf  - ARMv6 musl cross toolchain
#   buildroot/                              - Buildroot checkout
#   out/rpiz/host/                          - Buildroot's built sysroot/host
#
# Usage:
#   ./scripts/build/build-rpiz-builder-image.sh              # build only
#   ./scripts/build/build-rpiz-builder-image.sh --push       # build + push
#   ./scripts/build/build-rpiz-builder-image.sh --image-tag dtconcepts/rhythm-rpiz-builder:v2

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DOCKERFILE="$SCRIPT_DIR/Dockerfile.rpiz-builder"
IMAGE_TAG="${RHYTHM_RPIZ_BUILDER_IMAGE:-dtconcepts/rhythm-rpiz-builder:latest}"
NO_BUILD_CACHE=false
PUSH=false

CHIP_SRC="${RHYTHM_CHIP_SRC_DIR:-$PROJECT_ROOT/connectedhomeip}"
TOOLCHAIN_SRC="${RHYTHM_X_TOOLS_DIR:-$HOME/x-tools/arm-unknown-linux-musleabihf}"
BUILDROOT_SRC="${RHYTHM_BUILDROOT_DIR:-$PROJECT_ROOT/buildroot}"
RPIZ_OUT_SRC="${RHYTHM_RPIZ_OUT_DIR:-$PROJECT_ROOT/out/rpiz}"

# third_party vendor subdirs to drop from the chip source copy. These are
# microcontroller vendor SDKs and non-Linux transports that the rpiz Rust
# bridge compilation does not pull in. If you hit a missing-header error at
# build time, remove the offending entry here (or set RHYTHM_CHIP_PRUNE_EXTRA
# to an additional list) and rebuild the image.
CHIP_PRUNE_DEFAULT=(
    amazon-kinesis-video-streams-webrtc-sdk-c
    ameba
    android_deps
    asr
    bouffalolab
    cirque
    freertos
    imx-secure-enclave
    infineon
    java_deps
    jlink
    libdatachannel
    libtrustymatter
    libwebsockets
    lwip
    mt793x_sdk
    mynewt-core
    nxp
    openthread
    ot-br-posix
    qpg_sdk
    silabs
    simw-top-mini
    st
    ti_simplelink_sdk
    tizen
)

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Options:
  --image-tag TAG     Image ref to build (default: $IMAGE_TAG)
  --push              docker push after a successful build
  --no-build-cache    Pass --no-cache to docker build
  -h, --help          Show this help

Environment overrides:
  RHYTHM_RPIZ_BUILDER_IMAGE   Default image tag
  RHYTHM_CHIP_SRC_DIR         Override source of connectedhomeip/
  RHYTHM_X_TOOLS_DIR          Override ARMv6 musl toolchain path
  RHYTHM_BUILDROOT_DIR        Override buildroot/ path
  RHYTHM_RPIZ_OUT_DIR         Override out/rpiz/ (Buildroot output) path
  RHYTHM_CHIP_PRUNE_EXTRA     Space-separated third_party subdirs to additionally exclude
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --image-tag)
            IMAGE_TAG="$2"
            shift 2
            ;;
        --push)
            PUSH=true
            shift
            ;;
        --no-build-cache)
            NO_BUILD_CACHE=true
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

if [ "$(uname -s)" != "Linux" ]; then
    echo "Error: the builder image must be assembled on Linux (Buildroot host/sysroot is Linux-only)." >&2
    exit 1
fi

require_dir() {
    local label="$1" path="$2"
    if [ ! -d "$path" ]; then
        echo "Error: $label not found at $path" >&2
        exit 1
    fi
}

require_dir "connectedhomeip source" "$CHIP_SRC"
require_dir "ARMv6 musl toolchain"  "$TOOLCHAIN_SRC"
require_dir "buildroot checkout"    "$BUILDROOT_SRC"
require_dir "Buildroot output"       "$RPIZ_OUT_SRC"

if [ ! -d "$CHIP_SRC/out/rpiz-arm-musl" ]; then
    echo "Error: $CHIP_SRC/out/rpiz-arm-musl is missing. Run the chip build there first." >&2
    exit 1
fi

if [ ! -d "$RPIZ_OUT_SRC/host" ]; then
    echo "Error: $RPIZ_OUT_SRC/host is missing. Finish a local ./scripts/build-rpiz-image.sh run first." >&2
    exit 1
fi

if ! command -v docker >/dev/null 2>&1; then
    echo "Error: docker is required" >&2
    exit 1
fi

if ! command -v rsync >/dev/null 2>&1; then
    echo "Error: rsync is required" >&2
    exit 1
fi

CONTEXT_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-rpiz-builder.XXXXXX")"
trap 'rm -rf "$CONTEXT_DIR"' EXIT

echo "Assembling build context at $CONTEXT_DIR"

# --- connectedhomeip ---
echo "  connectedhomeip/ <- $CHIP_SRC"
# rsync filter rules are first-match-wins, so the "keep out/rpiz-arm-musl/"
# includes must precede the "drop every other out/ subdir" exclude.
chip_filters=(
    --filter='+ /out/'
    --filter='+ /out/rpiz-arm-musl/***'
    --filter='- /out/*'
    --exclude=.git
    --exclude=docs/
    --exclude=examples/
    --exclude=integrations/
    --exclude=crosstool-ng/
)
for dir in "${CHIP_PRUNE_DEFAULT[@]}" ${RHYTHM_CHIP_PRUNE_EXTRA:-}; do
    chip_filters+=(--exclude="third_party/$dir/")
done
rsync -a "${chip_filters[@]}" "$CHIP_SRC/" "$CONTEXT_DIR/connectedhomeip/"

# --- ARMv6 musl toolchain ---
echo "  x-tools/arm-unknown-linux-musleabihf/ <- $TOOLCHAIN_SRC"
mkdir -p "$CONTEXT_DIR/x-tools"
rsync -a "$TOOLCHAIN_SRC/" "$CONTEXT_DIR/x-tools/arm-unknown-linux-musleabihf/"

# --- Buildroot ---
echo "  buildroot/ <- $BUILDROOT_SRC"
rsync -a --exclude=.git "$BUILDROOT_SRC/" "$CONTEXT_DIR/buildroot/"

# --- Buildroot output (pre-warmed) ---
# Skip images/ (final artifacts, not input state). Rewrite the staging symlink
# to a relative path so it resolves both inside the image (/opt/rpiz-out/...)
# and after the in-container rsync to /output/.
#
# --chmod=a+rX: Buildroot creates some build-time dirs with mode 700 (e.g.
# dbus test fixtures, various XDG_RUNTIME_DIR scaffolds). Those end up owned
# by root when COPYed into the image, which breaks the in-container rsync when
# the container runs as --user <host-uid>. Relax to world-readable on the way
# into the context, preserving the executable bit only where it was already
# set (capital X).
echo "  rpiz-out/ <- $RPIZ_OUT_SRC"
rsync -a --exclude=/images/ --chmod=a+rX "$RPIZ_OUT_SRC/" "$CONTEXT_DIR/rpiz-out/"
if [ -L "$CONTEXT_DIR/rpiz-out/staging" ]; then
    rm -f "$CONTEXT_DIR/rpiz-out/staging"
fi
ln -s "host/arm-buildroot-linux-musleabi/sysroot" "$CONTEXT_DIR/rpiz-out/staging"

# Record the absolute build-time prefix this output tree was created against.
# Buildroot installs its host-side tools (fakeroot wrapper, pkg-config files,
# libtool archives, wrapper Makefiles, etc.) with this path hardcoded. The
# seed step in build-rpiz-image.sh reads this sidecar and sed-rewrites the
# prefix to the new OUTPUT_DIR so Buildroot can resume instead of failing to
# find its own libraries.
rpiz_out_abs="$(cd "$RPIZ_OUT_SRC" && pwd)"
printf '%s\n' "$rpiz_out_abs" > "$CONTEXT_DIR/rpiz-out/.rhythm-baked-prefix"
echo "  baked prefix: $rpiz_out_abs"

echo ""
echo "Context size:"
du -sh "$CONTEXT_DIR" 2>/dev/null || true
du -sh "$CONTEXT_DIR"/* 2>/dev/null || true

echo ""
echo "Building $IMAGE_TAG ..."
build_args=(build)
if [ "$NO_BUILD_CACHE" = true ]; then
    build_args+=(--no-cache)
fi
build_args+=(-f "$DOCKERFILE" -t "$IMAGE_TAG" "$CONTEXT_DIR")

docker "${build_args[@]}"

digest="$(docker image inspect --format '{{index .Id}}' "$IMAGE_TAG" 2>/dev/null || true)"
echo ""
echo "Built $IMAGE_TAG"
if [ -n "$digest" ]; then
    echo "  local digest: $digest"
fi

if [ "$PUSH" = true ]; then
    echo ""
    echo "Pushing $IMAGE_TAG ..."
    docker push "$IMAGE_TAG"
    echo "Pushed $IMAGE_TAG"
fi
