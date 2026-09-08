#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
RENDERER="$SCRIPT_DIR/render-flutter-ui-evidence.sh"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cross-flutter-ui-renderer-test.XXXXXX")"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

fail() {
    echo "FLUTTER UI RENDERER TEST: $*" >&2
    exit 1
}

BIN_DIR="$TMP_DIR/bin"
mkdir -p "$BIN_DIR"
cat > "$BIN_DIR/flutter" <<'EOF'
#!/bin/bash
pwd -P > "$FAKE_FLUTTER_PWD"
printf '%s\n' "$RHYTHM_UI_EVIDENCE_RENDERER" > "$FAKE_FLUTTER_RENDERER"
printf '%s\n' "$@" > "$FAKE_FLUTTER_ARGS"
EOF
chmod +x "$BIN_DIR/flutter"

export FAKE_FLUTTER_PWD="$TMP_DIR/pwd"
export FAKE_FLUTTER_RENDERER="$TMP_DIR/renderer"
export FAKE_FLUTTER_ARGS="$TMP_DIR/args"
PATH="$BIN_DIR:$PATH" "$RENDERER" \
    test/widgets/example_test.dart --plain-name 'visible state'

[ "$(cat "$FAKE_FLUTTER_PWD")" = "$(cd "$SCRIPT_DIR/../../app/flutter/rhythm_app" && pwd -P)" ] ||
    fail "renderer did not run from the Flutter app root"
[ "$(cat "$FAKE_FLUTTER_RENDERER")" = "rhythm_flutter_test_fonts_v1" ] ||
    fail "renderer identity was not exported"
grep -Fxq -- '--update-goldens' "$FAKE_FLUTTER_ARGS" ||
    fail "renderer did not enable golden updates"
grep -Fxq -- 'test/widgets/example_test.dart' "$FAKE_FLUTTER_ARGS" ||
    fail "test path was not forwarded"
grep -Fxq -- 'visible state' "$FAKE_FLUTTER_ARGS" ||
    fail "plain-name selector was not forwarded"

echo "Flutter UI evidence renderer contract passed."
