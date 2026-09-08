#!/bin/bash

set -euo pipefail

# Git hooks export repository-local variables such as GIT_DIR. Clear them before
# creating the disposable fixture repository so its commits cannot touch the
# checkout that invoked the hook.
while IFS= read -r git_var; do
    [ -n "$git_var" ] && unset "$git_var"
done < <(git rev-parse --local-env-vars)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
POSTER="$SCRIPT_DIR/post-flutter-ui-evidence.sh"
DETECTOR="$SCRIPT_DIR/detect-changed-surfaces.sh"
FAKE_GH="$SCRIPT_DIR/tests/fixtures/fake-gh-flutter-ui-evidence.sh"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cross-flutter-ui-evidence-test.XXXXXX")"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

fail() {
    echo "FLUTTER UI EVIDENCE TEST: $*" >&2
    exit 1
}

REPO="$TMP_DIR/repo"
BIN_DIR="$TMP_DIR/bin"
mkdir -p "$REPO/tools/ci" "$BIN_DIR"
cp "$DETECTOR" "$REPO/tools/ci/detect-changed-surfaces.sh"
cp "$FAKE_GH" "$BIN_DIR/gh"
chmod +x "$REPO/tools/ci/detect-changed-surfaces.sh" "$BIN_DIR/gh"

cd "$REPO"
git init -q
git config user.name "Flutter Evidence Test"
git config user.email "fixture@example.invalid"
printf 'baseline\n' > README.md
git add README.md tools/ci/detect-changed-surfaces.sh
git commit -qm baseline
FAKE_BASE_SHA="$(git rev-parse HEAD)"

mkdir -p app/flutter/rhythm_app/lib/widgets
printf 'class EvidenceWidget {}\n' > app/flutter/rhythm_app/lib/widgets/evidence_widget.dart
git add app/flutter/rhythm_app/lib/widgets/evidence_widget.dart
git commit -qm ui-change
FAKE_HEAD_SHA="$(git rev-parse HEAD)"

SCREENSHOT_DIR="$TMP_DIR/evidence/screenshots"
mkdir -p "$SCREENSHOT_DIR"
cp "$SCRIPT_DIR/../../app/flutter/rhythm_app/web/favicon.png" "$SCREENSHOT_DIR/room-settings.png"
MANIFEST="$TMP_DIR/evidence/manifest.json"
jq -n '{
    schema_version: 1,
    uses_mock_data: true,
    renderer: "rhythm_flutter_test_fonts_v1",
    source_test: "test/visual/room_settings_test.dart",
    screenshots: [{path: "screenshots/room-settings.png", caption: "Room settings — Auto color"}]
}' > "$MANIFEST"

export FAKE_BASE_SHA FAKE_HEAD_SHA
export FAKE_GH_LOG="$TMP_DIR/gh.log"
export FAKE_COMMENT_JSON="$TMP_DIR/comment.json"
export FAKE_TREE_JSON="$TMP_DIR/tree.json"
export CROSS_GITHUB_REPOSITORY="test/repo"
export PATH="$BIN_DIR:$PATH"
: > "$FAKE_GH_LOG"

"$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$MANIFEST"

grep -Fq 'POST repos/test/repo/git/blobs' "$FAKE_GH_LOG" || fail "screenshots were not uploaded as Git blobs"
grep -Fq 'POST repos/test/repo/git/trees' "$FAKE_GH_LOG" || fail "evidence tree was not created"
grep -Fq 'GET repos/test/repo/git/ref/heads/codex-ui-evidence' "$FAKE_GH_LOG" || fail "shared evidence branch was not resolved"
grep -Fq 'PATCH repos/test/repo/git/refs/heads/codex-ui-evidence' "$FAKE_GH_LOG" || fail "shared evidence branch was not advanced"
grep -Fq 'POST repos/test/repo/issues/7/comments' "$FAKE_GH_LOG" || fail "PR comment was not posted"
[ "$(jq '.tree | length' "$FAKE_TREE_JSON")" -eq 2 ] || fail "tree should contain one screenshot and one manifest"
[ "$(jq -r '.base_tree' "$FAKE_TREE_JSON")" = "dddddddddddddddddddddddddddddddddddddddd" ] || fail "new shared branch should extend its initial default-branch tree"
COMMENT_BODY="$(jq -r '.body' "$FAKE_COMMENT_JSON")"
printf '%s' "$COMMENT_BODY" | grep -Fq "<!-- codex-cross-flutter-ui pr=7 head=$FAKE_HEAD_SHA -->" || fail "comment is missing the exact-head marker"
printf '%s' "$COMMENT_BODY" | grep -Fq 'deterministic mock/test data' || fail "comment is missing the privacy statement"
printf '%s' "$COMMENT_BODY" | grep -Fq 'Room settings — Auto color' || fail "comment is missing the screenshot caption"
printf '%s' "$COMMENT_BODY" | grep -Fq "../blob/ffffffffffffffffffffffffffffffffffffffff/.github/ui-evidence/pr-7/$FAKE_HEAD_SHA/room-settings.png?raw=true" || fail "comment is missing the immutable private-repository screenshot URL"

export FAKE_EVIDENCE_BRANCH_EXISTS=true
: > "$FAKE_GH_LOG"
"$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$MANIFEST"
if grep -Fq 'POST repos/test/repo/git/refs' "$FAKE_GH_LOG"; then
    fail "an existing shared evidence branch must not be recreated"
fi
[ "$(jq -r '.base_tree' "$FAKE_TREE_JSON")" = "2222222222222222222222222222222222222222" ] || fail "uploads must preserve the existing shared evidence tree"
unset FAKE_EVIDENCE_BRANCH_EXISTS

export FAKE_EXISTING_MARKER=true
: > "$FAKE_GH_LOG"
"$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$MANIFEST"
if grep -Eq '^(POST|PATCH) ' "$FAKE_GH_LOG"; then
    fail "an existing exact-head marker should prevent all GitHub mutations"
fi
unset FAKE_EXISTING_MARKER

: > "$FAKE_GH_LOG"
"$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$MANIFEST" --dry-run
if grep -Eq '^(POST|PATCH) ' "$FAKE_GH_LOG"; then
    fail "dry run should not mutate GitHub"
fi

INVALID_MANIFEST="$TMP_DIR/evidence/invalid-manifest.json"
jq '.uses_mock_data = false' "$MANIFEST" > "$INVALID_MANIFEST"
: > "$FAKE_GH_LOG"
if "$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$INVALID_MANIFEST" >/dev/null 2>&1; then
    fail "live-data manifests must be rejected"
fi
[ ! -s "$FAKE_GH_LOG" ] || fail "invalid manifests should fail before contacting GitHub"

INVALID_RENDERER_MANIFEST="$TMP_DIR/evidence/invalid-renderer-manifest.json"
jq 'del(.renderer)' "$MANIFEST" > "$INVALID_RENDERER_MANIFEST"
: > "$FAKE_GH_LOG"
if "$POSTER" --pr 7 --head "$FAKE_HEAD_SHA" --manifest "$INVALID_RENDERER_MANIFEST" >/dev/null 2>&1; then
    fail "manifests without the canonical font renderer must be rejected"
fi
[ ! -s "$FAKE_GH_LOG" ] || fail "invalid renderer manifests should fail before contacting GitHub"

STALE_HEAD="aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
: > "$FAKE_GH_LOG"
if "$POSTER" --pr 7 --head "$STALE_HEAD" --manifest "$MANIFEST" >/dev/null 2>&1; then
    fail "stale PR heads must be rejected"
fi
if grep -Eq '^(POST|PATCH) ' "$FAKE_GH_LOG"; then
    fail "stale PR heads should not mutate GitHub"
fi

echo "Flutter UI evidence posting contract passed."
