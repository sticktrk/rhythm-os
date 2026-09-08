#!/bin/bash
# Deterministic contract test for stable release note comparison boundaries.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GENERATOR="$SCRIPT_DIR/../generate-stable-release-notes.sh"
TEST_ROOT="$(mktemp -d)"
trap 'rm -rf "$TEST_ROOT"' EXIT

# Git exports repository-local environment variables to hooks. Clear them
# before initializing the disposable repository so this simulation cannot
# write its fixture commits or tags into the caller's worktree.
while IFS= read -r git_local_env_var; do
    unset "$git_local_env_var"
done < <(git rev-parse --local-env-vars)

REPO="$TEST_ROOT/repo"
BIN="$TEST_ROOT/bin"
GH_ARGS="$TEST_ROOT/gh-args"
NOTES="$TEST_ROOT/stable-notes.md"
mkdir -p "$REPO/tools/os/scripts" "$BIN"
cp "$GENERATOR" "$REPO/tools/os/scripts/generate-stable-release-notes.sh"

git -C "$REPO" init -q
git -C "$REPO" config user.name "Release Test"
git -C "$REPO" config user.email "fixture@example.invalid"
touch "$REPO/history"
git -C "$REPO" add history
git -C "$REPO" commit -qm "initial"
git -C "$REPO" tag -a v0.6.100-stable -m "old stable"
printf 'beta\n' >> "$REPO/history"
git -C "$REPO" add history
git -C "$REPO" commit -qm "beta one"
git -C "$REPO" tag -a v0.6.101-beta -m "beta"
printf 'stable\n' >> "$REPO/history"
git -C "$REPO" add history
git -C "$REPO" commit -qm "beta two"
git -C "$REPO" tag -a v0.6.102-beta -m "beta"
git -C "$REPO" tag -a v0.6.102-stable -m "current stable"

cat > "$BIN/gh" <<'EOF'
#!/bin/bash
printf '%s\n' "$@" > "$GH_ARGS"
printf '%s\n' '## What'\''s Changed' '* Added a useful improvement in https://example.test/pr/1'
EOF
chmod +x "$BIN/gh"

GH_ARGS="$GH_ARGS" \
GITHUB_REPOSITORY="sticktrk/rhythm-os" \
PATH="$BIN:$PATH" \
    "$REPO/tools/os/scripts/generate-stable-release-notes.sh" \
        v0.6.102-stable "$NOTES"

grep -Fx 'previous_tag_name=v0.6.100-stable' "$GH_ARGS" >/dev/null
grep -F 'every merged pull request since [v0.6.100-stable]' "$NOTES" >/dev/null
grep -F '## What'"'"'s Changed' "$NOTES" >/dev/null
if grep -F 'previous_tag_name=v0.6.102-beta' "$GH_ARGS" >/dev/null; then
    echo "Error: beta tag was used as the stable comparison boundary" >&2
    exit 1
fi

echo "Stable release note comparison simulation passed."
