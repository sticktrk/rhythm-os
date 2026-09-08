#!/bin/bash
# Fast header-level compile check of the native Matter bridge
# (os/rust/bins/rhythm-chipd/native/chip_bridge.cc) against a connectedhomeip
# checkout, without linking libCHIP or the Linux platform deps. Runs on macOS
# too, so a bridge change can be semantically checked (templates instantiated,
# every SDK identifier resolved) before the Linux box does the full
# `--features chip-ffi` build.
#
# This is NOT the real build: linkage, platform (glib/dbus/avahi) headers used
# only by libCHIP internals, and the rpiz cross toolchain are exercised solely
# by the release rpiz-binaries job. Keep the include list in sync with
# `ChipArtifacts::include_dirs()` in os/rust/bins/rhythm-chipd/build.rs.
#
# Usage:
#   RHYTHM_CHIP_ROOT=/path/to/connectedhomeip \
#   RHYTHM_CHIP_OUT_DIR=/path/to/connectedhomeip/out/host \
#     tools/os/scripts/build/chip-bridge-syntax-check.sh [extra clang++ flags]
#
# Exit code is clang++'s (0 = clean).
# Pass --test-phone-discovery to compile/run the focused discovery regression
# against this same SDK's host library and headers.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CHIP_ROOT="${RHYTHM_CHIP_ROOT:-${RHYTHM_CHIP_SRC_DIR:-$REPO_ROOT/os/connectedhomeip}}"
CHIP_OUT_DIR="${RHYTHM_CHIP_OUT_DIR:-$CHIP_ROOT/out/host}"
BRIDGE_DIR="$REPO_ROOT/os/rust/bins/rhythm-chipd/native"

if [ ! -f "$CHIP_ROOT/src/lib/core/CHIPError.h" ]; then
    echo "Error: RHYTHM_CHIP_ROOT ($CHIP_ROOT) does not look like a connectedhomeip checkout" >&2
    exit 1
fi
if [ ! -d "$CHIP_OUT_DIR/gen/include" ]; then
    echo "Error: RHYTHM_CHIP_OUT_DIR ($CHIP_OUT_DIR) has no gen/include — run 'gn gen' + 'ninja' for a host build first" >&2
    exit 1
fi

CXX="${CXX:-clang++}"
INCLUDES=(
    -I"$CHIP_ROOT/src/include"
    -I"$CHIP_ROOT/src"
    -I"$CHIP_OUT_DIR/gen/include"
    -I"$CHIP_ROOT/config/standalone"
    -I"$CHIP_ROOT/zzz_generated/app-common"
    -I"$CHIP_ROOT/third_party/nlassert/repo/include"
    -I"$CHIP_ROOT/third_party/nlio/repo/include"
    -I"$CHIP_ROOT/third_party/nlfaultinjection/include"
    -I"$CHIP_ROOT/third_party/inipp/repo/inipp"
    -I"$CHIP_ROOT/third_party/ot-commissioner/repo/include"
    -I"$CHIP_ROOT/third_party/boringssl/repo/src/include"
    -I"$CHIP_ROOT/examples/platform/linux"
    -I"$BRIDGE_DIR"
)
case "$(uname -s)" in
    Darwin) INCLUDES+=(-I"$CHIP_ROOT/src/tracing/darwin/include") ;;
esac

PKG_FLAGS=()
if command -v pkg-config >/dev/null 2>&1; then
    # Optional: only needed if the SDK headers pull glib/dbus on this host.
    read -r -a PKG_FLAGS <<< "$(pkg-config --cflags glib-2.0 dbus-1 2>/dev/null || true)"
fi

echo "Checking $BRIDGE_DIR/chip_bridge.cc against $CHIP_ROOT ($(git -C "$CHIP_ROOT" describe --tags --always 2>/dev/null || echo unknown))"
if [ "${1:-}" = "--test-phone-discovery" ]; then
    shift
    TEST_BINARY="$(mktemp "${TMPDIR:-/tmp}/phone-discovery.XXXXXX")"
    trap 'rm -f "$TEST_BINARY"' EXIT
    "$CXX" -std=c++17 -fno-rtti -DCHIP_HAVE_CONFIG_H=1 -DOPENSSL_NO_ASM=1 \
        "${INCLUDES[@]}" "${PKG_FLAGS[@]}" "$@" \
        "$BRIDGE_DIR/tests/phone_commissioning_discovery_test.cc" \
        "$CHIP_OUT_DIR/lib/libCHIP.a" -o "$TEST_BINARY"
    "$TEST_BINARY"
    exit 0
fi
exec "$CXX" -std=c++17 -fno-rtti -fsyntax-only \
    -DCHIP_HAVE_CONFIG_H=1 -DOPENSSL_NO_ASM=1 -DRHYTHM_CHIP_BRIDGE_NATIVE_LIBCHIP=1 \
    "${INCLUDES[@]}" "${PKG_FLAGS[@]}" "$@" \
    "$BRIDGE_DIR/chip_bridge.cc"
