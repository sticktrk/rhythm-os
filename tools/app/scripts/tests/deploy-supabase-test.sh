#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-supabase-combined-deploy-test.XXXXXX")"
trap 'rm -r "$TEMP_DIR"' EXIT
TEST_REPO="$TEMP_DIR/repo"
DEPLOY_SCRIPT="$TEST_REPO/tools/deploy-supabase.sh"

mkdir -p "$TEMP_DIR/bin"
mkdir -p "$TEST_REPO/tools/app/supabase/.temp"
mkdir -p "$TEST_REPO/tools/app/supabase/functions/blog-post-intake"
mkdir -p "$TEST_REPO/tools/app/supabase/functions/report-bug"

cp "$REPO_ROOT/tools/deploy-supabase.sh" "$DEPLOY_SCRIPT"
cp "$REPO_ROOT/tools/deploy-supabase-functions.sh" \
    "$TEST_REPO/tools/deploy-supabase-functions.sh"
printf '%s\n' 'test-project-ref' \
    >"$TEST_REPO/tools/app/supabase/.temp/project-ref"
printf '%s\n' '// test entrypoint' \
    >"$TEST_REPO/tools/app/supabase/functions/blog-post-intake/index.ts"
printf '%s\n' '// test entrypoint' \
    >"$TEST_REPO/tools/app/supabase/functions/report-bug/index.ts"

cat >"$TEMP_DIR/bin/supabase" <<'EOF'
#!/bin/bash
set -euo pipefail

printf '%s\n' "$*" >>"$RHYTHM_TEST_SUPABASE_LOG"

if [ "${RHYTHM_TEST_FAIL_DB_PUSH:-0}" = 1 ] &&
    [[ " $* " == *" db push "* ]]; then
    exit 23
fi
EOF
chmod +x "$TEMP_DIR/bin/supabase"

run_deploy() {
    PATH="$TEMP_DIR/bin:$PATH" \
        RHYTHM_TEST_SUPABASE_LOG="$TEMP_DIR/supabase.log" \
        SUPABASE_PROJECT_REF=test-project-ref \
        "$DEPLOY_SCRIPT" "$@"
}

: >"$TEMP_DIR/supabase.log"
deploy_output="$(run_deploy)"
deploy_call_count="$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')"
deploy_first_call="$(sed -n '1p' "$TEMP_DIR/supabase.log")"
deploy_second_call="$(sed -n '2p' "$TEMP_DIR/supabase.log")"

[[ "$deploy_call_count" -eq 2 ]]
[[ "$deploy_first_call" == "--workdir tools/app db push --linked" ]]
[[ "$deploy_second_call" == *"--workdir tools/app functions deploy"* ]]
[[ "$deploy_second_call" == *"--use-api --project-ref test-project-ref"* ]]
printf '%s\n' "$deploy_output" | grep -Fq 'Combined Supabase deployment complete.'

: >"$TEMP_DIR/supabase.log"
dry_run_output="$(run_deploy --dry-run)"
dry_run_call_count="$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')"
dry_run_first_call="$(sed -n '1p' "$TEMP_DIR/supabase.log")"

[[ "$dry_run_call_count" -eq 1 ]]
[[ "$dry_run_first_call" == "--workdir tools/app db push --linked --dry-run" ]]
printf '%s\n' "$dry_run_output" | grep -Fq 'functions deploy'
printf '%s\n' "$dry_run_output" | grep -Fq 'Dry run only; nothing was deployed.'
printf '%s\n' "$dry_run_output" | grep -Fq 'Combined dry run complete; nothing was deployed.'

: >"$TEMP_DIR/supabase.log"
set +e
PATH="$TEMP_DIR/bin:$PATH" \
    RHYTHM_TEST_SUPABASE_LOG="$TEMP_DIR/supabase.log" \
    RHYTHM_TEST_FAIL_DB_PUSH=1 \
    SUPABASE_PROJECT_REF=test-project-ref \
    "$DEPLOY_SCRIPT" >"$TEMP_DIR/failure.out" 2>&1
failure_status=$?
set -e

[[ "$failure_status" -eq 23 ]]
failure_call_count="$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')"
failure_first_call="$(sed -n '1p' "$TEMP_DIR/supabase.log")"
[[ "$failure_call_count" -eq 1 ]]
[[ "$failure_first_call" == "--workdir tools/app db push --linked" ]]
if grep -Fq 'functions deploy' "$TEMP_DIR/failure.out"; then
    echo "function deployment ran after a failed database push" >&2
    exit 1
fi

: >"$TEMP_DIR/supabase.log"
set +e
PATH="$TEMP_DIR/bin:$PATH" \
    RHYTHM_TEST_SUPABASE_LOG="$TEMP_DIR/supabase.log" \
    SUPABASE_PROJECT_REF=other-project-ref \
    "$DEPLOY_SCRIPT" >"$TEMP_DIR/mismatch.out" 2>&1
mismatch_status=$?
set -e

[[ "$mismatch_status" -eq 1 ]]
[[ ! -s "$TEMP_DIR/supabase.log" ]]
grep -Fq 'SUPABASE_PROJECT_REF does not match' "$TEMP_DIR/mismatch.out"

echo "Combined Supabase deployment wrapper contract passed."
