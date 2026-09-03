#!/bin/bash

set -euo pipefail

# Git hooks export repository-local variables such as GIT_DIR. Clear them before
# creating the disposable fixture repository so its commits cannot touch the
# checkout that invoked the hook.
while IFS= read -r git_var; do
    [ -n "$git_var" ] && unset "$git_var"
done < <(git rev-parse --local-env-vars)

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/cross-surface-detector.XXXXXX")"
cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

fail() {
    echo "SURFACE DETECTOR TEST: $*" >&2
    exit 1
}

assert_output() {
    local output="$1"
    local expected="$2"
    printf '%s\n' "$output" | grep -Fxq "$expected" || fail "missing output '$expected'"
}

cd "$TMP_DIR"
git init -q
git config user.name "Surface Detector Test"
git config user.email "surface-detector@example.invalid"
mkdir -p tools/ci
cp "$SCRIPT_DIR/detect-changed-surfaces.sh" tools/ci/detect-changed-surfaces.sh
chmod +x tools/ci/detect-changed-surfaces.sh
printf 'baseline\n' > README.md
git add README.md tools/ci/detect-changed-surfaces.sh
git commit -qm baseline

mkdir -p app/flutter/rhythm_app/lib/providers
printf 'class ExampleProvider {}\n' > app/flutter/rhythm_app/lib/providers/example_provider.dart
git add app/flutter/rhythm_app/lib/providers/example_provider.dart
git commit -qm provider
OUTPUT="$(tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD)"
assert_output "$OUTPUT" 'flutter=true'
assert_output "$OUTPUT" 'flutter_ui=false'

mkdir -p app/flutter/rhythm_app/lib/widgets
printf 'class ExampleWidget {}\n' > app/flutter/rhythm_app/lib/widgets/example_widget.dart
git add app/flutter/rhythm_app/lib/widgets/example_widget.dart
git commit -qm widget
OUTPUT="$(tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD)"
assert_output "$OUTPUT" 'flutter=true'
assert_output "$OUTPUT" 'flutter_ui=true'

mkdir -p os/rust/integrations/rhythm-matter/src
printf 'pub struct Profile;\n' > os/rust/integrations/rhythm-matter/src/profile.rs
git add os/rust/integrations/rhythm-matter/src/profile.rs
git commit -qm matter
OUTPUT="$(tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD)"
assert_output "$OUTPUT" 'rust=true'
assert_output "$OUTPUT" 'matter=true'

mkdir -p app/flutter/rhythm_app/lib/onboarding/screens
printf 'class WelcomeScreen {}\n' > app/flutter/rhythm_app/lib/onboarding/screens/welcome_screen.dart
git add app/flutter/rhythm_app/lib/onboarding/screens/welcome_screen.dart
git commit -qm onboarding
OUTPUT="$(tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD)"
assert_output "$OUTPUT" 'flutter_ui=true'

mkdir -p app/flutter/rhythm_app/assets/images
printf 'asset\n' > app/flutter/rhythm_app/assets/images/example.txt
git add app/flutter/rhythm_app/assets/images/example.txt
git commit -qm asset
OUTPUT="$(tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD)"
assert_output "$OUTPUT" 'flutter_ui=true'

OUTPUT="$(tools/ci/detect-changed-surfaces.sh --all)"
assert_output "$OUTPUT" 'flutter_ui=true'
assert_output "$OUTPUT" 'matter=true'

GITHUB_OUTPUT="$TMP_DIR/github-output.txt"
export GITHUB_OUTPUT
tools/ci/detect-changed-surfaces.sh --base HEAD^ --head HEAD --github-output
grep -Fxq 'flutter_ui=true' "$GITHUB_OUTPUT" || fail "GitHub output is missing flutter_ui=true"

echo "Changed-surface detection contract passed."
