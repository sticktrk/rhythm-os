#!/bin/bash

set -euo pipefail

PR_NUMBER=""
HEAD_SHA=""
MANIFEST=""
DRY_RUN=false
REPOSITORY="${CROSS_GITHUB_REPOSITORY:-sticktrk/cross}"
MAX_SCREENSHOTS=6
MAX_SCREENSHOT_BYTES=$((5 * 1024 * 1024))
MAX_TOTAL_BYTES=$((20 * 1024 * 1024))

usage() {
    cat <<'EOF'
Usage: post-flutter-ui-evidence.sh --pr NUMBER --head FULL_SHA --manifest PATH [--dry-run]

Validate deterministic Flutter screenshots, store them on the shared,
non-merged GitHub evidence branch, and add an exact-head PR comment with inline
images.

Manifest schema:
{
  "schema_version": 1,
  "uses_mock_data": true,
  "source_test": "test/visual/example_test.dart",
  "screenshots": [
    {"path": "screenshots/example.png", "caption": "Example state"}
  ]
}
EOF
}

fail() {
    echo "FLUTTER UI EVIDENCE: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --pr)
            PR_NUMBER="${2:?--pr requires a number}"
            shift 2
            ;;
        --head)
            HEAD_SHA="${2:?--head requires a full commit SHA}"
            shift 2
            ;;
        --manifest)
            MANIFEST="${2:?--manifest requires a path}"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            fail "unknown option: $1"
            ;;
    esac
done

case "$PR_NUMBER" in
    ''|*[!0-9]*) fail "--pr must be a positive integer" ;;
esac
[ "$PR_NUMBER" -gt 0 ] || fail "--pr must be a positive integer"

HEAD_SHA="$(printf '%s' "$HEAD_SHA" | tr '[:upper:]' '[:lower:]')"
printf '%s' "$HEAD_SHA" | grep -Eq '^[0-9a-f]{40}$' || fail "--head must be a full 40-character commit SHA"
printf '%s' "$REPOSITORY" | grep -Eq '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$' || fail "invalid CROSS_GITHUB_REPOSITORY"

for command in git gh jq base64 od file; do
    command -v "$command" >/dev/null 2>&1 || fail "required command is unavailable: $command"
done

[ -f "$MANIFEST" ] || fail "manifest does not exist: $MANIFEST"
MANIFEST_DIR="$(cd "$(dirname "$MANIFEST")" && pwd -P)"
MANIFEST="$MANIFEST_DIR/$(basename "$MANIFEST")"

REPO_ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || fail "run from a git worktree"
cd "$REPO_ROOT"
[ -x "$REPO_ROOT/tools/ci/detect-changed-surfaces.sh" ] || fail "missing executable surface detector"

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cross-flutter-ui-evidence.XXXXXX")"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

jq -e '
    .schema_version == 1 and
    .uses_mock_data == true and
    (.source_test | type == "string" and length > 0 and length <= 240) and
    (.screenshots | type == "array" and length > 0 and length <= 6) and
    all(.screenshots[];
        type == "object" and
        (.path | type == "string" and length > 0 and length <= 240) and
        (.caption | type == "string" and length > 0 and length <= 120)
    )
' "$MANIFEST" >/dev/null || fail "manifest must use schema version 1, declare mock data, name its source test, and contain 1-$MAX_SCREENSHOTS screenshots"

SOURCE_TEST="$(jq -r '.source_test' "$MANIFEST")"
printf '%s' "$SOURCE_TEST" | grep -Eq '^[A-Za-z0-9._/-]+$' || fail "source_test must be a repository-relative test path"

SCREENSHOT_COUNT="$(jq -r '.screenshots | length' "$MANIFEST")"
SCREENSHOT_ROWS="$TMP_DIR/screenshots.tsv"
: > "$SCREENSHOT_ROWS"
TOTAL_BYTES=0
INDEX=0
while [ "$INDEX" -lt "$SCREENSHOT_COUNT" ]; do
    RELATIVE_PATH="$(jq -r ".screenshots[$INDEX].path" "$MANIFEST")"
    CAPTION="$(jq -r ".screenshots[$INDEX].caption" "$MANIFEST")"

    case "$RELATIVE_PATH" in
        /*|..|../*|*/..|*/../*) fail "screenshot path must remain below the manifest directory: $RELATIVE_PATH" ;;
    esac
    printf '%s' "$RELATIVE_PATH" | grep -Eq '^[A-Za-z0-9._/-]+$' || fail "screenshot path contains unsupported characters: $RELATIVE_PATH"
    if printf '%s' "$CAPTION" | LC_ALL=C grep -q '[[:cntrl:]]'; then
        fail "screenshot caption contains a control character"
    fi

    SCREENSHOT_CANDIDATE="$MANIFEST_DIR/$RELATIVE_PATH"
    [ -f "$SCREENSHOT_CANDIDATE" ] || fail "screenshot does not exist: $RELATIVE_PATH"
    [ ! -L "$SCREENSHOT_CANDIDATE" ] || fail "screenshot may not be a symlink: $RELATIVE_PATH"
    SCREENSHOT_PARENT="$(cd "$(dirname "$SCREENSHOT_CANDIDATE")" && pwd -P)"
    case "$SCREENSHOT_PARENT/" in
        "$MANIFEST_DIR/"|"$MANIFEST_DIR/"*) ;;
        *) fail "screenshot resolves outside the manifest directory: $RELATIVE_PATH" ;;
    esac
    SCREENSHOT="$SCREENSHOT_PARENT/$(basename "$SCREENSHOT_CANDIDATE")"
    FILE_NAME="$(basename "$SCREENSHOT")"
    printf '%s' "$FILE_NAME" | grep -Eq '^[A-Za-z0-9._-]+\.png$' || fail "screenshot must have a safe .png filename: $FILE_NAME"
    if cut -f2 "$SCREENSHOT_ROWS" | grep -Fxq "$FILE_NAME"; then
        fail "screenshot filenames must be unique: $FILE_NAME"
    fi

    SIGNATURE="$(od -An -tx1 -N8 "$SCREENSHOT" | tr -d ' \n')"
    [ "$SIGNATURE" = "89504e470d0a1a0a" ] || fail "screenshot is not a PNG: $RELATIVE_PATH"
    [ "$(file --mime-type -b "$SCREENSHOT")" = "image/png" ] || fail "screenshot is not a readable PNG: $RELATIVE_PATH"
    FILE_BYTES="$(wc -c < "$SCREENSHOT" | tr -d ' ')"
    [ "$FILE_BYTES" -le "$MAX_SCREENSHOT_BYTES" ] || fail "screenshot exceeds 5 MiB: $RELATIVE_PATH"
    TOTAL_BYTES=$((TOTAL_BYTES + FILE_BYTES))
    [ "$TOTAL_BYTES" -le "$MAX_TOTAL_BYTES" ] || fail "screenshots exceed the 20 MiB total limit"

    printf '%s\t%s\t%s\n' "$SCREENSHOT" "$FILE_NAME" "$CAPTION" >> "$SCREENSHOT_ROWS"
    INDEX=$((INDEX + 1))
done

PR_JSON="$TMP_DIR/pr.json"
gh api "repos/$REPOSITORY/pulls/$PR_NUMBER" > "$PR_JSON"
PR_STATE="$(jq -r '.state // empty' "$PR_JSON")"
LIVE_HEAD="$(jq -r '.head.sha // empty' "$PR_JSON" | tr '[:upper:]' '[:lower:]')"
BASE_SHA="$(jq -r '.base.sha // empty' "$PR_JSON" | tr '[:upper:]' '[:lower:]')"
[ "$PR_STATE" = "open" ] || fail "PR #$PR_NUMBER is not open"
[ "$LIVE_HEAD" = "$HEAD_SHA" ] || fail "PR #$PR_NUMBER head is $LIVE_HEAD, not requested head $HEAD_SHA"
printf '%s' "$BASE_SHA" | grep -Eq '^[0-9a-f]{40}$' || fail "PR #$PR_NUMBER has no valid base SHA"
if jq -e '.labels[]?.name == "automation-hold"' "$PR_JSON" >/dev/null; then
    fail "PR #$PR_NUMBER has automation-hold"
fi

MARKER="<!-- codex-cross-flutter-ui pr=$PR_NUMBER head=$HEAD_SHA -->"
COMMENTS_JSON="$TMP_DIR/comments.json"
gh api --paginate "repos/$REPOSITORY/issues/$PR_NUMBER/comments?per_page=100" | jq -s 'add // []' > "$COMMENTS_JSON"
if jq -e --arg marker "$MARKER" 'any(.[]; (.body // "") | contains($marker))' "$COMMENTS_JSON" >/dev/null; then
    echo "Flutter UI evidence already exists for PR #$PR_NUMBER at $HEAD_SHA."
    exit 0
fi

ensure_commit() {
    local sha="$1"
    if ! git cat-file -e "$sha^{commit}" 2>/dev/null; then
        git fetch --quiet origin "$sha"
    fi
    [ "$(git rev-parse "$sha^{commit}")" = "$sha" ] || fail "could not resolve exact commit $sha"
}

ensure_commit "$BASE_SHA"
ensure_commit "$HEAD_SHA"
SURFACES="$TMP_DIR/surfaces.txt"
"$REPO_ROOT/tools/ci/detect-changed-surfaces.sh" --base "$BASE_SHA" --head "$HEAD_SHA" > "$SURFACES"
grep -Fxq 'flutter_ui=true' "$SURFACES" || fail "PR #$PR_NUMBER does not change a recognized Flutter UI surface"

if [ "$DRY_RUN" = true ]; then
    echo "Validated $SCREENSHOT_COUNT mock-data Flutter screenshot(s) from $SOURCE_TEST for PR #$PR_NUMBER at $HEAD_SHA."
    echo "Dry run: no evidence branch or PR comment was changed."
    exit 0
fi

DEFAULT_BRANCH="$(gh api "repos/$REPOSITORY" --jq '.default_branch')"
[ -n "$DEFAULT_BRANCH" ] || fail "could not resolve the repository default branch"
EVIDENCE_BRANCH="codex-ui-evidence"
DEFAULT_REF_JSON="$TMP_DIR/default-ref.json"
gh api "repos/$REPOSITORY/git/ref/heads/$DEFAULT_BRANCH" > "$DEFAULT_REF_JSON"
DEFAULT_COMMIT="$(jq -r '.object.sha // empty' "$DEFAULT_REF_JSON")"
printf '%s' "$DEFAULT_COMMIT" | grep -Eq '^[0-9a-f]{40}$' || fail "default branch ref has no valid commit"

EVIDENCE_REF_JSON="$TMP_DIR/evidence-ref.json"
if gh api "repos/$REPOSITORY/git/ref/heads/$EVIDENCE_BRANCH" > "$EVIDENCE_REF_JSON" 2>/dev/null; then
    PARENT_COMMIT="$(jq -r '.object.sha // empty' "$EVIDENCE_REF_JSON")"
else
    CREATE_REF_JSON="$TMP_DIR/create-ref.json"
    jq -n --arg ref "refs/heads/$EVIDENCE_BRANCH" --arg sha "$DEFAULT_COMMIT" '{ref: $ref, sha: $sha}' > "$CREATE_REF_JSON"
    gh api --method POST "repos/$REPOSITORY/git/refs" --input "$CREATE_REF_JSON" > "$EVIDENCE_REF_JSON"
    PARENT_COMMIT="$DEFAULT_COMMIT"
fi
printf '%s' "$PARENT_COMMIT" | grep -Eq '^[0-9a-f]{40}$' || fail "evidence branch ref has no valid commit"

PARENT_COMMIT_JSON="$TMP_DIR/parent-commit.json"
gh api "repos/$REPOSITORY/git/commits/$PARENT_COMMIT" > "$PARENT_COMMIT_JSON"
PARENT_TREE="$(jq -r '.tree.sha // empty' "$PARENT_COMMIT_JSON")"
printf '%s' "$PARENT_TREE" | grep -Eq '^[0-9a-f]{40}$' || fail "evidence branch parent has no valid tree"

REMOTE_ROOT=".github/ui-evidence/pr-$PR_NUMBER/$HEAD_SHA"
TREE_ENTRIES="$TMP_DIR/tree-entries.jsonl"
REMOTE_ROWS="$TMP_DIR/remote.tsv"
: > "$TREE_ENTRIES"
: > "$REMOTE_ROWS"
while IFS="$(printf '\t')" read -r SCREENSHOT FILE_NAME CAPTION; do
    BLOB_PAYLOAD="$TMP_DIR/blob-$FILE_NAME.json"
    base64 < "$SCREENSHOT" | tr -d '\n' | jq -Rs '{content: ., encoding: "base64"}' > "$BLOB_PAYLOAD"
    BLOB_SHA="$(gh api --method POST "repos/$REPOSITORY/git/blobs" --input "$BLOB_PAYLOAD" --jq '.sha')"
    printf '%s' "$BLOB_SHA" | grep -Eq '^[0-9a-f]{40}$' || fail "GitHub returned an invalid blob SHA for $FILE_NAME"
    REMOTE_PATH="$REMOTE_ROOT/$FILE_NAME"
    jq -n --arg path "$REMOTE_PATH" --arg sha "$BLOB_SHA" '{path: $path, mode: "100644", type: "blob", sha: $sha}' >> "$TREE_ENTRIES"
    printf '%s\t%s\n' "$REMOTE_PATH" "$CAPTION" >> "$REMOTE_ROWS"
done < "$SCREENSHOT_ROWS"

EVIDENCE_MANIFEST="$TMP_DIR/evidence-manifest.json"
jq --arg repository "$REPOSITORY" --argjson pr "$PR_NUMBER" --arg head "$HEAD_SHA" '
    {
        schema_version: 1,
        repository: $repository,
        pull_request: $pr,
        head: $head,
        uses_mock_data: true,
        source_test: .source_test,
        screenshots: [.screenshots[] | {file: (.path | split("/")[-1]), caption: .caption}]
    }
' "$MANIFEST" > "$EVIDENCE_MANIFEST"
MANIFEST_PAYLOAD="$TMP_DIR/manifest-blob.json"
base64 < "$EVIDENCE_MANIFEST" | tr -d '\n' | jq -Rs '{content: ., encoding: "base64"}' > "$MANIFEST_PAYLOAD"
MANIFEST_BLOB="$(gh api --method POST "repos/$REPOSITORY/git/blobs" --input "$MANIFEST_PAYLOAD" --jq '.sha')"
printf '%s' "$MANIFEST_BLOB" | grep -Eq '^[0-9a-f]{40}$' || fail "GitHub returned an invalid manifest blob SHA"
jq -n --arg path "$REMOTE_ROOT/manifest.json" --arg sha "$MANIFEST_BLOB" '{path: $path, mode: "100644", type: "blob", sha: $sha}' >> "$TREE_ENTRIES"

TREE_PAYLOAD="$TMP_DIR/tree.json"
jq -s --arg base_tree "$PARENT_TREE" '{base_tree: $base_tree, tree: .}' "$TREE_ENTRIES" > "$TREE_PAYLOAD"
TREE_SHA="$(gh api --method POST "repos/$REPOSITORY/git/trees" --input "$TREE_PAYLOAD" --jq '.sha')"
printf '%s' "$TREE_SHA" | grep -Eq '^[0-9a-f]{40}$' || fail "GitHub returned an invalid tree SHA"

COMMIT_PAYLOAD="$TMP_DIR/commit.json"
jq -n \
    --arg message "Flutter UI evidence for PR #$PR_NUMBER at $HEAD_SHA" \
    --arg tree "$TREE_SHA" \
    --arg parent "$PARENT_COMMIT" \
    '{message: $message, tree: $tree, parents: [$parent]}' > "$COMMIT_PAYLOAD"
EVIDENCE_COMMIT="$(gh api --method POST "repos/$REPOSITORY/git/commits" --input "$COMMIT_PAYLOAD" --jq '.sha')"
printf '%s' "$EVIDENCE_COMMIT" | grep -Eq '^[0-9a-f]{40}$' || fail "GitHub returned an invalid evidence commit SHA"

UPDATE_REF_PAYLOAD="$TMP_DIR/update-ref.json"
jq -n --arg sha "$EVIDENCE_COMMIT" '{sha: $sha, force: false}' > "$UPDATE_REF_PAYLOAD"
gh api --method PATCH "repos/$REPOSITORY/git/refs/heads/$EVIDENCE_BRANCH" --input "$UPDATE_REF_PAYLOAD" >/dev/null

COMMENT_BODY="$TMP_DIR/comment.md"
{
    printf '%s\n\n' "$MARKER"
    printf '### Flutter UI evidence\n\n'
    printf 'Rendered from deterministic mock/test data by `%s` for exact head `%s`. No live customer, home, or device data is included.\n\n' "$SOURCE_TEST" "$HEAD_SHA"
    while IFS="$(printf '\t')" read -r REMOTE_PATH CAPTION; do
        IMAGE_URL="../blob/$EVIDENCE_COMMIT/$REMOTE_PATH?raw=true"
        printf '**%s**\n\n![%s](%s)\n\n' "$CAPTION" "$CAPTION" "$IMAGE_URL"
    done < "$REMOTE_ROWS"
} > "$COMMENT_BODY"
COMMENT_PAYLOAD="$TMP_DIR/comment.json"
jq -Rs '{body: .}' < "$COMMENT_BODY" > "$COMMENT_PAYLOAD"
gh api --method POST "repos/$REPOSITORY/issues/$PR_NUMBER/comments" --input "$COMMENT_PAYLOAD" >/dev/null

echo "Posted $SCREENSHOT_COUNT mock-data Flutter screenshot(s) for PR #$PR_NUMBER at $HEAD_SHA."
