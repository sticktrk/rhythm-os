#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
DEPLOY_SCRIPT="$REPO_ROOT/tools/deploy-supabase-functions.sh"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-supabase-deploy-test.XXXXXX")"
trap 'rm -r "$TEMP_DIR"' EXIT

mkdir -p "$TEMP_DIR/bin"
for utility in dirname basename; do
    utility_path="$(type -P "$utility")"
    [ -n "$utility_path" ] || {
        echo "required test utility is unavailable: $utility" >&2
        exit 1
    }
    ln -s "$utility_path" "$TEMP_DIR/bin/$utility"
done

run_dry() {
    # Prove dry-run command validation works without a Supabase CLI in PATH.
    PATH="$TEMP_DIR/bin" \
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

if PATH="$TEMP_DIR/bin" SUPABASE_PROJECT_REF=test-project-ref \
    "$DEPLOY_SCRIPT" report-bug >"$TEMP_DIR/missing-cli.out" 2>&1; then
    echo "real deployment unexpectedly accepted a missing Supabase CLI" >&2
    exit 1
fi
grep -Fq 'Supabase CLI is required' "$TEMP_DIR/missing-cli.out"

echo "Supabase function deployment wrapper contract passed."
