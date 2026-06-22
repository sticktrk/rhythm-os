#!/usr/bin/env bash
# Capture tutorial screenshots of the Matter pairing flow.
#
# Usage:
#   ./tools/app/scripts/capture-tutorial-screenshots.sh           # uses first available device
#   ./tools/app/scripts/capture-tutorial-screenshots.sh -d <id>   # specific device
#
# Output: flutter/rhythm_app/screenshots/*.png
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_ROOT="$(cd "$SCRIPT_DIR/../../../app/flutter/rhythm_app" && pwd)"

cd "$APP_ROOT"

DEVICE_ARG=()
if [[ $# -gt 0 ]]; then
  DEVICE_ARG=("$@")
fi

mkdir -p screenshots

echo "==> Running integration test against device: ${DEVICE_ARG[*]:-<auto>}"
flutter drive \
  --driver=test_driver/integration_test.dart \
  --target=integration_test/matter_pairing_screenshots_test.dart \
  ${DEVICE_ARG[@]+"${DEVICE_ARG[@]}"}

echo
echo "Screenshots saved to: $(pwd)/screenshots/"
ls -1 screenshots/ | sed 's/^/  /'
