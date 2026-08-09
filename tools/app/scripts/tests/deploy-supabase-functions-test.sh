#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
DEPLOY_SCRIPT="$REPO_ROOT/tools/deploy-supabase-functions.sh"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-supabase-deploy-test.XXXXXX")"
trap 'rm -r "$TEMP_DIR"' EXIT

mkdir -p "$TEMP_DIR/bin"
ln -s "$(command -v true)" "$TEMP_DIR/bin/supabase"

run_dry() {
    PATH="$TEMP_DIR/bin:$PATH" \
        SUPABASE_PROJECT_REF=test-project-ref \
        "$DEPLOY_SCRIPT" "$@" --dry-run
}

named_output="$(run_dry report-bug blog-post-intake)"
printf '%s\n' "$named_output" | grep -Fq 'Supabase workdir: tools/app'
printf '%s\n' "$named_output" | grep -Eq 'functions deploy .*report-bug .*blog-post-intake .*--use-api .*--project-ref test-project-ref'
if printf '%s\n' "$named_output" | grep -q -- '--prune\|--no-verify-jwt'; then
    echo "named deployment bypassed canonical config or enabled pruning" >&2
    exit 1
fi

all_output="$(run_dry --all)"
printf '%s\n' "$all_output" | grep -Fq 'blog-post-intake'
printf '%s\n' "$all_output" | grep -Fq 'report-bug'
if printf '%s\n' "$all_output" | grep -Fq '_shared'; then
    echo "--all attempted to deploy the shared source directory" >&2
    exit 1
fi
if printf '%s\n' "$all_output" | grep -Fq 'join-home-by-server-instance'; then
    echo "--all attempted to deploy a directory without an index.ts entrypoint" >&2
    exit 1
fi

if run_dry app report-bug >"$TEMP_DIR/old-scope.out" 2>&1; then
    echo "legacy split-workdir scope was unexpectedly accepted" >&2
    exit 1
fi

if run_dry --prune >"$TEMP_DIR/prune.out" 2>&1; then
    echo "--prune was unexpectedly accepted" >&2
    exit 1
fi

echo "Supabase function deployment wrapper contract passed."
