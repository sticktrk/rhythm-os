#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_SCRIPTS="$(cd "$SCRIPT_DIR/.." && pwd)"
WRITER="$APP_SCRIPTS/write-testflight-notes.sh"
BUILD_SCRIPT="$APP_SCRIPTS/build-mobile.sh"
WORKER="$APP_SCRIPTS/run-testflight-dispatch.sh"
BATCH_DISPATCH="$APP_SCRIPTS/dispatch-testflight-batch.sh"
PR_DISPATCH="$APP_SCRIPTS/dispatch-testflight.sh"
BATCH_DISPATCH_TEST="$SCRIPT_DIR/testflight-batch-dispatch-test.sh"
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
MISSING_OUTPUT="$(RHYTHM_TESTFLIGHT_NOTES_FILE= "$BUILD_SCRIPT" --testflight 2>&1)"
MISSING_STATUS=$?
set -e
[ "$MISSING_STATUS" -ne 0 ] || fail "TestFlight upload accepted missing notes"
grep -Fq "require build-specific 'What to Test' notes" <<<"$MISSING_OUTPUT" || \
    fail "missing-notes failure was not actionable"

grep -Fq -- '--changelog "$TESTFLIGHT_CHANGELOG"' "$BUILD_SCRIPT" || \
    fail "Fastlane upload does not receive rendered changelog"
grep -Fq 'RHYTHM_TESTFLIGHT_NOTES_FILE="$NOTES_FILE"' "$WORKER" || \
    fail "PR dispatch worker does not hand notes to the uploader"

bash -n "$BATCH_DISPATCH" || fail "batch dispatcher has invalid shell syntax"
bash -n "$PR_DISPATCH" || fail "PR dispatcher has invalid shell syntax"
grep -Fq 'if [ ! -f "$WORKTREE/tools/config/materialize_app_build.py" ]; then' \
    "$PR_DISPATCH" || fail "PR dispatcher accepts heads without external profile support"
grep -Fq 'if [ "$LIVE_MASTER" != "$COMMIT_SHA" ]; then' "$BATCH_DISPATCH" || \
    fail "batch dispatcher does not bind the upload to current origin/master"
grep -Fq 'git tag --points-at "$COMMIT_SHA"' "$BATCH_DISPATCH" || \
    fail "batch dispatcher does not require an exact beta tag"
grep -Fq 'git merge-base --is-ancestor "$PR_MERGE" "$COMMIT_SHA"' "$BATCH_DISPATCH" || \
    fail "batch dispatcher does not prove every named PR is in the final commit"
grep -Fq 'RHYTHM_TESTFLIGHT_BATCH_BASE="$BASE_SHA"' "$BATCH_DISPATCH" || \
    fail "batch dispatcher does not preserve the immutable batch base in its receipt"
"$BATCH_DISPATCH_TEST" || fail "batch dispatcher remote-tag gate failed"

echo "TestFlight notes tests passed"
