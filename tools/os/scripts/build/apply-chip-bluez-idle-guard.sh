#!/bin/bash
# Apply the shared-adapter guard before rebuilding the external CHIP archive.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
CHIP_ROOT="${1:-${RHYTHM_CHIP_ROOT:-${RHYTHM_CHIP_SRC_DIR:-$REPO_ROOT/os/connectedhomeip}}}"
PATCH="$SCRIPT_DIR/patches/chip-bluez-idle-guard.patch"
if git -C "$CHIP_ROOT" apply --reverse --check "$PATCH" 2>/dev/null; then
    echo "CHIP BlueZ idle guard is already applied"
elif git -C "$CHIP_ROOT" apply --check "$PATCH"; then
    git -C "$CHIP_ROOT" apply "$PATCH"
    echo "Applied CHIP BlueZ idle guard; rebuild lib/libCHIP.a before packaging"
else
    echo "Error: CHIP BlueZ source differs from the supported patch; review the SDK upgrade before building" >&2
    exit 1
fi
