#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
HELPER="$SCRIPT_DIR/../lib/app-build-profile.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-app-build-profile-test.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

fail() {
    echo "APP BUILD PROFILE TEST: $*" >&2
    exit 1
}

PROFILE_DIR="$TEST_ROOT/config"
FLUTTER_APP="$TEST_ROOT/flutter"
PROFILE="$PROFILE_DIR/app-build.env"
mkdir -p "$PROFILE_DIR" "$FLUTTER_APP"
cat > "$PROFILE" <<'EOF'
SUPABASE_URL=https://file.example.invalid
SUPABASE_ANON_KEY=file-anon
LOGIN_ENABLED=true
EOF
chmod 600 "$PROFILE"

(
    export RHYTHM_APP_BUILD_ENV_FILE="$PROFILE"
    export SUPABASE_URL="https://override.example.invalid"
    # shellcheck source=../lib/app-build-profile.sh
    source "$HELPER"
    prepare_app_build_define_file "$REPO_ROOT" "$FLUTTER_APP"
    [ "$RHYTHM_APP_BUILD_PROFILE_SOURCE" = "external" ] || \
        fail "external profile was not selected"
    [ "$(cd "$(dirname "$RHYTHM_APP_BUILD_DEFINE_FILE")" && pwd)" = \
        "$(cd "$PROFILE_DIR" && pwd)" ] || \
        fail "materialized profile was not kept beside its external source"
    python3 - "$RHYTHM_APP_BUILD_DEFINE_FILE" <<'PY'
import json
import stat
import sys
from pathlib import Path

path = Path(sys.argv[1])
values = json.loads(path.read_text(encoding="utf-8"))
assert stat.S_IMODE(path.stat().st_mode) == 0o600
assert values["SUPABASE_URL"] == "https://override.example.invalid"
assert values["SUPABASE_ANON_KEY"] == "file-anon"
PY
    materialized="$RHYTHM_APP_BUILD_DEFINE_FILE"
    cleanup_app_build_define_file
    [ ! -e "$materialized" ] || fail "private materialized profile was not removed"
)

MISSING_OUTPUT="$TEST_ROOT/missing.txt"
if (
    export RHYTHM_APP_BUILD_ENV_FILE="$TEST_ROOT/missing.env"
    # shellcheck source=../lib/app-build-profile.sh
    source "$HELPER"
    prepare_app_build_define_file "$REPO_ROOT" "$FLUTTER_APP"
) >"$MISSING_OUTPUT" 2>&1; then
    fail "missing explicit app-build profile did not fail closed"
fi
grep -Fq "external app-build profile is not ready" "$MISSING_OUTPUT" || \
    fail "missing profile failure was not actionable"
if grep -Fq "$TEST_ROOT" "$MISSING_OUTPUT"; then
    fail "missing profile failure exposed a resolved local path"
fi

echo "App build profile materialization tests passed"
