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
mkdir -p "$TEST_REPO/tools/app/supabase/functions/delete-user"
mkdir -p "$TEST_REPO/tools/app/supabase/functions/report-bug"

cp "$REPO_ROOT/tools/deploy-supabase.sh" "$DEPLOY_SCRIPT"
cp "$REPO_ROOT/tools/deploy-supabase-functions.sh" \
    "$TEST_REPO/tools/deploy-supabase-functions.sh"
printf '%s\n' 'test-project-ref' \
    >"$TEST_REPO/tools/app/supabase/.temp/project-ref"
printf '%s\n' '// test entrypoint' \
    >"$TEST_REPO/tools/app/supabase/functions/delete-user/index.ts"
printf '%s\n' '// test entrypoint' \
    >"$TEST_REPO/tools/app/supabase/functions/report-bug/index.ts"

cat >"$TEMP_DIR/bin/supabase" <<'EOF'
#!/bin/bash
set -euo pipefail

printf '%s\n' "$*" >>"$RHYTHM_TEST_SUPABASE_LOG"
if [ "${RHYTHM_TEST_EXPECT_COMPOSED:-0}" = 1 ] && [[ " $* " == *" db push "* ]]; then
    [ -f "$2/supabase/migrations/20260101000000_product.sql" ]
    [ -f "$2/supabase/migrations/20260102000000_marketing.sql" ]
    [ ! -f "$2/supabase/.env" ]
fi

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

# Shared history must be composed before any hosted mutation.
cp "$REPO_ROOT/tools/prepare-supabase-workdir.py" "$TEST_REPO/tools/"
mkdir -p "$TEST_REPO/tools/app/supabase/migrations" "$TEST_REPO/marketing/supabase/migrations" "$TEST_REPO/marketing/scripts"
printf '%s\n' '# fixture config' >"$TEST_REPO/tools/app/supabase/config.toml"
printf '%s\n' '# fixture config' >"$TEST_REPO/marketing/supabase/config.toml"
printf '%s\n' 'SELECT 1;' >"$TEST_REPO/tools/app/supabase/migrations/20260101000000_product.sql"
printf '%s\n' 'SELECT 2;' >"$TEST_REPO/marketing/supabase/migrations/20260102000000_marketing.sql"
printf '%s\n' 'SECRET=must-not-copy' >"$TEST_REPO/tools/app/supabase/.env"
cat >"$TEST_REPO/marketing/scripts/deploy-supabase-functions.sh" <<'EOF'
#!/bin/bash
set -euo pipefail
if [ "${1:-}" = --dry-run ]; then echo 'marketing function dry run'; exit 0; fi
supabase functions deploy blog-post-intake --project-ref "$SUPABASE_PROJECT_REF"
EOF
: >"$TEMP_DIR/supabase.log"
shared_output="$(RHYTHM_TEST_EXPECT_COMPOSED=1 run_deploy --marketing-repo "$TEST_REPO/marketing")"
[[ "$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')" -eq 3 ]]
shared_workdir="$(sed -n '1s/^--workdir \(.*\) db push --linked$/\1/p' "$TEMP_DIR/supabase.log")"
[[ -n "$shared_workdir" && ! -e "$shared_workdir" ]]
sed -n '3p' "$TEMP_DIR/supabase.log" | grep -Fq 'functions deploy blog-post-intake --project-ref test-project-ref'
: >"$TEMP_DIR/supabase.log"
RHYTHM_TEST_EXPECT_COMPOSED=1 run_deploy --marketing-repo "$TEST_REPO/marketing" --dry-run >"$TEMP_DIR/shared-dry.out"
[[ "$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')" -eq 1 ]]
grep -Fq 'marketing function dry run' "$TEMP_DIR/shared-dry.out"

# A failed shared migration push must not deploy either repository's functions.
: >"$TEMP_DIR/supabase.log"
if RHYTHM_TEST_FAIL_DB_PUSH=1 run_deploy --marketing-repo "$TEST_REPO/marketing" >"$TEMP_DIR/shared-failure.out" 2>&1; then
    echo 'shared database failure was ignored' >&2; exit 1
fi
[[ "$(wc -l <"$TEMP_DIR/supabase.log" | tr -d '[:space:]')" -eq 1 ]]

: >"$TEMP_DIR/supabase.log"
printf '%s\n' 'SELECT 3;' >"$TEST_REPO/marketing/supabase/migrations/20260101000000_duplicate.sql"
if run_deploy --marketing-repo "$TEST_REPO/marketing" >"$TEMP_DIR/duplicate.out" 2>&1; then
    echo 'duplicate versions were accepted' >&2; exit 1
fi
[[ ! -s "$TEMP_DIR/supabase.log" ]]
grep -Fq 'Duplicate migration version' "$TEMP_DIR/duplicate.out"
rm "$TEST_REPO/marketing/supabase/migrations/20260101000000_duplicate.sql"

mkdir -p "$TEST_REPO/marketing/supabase/.temp"
printf '%s\n' 'different-project' >"$TEST_REPO/marketing/supabase/.temp/project-ref"
if run_deploy --marketing-repo "$TEST_REPO/marketing" >"$TEMP_DIR/marketing-mismatch.out" 2>&1; then
    echo 'mismatched marketing link was accepted' >&2; exit 1
fi
[[ ! -s "$TEMP_DIR/supabase.log" ]]
grep -Fq 'linked to a different project' "$TEMP_DIR/marketing-mismatch.out"

echo "Combined Supabase deployment wrapper contract passed."
