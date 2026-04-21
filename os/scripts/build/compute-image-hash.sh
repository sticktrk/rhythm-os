#!/bin/bash
# Hash the slow-path inputs that actually require a builder-image rebuild.
# Outputs a 12-character sha256 prefix on stdout — used as the image tag.
#
# Inputs hashed (in order):
#   - scripts/build/Dockerfile.rpiz-builder
#   - scripts/build/build-rpiz-builder-image.sh
#   - install/rpiz/buildroot/**  (defconfig, fragments, overlays, packages)
#   - buildroot/ commit SHA
#   - connectedhomeip/ commit SHA + local-diff hash (captures BLE patch)
#   - connectedhomeip/out/rpiz-arm-musl/lib/libCHIP.a (content sha)
#   - connectedhomeip/out/rpiz-arm-musl/args.gn (gn config)
#   - ~/x-tools/arm-unknown-linux-musleabihf/bin/arm-unknown-linux-musleabihf-gcc
#
# Inputs NOT hashed (they run inside the image and don't need rebuild):
#   - rust/** source code
#   - Cargo.{toml,lock}
#   - scripts/build-rpiz-image.sh, scripts/build-server.sh (run at invoke time)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

CHIP_SRC="${RHYTHM_CHIP_SRC_DIR:-$PROJECT_ROOT/connectedhomeip}"
BUILDROOT_SRC="${RHYTHM_BUILDROOT_DIR:-$PROJECT_ROOT/buildroot}"
TOOLCHAIN_SRC="${RHYTHM_X_TOOLS_DIR:-$HOME/x-tools/arm-unknown-linux-musleabihf}"

sha_file() {
    if [ -f "$1" ]; then
        sha256sum "$1" | awk '{print $1}'
    else
        echo "missing-$(basename "$1")"
    fi
}

sha_dir_tree() {
    # Deterministic directory hash: path + mode + content, sorted.
    if [ ! -d "$1" ]; then
        echo "missing-dir"
        return
    fi
    (cd "$1" && find . -type f -print0 2>/dev/null | sort -z | while IFS= read -r -d '' path; do
        printf '%s\0' "$path"
        sha256sum "$path" | awk '{print $1}'
    done) | sha256sum | awk '{print $1}'
}

git_rev() {
    if [ -d "$1/.git" ] || [ -f "$1/.git" ]; then
        git -C "$1" rev-parse HEAD 2>/dev/null || echo "no-rev"
    else
        echo "no-git"
    fi
}

git_local_diff_sha() {
    # Captures uncommitted changes (e.g. the BLE patch you keep in
    # connectedhomeip/). Empty-tree sha when clean.
    if [ -d "$1/.git" ] || [ -f "$1/.git" ]; then
        (git -C "$1" diff HEAD 2>/dev/null; git -C "$1" diff --cached 2>/dev/null) \
            | sha256sum | awk '{print $1}'
    else
        echo "no-diff"
    fi
}

{
    echo "dockerfile $(sha_file "$SCRIPT_DIR/Dockerfile.rpiz-builder")"
    echo "packager   $(sha_file "$SCRIPT_DIR/build-rpiz-builder-image.sh")"
    echo "br-external $(sha_dir_tree "$PROJECT_ROOT/install/rpiz/buildroot")"
    echo "buildroot  $(git_rev "$BUILDROOT_SRC")"
    echo "chip-rev   $(git_rev "$CHIP_SRC")"
    echo "chip-diff  $(git_local_diff_sha "$CHIP_SRC")"
    echo "libchip    $(sha_file "$CHIP_SRC/out/rpiz-arm-musl/lib/libCHIP.a")"
    echo "args.gn    $(sha_file "$CHIP_SRC/out/rpiz-arm-musl/args.gn")"
    echo "toolchain  $(sha_file "$TOOLCHAIN_SRC/bin/arm-unknown-linux-musleabihf-gcc")"
} | sha256sum | cut -c1-12
