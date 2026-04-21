#!/bin/bash
# Fast dev iteration: cross-compile rhythm-linux-appliance for rpiz inside
# the prebuilt builder image, scp to the target appliance, swap the binary,
# and kick BusyBox init to respawn it with the new code. Skips git, CI,
# Docker Hub pushes, and OTA entirely.
#
# Does NOT rebuild rhythm-chipd — the running chipd keeps going under its
# previous binary. Use for iterating on appliance-side logic that doesn't
# depend on matching chipd changes.
#
# Usage:
#   ./scripts/push-rpiz-dev.sh <user@host> [--image <ref>] [--no-pull] [--remote-path <path>]
#
# Requires on this host:
#   - docker
#   - ssh + scp reachable to <user@host> (dev-mode rpiz images have Dropbear
#     + root password `rhythm`; you probably want `ssh-copy-id` first so we
#     don't prompt for a password every iteration)
#
# Examples:
#   ./scripts/push-rpiz-dev.sh root@rhythm-rpiz.local
#   ./scripts/push-rpiz-dev.sh root@192.168.7.2 --no-pull

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
LOCK_FILE="$PROJECT_ROOT/install/rpiz/builder-image.lock"
RUST_TARGET="arm-unknown-linux-musleabihf"
REMOTE_PATH="/usr/bin/rhythm-server"
DOCKER_PULL=true
SSH_DEST=""
IMAGE_OVERRIDE=""

usage() {
    sed -n '2,24p' "$0"
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --image)
            IMAGE_OVERRIDE="$2"
            shift 2
            ;;
        --no-pull)
            DOCKER_PULL=false
            shift
            ;;
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

for cmd in docker ssh scp; do
    if ! command -v "$cmd" >/dev/null 2>&1; then
        echo "Error: $cmd is required" >&2
        exit 1
    fi
done

resolve_image() {
    if [ -n "$IMAGE_OVERRIDE" ]; then
        echo "$IMAGE_OVERRIDE"
        return
    fi
    if [ -n "${RHYTHM_RPIZ_BUILDER_IMAGE:-}" ]; then
        echo "$RHYTHM_RPIZ_BUILDER_IMAGE"
        return
    fi
    if [ -f "$LOCK_FILE" ]; then
        awk -F= '/^image=/ {print $2; exit}' "$LOCK_FILE"
        return
    fi
    echo "dtconcepts/rhythm-rpiz-builder:latest"
}

IMAGE="$(resolve_image)"
BINARY_PATH="target/$RUST_TARGET/release/rhythm-linux-appliance"

if [ "$DOCKER_PULL" = true ]; then
    echo "Pulling $IMAGE ..."
    docker pull "$IMAGE"
fi

echo "Cross-compiling rhythm-linux-appliance for $RUST_TARGET ..."
# Direct cargo build inside the image — skip build-server.sh so we don't
# also compile rhythm-chipd. The image ENV sets PATH to include the ARMv6
# musl toolchain; we pin the linker explicitly so this works regardless of
# whether the inner shell re-sources anything.
docker run --rm -t \
    -e HOME=/tmp/rhythm-home \
    -e LANG=C.UTF-8 \
    -e LC_ALL=C.UTF-8 \
    -e CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABIHF_LINKER=arm-unknown-linux-musleabihf-gcc \
    -e CC_arm_unknown_linux_musleabihf=arm-unknown-linux-musleabihf-gcc \
    --user "$(id -u):$(id -g)" \
    -v "$PROJECT_ROOT:/workspace" \
    -w /workspace \
    "$IMAGE" bash -c "cargo build --release -p rhythm-linux-appliance --target $RUST_TARGET"

if [ ! -x "$PROJECT_ROOT/$BINARY_PATH" ]; then
    echo "Error: missing expected output at $BINARY_PATH" >&2
    exit 1
fi

echo "Pushing binary to $SSH_DEST:$REMOTE_PATH ..."
scp "$PROJECT_ROOT/$BINARY_PATH" "$SSH_DEST:/tmp/rhythm-server.new"

# Atomic swap + restart via BusyBox init respawn. We `mv` (not `cp`) because
# the destination is in use — `mv` unlinks the old inode and links in the new
# file, which is safe while the old binary's mapped pages keep running until
# the process exits. Then killall triggers init to respawn with the new one.
ssh "$SSH_DEST" bash -s -- "$REMOTE_PATH" <<'REMOTE'
set -euo pipefail
DEST="$1"
chmod +x /tmp/rhythm-server.new
mv /tmp/rhythm-server.new "$DEST"
killall rhythm-server 2>/dev/null || true
sleep 1
echo "respawned; current version:"
"$DEST" --version 2>/dev/null || head -c 80 "$DEST" | strings | head -1 || true
REMOTE

echo ""
echo "Done. Tail logs with:"
echo "  ssh $SSH_DEST 'tail -f /var/log/rhythm-server.log'"
