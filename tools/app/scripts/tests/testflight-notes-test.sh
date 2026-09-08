#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_SCRIPTS="$(cd "$SCRIPT_DIR/.." && pwd)"
WRITER="$APP_SCRIPTS/write-testflight-notes.sh"
BUILD_SCRIPT="$APP_SCRIPTS/build-mobile.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-testflight-notes-test.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

cat > "$TEST_ROOT/body-explicit.md" <<'EOF'
## Summary

- General implementation summary

## TestFlight — What to Test

<!-- This guidance must not reach testers. -->
- Confirm reconnecting restores the selected room.
- Confirm schedule editing remains responsive.

## Test plan

- Internal implementation details
EOF

"$WRITER" \
    --pr 42 \
    --title "Improve room reconnect behavior" \
    --body-file "$TEST_ROOT/body-explicit.md" \
    --output "$TEST_ROOT/explicit.txt"

cat > "$TEST_ROOT/expected-explicit.txt" <<'EOF'
PR #42 — Improve room reconnect behavior

- Confirm reconnecting restores the selected room.
- Confirm schedule editing remains responsive.
EOF

cmp -s "$TEST_ROOT/expected-explicit.txt" "$TEST_ROOT/explicit.txt" || \
    fail "explicit TestFlight section was not rendered exactly"

cat > "$TEST_ROOT/body-summary.md" <<'EOF'
## Summary

<!-- What changed? -->
- Fixes delayed schedule starts.

## TestFlight — What to Test

-

## Release notes

- Not part of the summary
EOF

"$WRITER" \
    --pr 43 \
    --title "Fix delayed schedules" \
    --body-file "$TEST_ROOT/body-summary.md" \
    --output "$TEST_ROOT/summary.txt"

grep -Fq -- '- Fixes delayed schedule starts.' "$TEST_ROOT/summary.txt" || \
    fail "empty TestFlight section did not fall back to Summary"
if grep -Fq 'Not part of the summary' "$TEST_ROOT/summary.txt"; then
    fail "renderer crossed the Summary section boundary"
fi

: > "$TEST_ROOT/body-empty.md"
"$WRITER" \
    --pr 44 \
    --title "Title-only fallback" \
    --body-file "$TEST_ROOT/body-empty.md" \
    --output "$TEST_ROOT/title.txt"
[ "$(cat "$TEST_ROOT/title.txt")" = "PR #44 — Title-only fallback" ] || \
    fail "empty PR body did not fall back to title"

{
    printf '## TestFlight — What to Test\n\n'
    awk 'BEGIN { for (i = 0; i < 4500; i++) printf "x"; printf "\n" }'
} > "$TEST_ROOT/body-long.md"
"$WRITER" \
    --pr 45 \
    --title "Long notes" \
    --body-file "$TEST_ROOT/body-long.md" \
    --output "$TEST_ROOT/long.txt"
LONG_LENGTH="$(awk '{ total += length($0) + (NR > 1 ? 1 : 0) } END { print total }' "$TEST_ROOT/long.txt")"
[ "$LONG_LENGTH" -eq 4000 ] || fail "rendered notes length is $LONG_LENGTH, expected 4000"
grep -Eq '\.\.\.$' "$TEST_ROOT/long.txt" || fail "truncated notes lack a marker"

set +e
MISSING_OUTPUT="$(RHYTHM_TESTFLIGHT_NOTES_FILE= "$BUILD_SCRIPT" --testflight --testflight-notes-file "$TEST_ROOT/missing.txt" 2>&1)"
MISSING_STATUS=$?
set -e
[ "$MISSING_STATUS" -ne 0 ] || fail "TestFlight upload accepted a nonexistent notes file"
grep -Fq "TestFlight notes file not found" <<<"$MISSING_OUTPUT" || \
    fail "nonexistent-notes failure was not actionable"

grep -Fq 'if [ "$UPLOAD_TESTFLIGHT" = true ] && [ -n "$TESTFLIGHT_NOTES_FILE" ]; then' \
    "$BUILD_SCRIPT" || fail "TestFlight notes validation is not optional"
grep -Fq 'TESTFLIGHT_UPLOAD_ARGS+=(--changelog "$TESTFLIGHT_CHANGELOG")' "$BUILD_SCRIPT" || \
    fail "Fastlane upload does not conditionally receive the rendered changelog"
grep -Fq 'fastlane pilot upload "${TESTFLIGHT_UPLOAD_ARGS[@]}"' "$BUILD_SCRIPT" || \
    fail "Fastlane upload does not use the optional argument list"

echo "TestFlight notes tests passed"
