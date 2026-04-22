#!/bin/bash
# Fast dev iteration: native cross-compile rhythm-linux-appliance on the host,
# scp to the target appliance, swap the binary, and kick BusyBox init to
# respawn with the new code. Skips git, CI, Docker, and OTA entirely.
#
# Does NOT rebuild rhythm-chipd — the running chipd keeps going under its
# previous binary. Use for iterating on appliance-side logic that doesn't
# depend on matching chipd changes.
#
# One-time host setup:
#   macOS:   brew install filosottile/musl-cross/musl-cross --with-arm-hf
#   Linux:   sudo apt install gcc-arm-linux-gnueabihf  (or use your ~/x-tools
#            crosstool-NG build)
#   Both:    rustup target add arm-unknown-linux-musleabihf
#
# One-time target setup:
#   ssh-copy-id root@rhythm-rpiz.local    # password is `rhythm` on dev images
#
# Usage:
#   ./scripts/push-rpiz-dev.sh <user@host> [--remote-path <path>]
#
# Examples:
#   ./scripts/push-rpiz-dev.sh root@rhythm-rpiz.local
#   ./scripts/push-rpiz-dev.sh root@192.168.7.2

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
RUST_TARGET="arm-unknown-linux-musleabihf"
REMOTE_PATH="/usr/bin/rhythm-server"
SSH_DEST=""

usage() { sed -n '2,26p' "$0"; }

while [[ $# -gt 0 ]]; do
    case "$1" in
        --remote-path)
            REMOTE_PATH="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -*)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
        *)
            if [ -n "$SSH_DEST" ]; then
                echo "Error: unexpected extra argument '$1'" >&2
                usage >&2
                exit 1
            fi
            SSH_DEST="$1"
            shift
            ;;
    esac
done

if [ -z "$SSH_DEST" ]; then
    echo "Error: missing <user@host> argument" >&2
    usage >&2
    exit 1
fi

for cmd in cargo rustup ssh scp; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: $cmd is required" >&2
        exit 1
    fi
done

# Make sure the Rust target is installed.
if ! rustup target list --installed 2>/dev/null | grep -q "^$RUST_TARGET\$"; then
    echo "Installing rustup target $RUST_TARGET ..."
    rustup target add "$RUST_TARGET"
fi

# Resolve the ARMv6 hard-float musl linker. Prefer the crosstool-NG
# arm-unknown-* naming when it's on PATH (matches the prebuilt CHIP libs on
# Linux), fall back to the filosottile brew naming on macOS.
linker=""
if command -v arm-unknown-linux-musleabihf-gcc >/dev/null 2>&1; then
    linker="arm-unknown-linux-musleabihf-gcc"
elif command -v arm-linux-musleabihf-gcc >/dev/null 2>&1; then
    linker="arm-linux-musleabihf-gcc"
else
    cat >&2 <<'HINT'
Error: no ARMv6 hard-float musl cross compiler on PATH.
  macOS:  brew install filosottile/musl-cross/musl-cross --with-arm-hf
  Linux:  install arm-unknown-linux-musleabihf-gcc (crosstool-NG), or
          ensure ~/x-tools/arm-unknown-linux-musleabihf/bin is on PATH
HINT
    exit 1
fi

BUILD_VERSION="$("$SCRIPT_DIR/resolve-version.sh" server)"
echo "Cross-compiling rhythm-linux-appliance v$BUILD_VERSION for $RUST_TARGET (linker: $linker)"
export CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$linker"
export CC_arm_unknown_linux_musleabihf="$linker"
export RHYTHM_BUILD_VERSION="$BUILD_VERSION"

cd "$PROJECT_ROOT"
cargo build --release -p rhythm-linux-appliance --target "$RUST_TARGET"

BINARY="target/$RUST_TARGET/release/rhythm-linux-appliance"
if [ ! -x "$BINARY" ]; then
    echo "Error: missing expected output at $BINARY" >&2
    exit 1
fi

echo "Pushing to $SSH_DEST:$REMOTE_PATH ..."
# -O forces legacy SCP protocol (BSD rcp-over-SSH) instead of the modern
# default SFTP — Dropbear on the rpiz image doesn't ship sftp-server.
scp -O "$BINARY" "$SSH_DEST:/tmp/rhythm-server.new"

# Atomic swap + restart via BusyBox init respawn. `mv` unlinks the old inode
# (safe while the running process keeps its mapped pages) and links in the
# new file; killing the old process lets init respawn with the new binary.
ssh "$SSH_DEST" sh -s -- "$REMOTE_PATH" <<'REMOTE'
set -eu
DEST="$1"
chmod +x /tmp/rhythm-server.new
mv /tmp/rhythm-server.new "$DEST"
killall rhythm-server 2>/dev/null || true
sleep 1
echo "respawned; version:"
"$DEST" --version 2>/dev/null || true
REMOTE

echo ""
echo "Done. Tail logs with:"
echo "  ssh $SSH_DEST 'tail -f /var/log/rhythm-server.log'"
