#!/usr/bin/env bash
# Capture tutorial screenshots of the Matter pairing flow.
#
# Usage:
#   ./scripts/capture-tutorial-screenshots.sh           # uses first available device
#   ./scripts/capture-tutorial-screenshots.sh -d <id>   # specific device
#
# Output: flutter/rhythm_app/screenshots/*.png
set -euo pipefail

cd "$(dirname "$0")/../flutter/rhythm_app"

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
