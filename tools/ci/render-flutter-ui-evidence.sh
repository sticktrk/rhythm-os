#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
APP_ROOT="$REPO_ROOT/app/flutter/rhythm_app"
RENDERER="rhythm_flutter_test_fonts_v1"

usage() {
    cat <<'EOF'
Usage: render-flutter-ui-evidence.sh TEST_PATH [flutter test arguments]

Render golden screenshots with deterministic Roboto, Roboto Mono, and Material
Icons fonts. Set the screenshot path environment variable consumed by the
selected test before invoking this command.

Example:
  RHYTHM_ROOM_CARD_SPINNER_SCREENSHOT=/tmp/evidence/room-card-spinner.png \
    tools/ci/render-flutter-ui-evidence.sh \
      test/widgets/room_card_test.dart \
      --plain-name "shows and clears a spinner while the room is transitioning"
EOF
}

if [ $# -eq 0 ]; then
    usage >&2
    exit 2
fi

[ -f "$APP_ROOT/test/flutter_test_config.dart" ] || {
    echo "FLUTTER UI RENDERER: missing test/flutter_test_config.dart" >&2
    exit 1
}

export RHYTHM_UI_EVIDENCE_RENDERER="$RENDERER"
cd "$APP_ROOT"
exec flutter test --update-goldens "$@"
