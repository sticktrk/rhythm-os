#!/bin/bash
# Check both source and archive; patching source alone does not update libCHIP.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHIP_ROOT="${1:?usage: check-chip-bluez-idle-guard.sh CHIP_ROOT LIBCHIP_ARCHIVE}"
ARCHIVE="${2:?usage: check-chip-bluez-idle-guard.sh CHIP_ROOT LIBCHIP_ARCHIVE}"
if ! git -C "$CHIP_ROOT" apply --reverse --check "$SCRIPT_DIR/patches/chip-bluez-idle-guard.patch" 2>/dev/null; then
    echo "Error: run tools/os/scripts/build/apply-chip-bluez-idle-guard.sh on this SDK, then rebuild libCHIP.a" >&2
    exit 1
fi
if ! "${NM:-nm}" -g "$ARCHIVE" | awk '$NF ~ /^_?rhythm_chip_bluez_idle_guard_v1$/ && $(NF-1) == "T" { found = 1 } END { exit !found }'; then
    echo "Error: libCHIP.a lacks the BlueZ idle guard; rebuild CHIP with the target toolchain before packaging" >&2
    exit 1
fi
