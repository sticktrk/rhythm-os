#!/usr/bin/env bash
set -euo pipefail
: "${RHYTHM_ADDON_ROOT:?Set RHYTHM_ADDON_ROOT to your rhythm-home-assistant checkout}"
exec python3 "$RHYTHM_ADDON_ROOT/tools/build.py" "$@"
