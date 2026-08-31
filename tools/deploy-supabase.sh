#!/bin/bash
# Deploy pending database migrations, then every canonical Edge Function.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy-supabase.sh [--dry-run]

Deploy the canonical Supabase project in release order:
  1. Apply pending database migrations from tools/app/supabase/migrations/.
  2. Deploy every canonical Edge Function from tools/app/supabase/functions/.

Options:
  --dry-run  Preview pending migrations and the function deployment command.
  -h, --help Show this help.

Authentication:
  Run `supabase login` and link the canonical workdir once:
  `supabase --workdir tools/app link --project-ref <project-ref>`.

The stages fail fast. If the database push fails, functions are not deployed.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

DRY_RUN=false

while [ $# -gt 0 ]; do
    case "$1" in
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            die "unknown argument: $1"
            ;;
    esac
done

command -v supabase >/dev/null 2>&1 || \
    die "Supabase CLI is required: https://supabase.com/docs/guides/cli"

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROJECT_REF_FILE="$REPO_ROOT/tools/app/supabase/.temp/project-ref"

if [ ! -f "$PROJECT_REF_FILE" ]; then
    die "'tools/app' is not linked; run: supabase --workdir tools/app link --project-ref <project-ref>"
fi

PROJECT_REF="$(tr -d '[:space:]' < "$PROJECT_REF_FILE")"
[ -n "$PROJECT_REF" ] || die "linked project ref is empty: $PROJECT_REF_FILE"

if [ -n "${SUPABASE_PROJECT_REF:-}" ] && [ "$SUPABASE_PROJECT_REF" != "$PROJECT_REF" ]; then
    die "SUPABASE_PROJECT_REF does not match the project linked at tools/app"
fi

DATABASE_COMMAND=(
    supabase
    --workdir tools/app
    db push
    --linked
)
FUNCTION_COMMAND=(
    env
    "SUPABASE_PROJECT_REF=$PROJECT_REF"
    "$SCRIPT_DIR/deploy-supabase-functions.sh"
    --all
)

if [ "$DRY_RUN" = true ]; then
    DATABASE_COMMAND+=(--dry-run)
    FUNCTION_COMMAND+=(--dry-run)
fi

cd "$REPO_ROOT"

echo "Supabase project: $PROJECT_REF"
echo "==> Database migrations"
"${DATABASE_COMMAND[@]}"

echo "==> Edge Functions"
"${FUNCTION_COMMAND[@]}"

if [ "$DRY_RUN" = true ]; then
    echo "Combined dry run complete; nothing was deployed."
else
    echo "Combined Supabase deployment complete."
fi
