#!/bin/bash
# Hash the slow-path inputs that actually require a builder-image rebuild.
# Outputs a 12-character sha256 prefix on stdout — used as the image tag.
#
# Inputs hashed (in order):
#   - tools/os/scripts/build/Dockerfile.rpiz-builder
#   - tools/os/scripts/build/build-rpiz-builder-image.sh
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
#   - tools/os/scripts/build-rpiz-image.sh, tools/os/scripts/build-server.sh (run at invoke time)
#
# Modes:
#   (default)     print the 12-char input hash
#   --inputs      print the hashed input lines (debugging: which input moved?)
#   --chip-info   print human-readable connectedhomeip pin metadata as
#                 key=value lines (chip_rev / chip_ref / chip_diff). Not part
#                 of the hash — the refresh script records it in
#                 install/rpiz/builder-image.lock so the SDK a release was
#                 built against is readable from the repo.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../../os" && pwd)"

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

git_describe() {
    # Nearest release tag + distance, e.g. v1.5.1.0 or v1.4.2.0-2520-gb468bbbea0.
    # Prefer bumping connectedhomeip to an exact release tag so this reads as
    # a plain version (see build/README.md "Bumping connectedhomeip").
    if [ -d "$1/.git" ] || [ -f "$1/.git" ]; then
        git -C "$1" describe --tags --always 2>/dev/null || echo "no-describe"
    else
        echo "no-git"
    fi
}

hashed_inputs() {
    echo "dockerfile $(sha_file "$SCRIPT_DIR/Dockerfile.rpiz-builder")"
    echo "packager   $(sha_file "$SCRIPT_DIR/build-rpiz-builder-image.sh")"
    echo "br-external $(sha_dir_tree "$PROJECT_ROOT/install/rpiz/buildroot")"
    echo "buildroot  $(git_rev "$BUILDROOT_SRC")"
    echo "chip-rev   $(git_rev "$CHIP_SRC")"
    echo "chip-diff  $(git_local_diff_sha "$CHIP_SRC")"
    echo "libchip    $(sha_file "$CHIP_SRC/out/rpiz-arm-musl/lib/libCHIP.a")"
    echo "args.gn    $(sha_file "$CHIP_SRC/out/rpiz-arm-musl/args.gn")"
    echo "toolchain  $(sha_file "$TOOLCHAIN_SRC/bin/arm-unknown-linux-musleabihf-gcc")"
}

chip_info() {
    # Informational only. Deliberately NOT hashed so adding/changing these
    # lines never forces a builder-image rebuild.
    local diff_sha empty_sha
    diff_sha="$(git_local_diff_sha "$CHIP_SRC")"
    empty_sha="$(printf '' | sha256sum | awk '{print $1}')"
    if [ "$diff_sha" = "$empty_sha" ]; then
        diff_sha="clean"
    fi
    echo "chip_rev=$(git_rev "$CHIP_SRC")"
    echo "chip_ref=$(git_describe "$CHIP_SRC")"
    echo "chip_diff=$diff_sha"
}

case "${1:-}" in
    "")
        hashed_inputs | sha256sum | cut -c1-12
        ;;
    --inputs)
        hashed_inputs
        ;;
    --chip-info)
        chip_info
        ;;
    -h|--help)
        sed -n '2,32p' "$0"
        ;;
    *)
        echo "Unknown option: $1" >&2
        exit 1
        ;;
esac
