#!/bin/bash
# Deploy pending database migrations, then every canonical Edge Function.

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy-supabase.sh [--dry-run] [--marketing-repo PATH]

Deploy the canonical Supabase project in release order:
  1. Apply pending database migrations from tools/app/supabase/migrations/.
  2. Deploy every canonical Edge Function from tools/app/supabase/functions/.

Options:
  --dry-run  Preview pending migrations and the function deployment command.
  --marketing-repo PATH  Compose the marketing history for a shared project,
                        then deploy its functions after core functions.
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
MARKETING_REPO=""
COMPOSED_WORKDIR=""
trap '[ -z "$COMPOSED_WORKDIR" ] || rm -rf "$COMPOSED_WORKDIR"' EXIT

while [ $# -gt 0 ]; do
    case "$1" in
        --marketing-repo)
            [ $# -ge 2 ] || die '--marketing-repo requires a path'
            MARKETING_REPO="$(cd "$2" && pwd)"
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

DATABASE_WORKDIR=tools/app
if [ -n "$MARKETING_REPO" ]; then
    [ -f "$MARKETING_REPO/scripts/deploy-supabase-functions.sh" ] || \
        die 'marketing repo has no function deployment wrapper'
    MARKETING_REF_FILE="$MARKETING_REPO/supabase/.temp/project-ref"
    if [ -f "$MARKETING_REF_FILE" ] && \
        [ "$(tr -d '[:space:]' < "$MARKETING_REF_FILE")" != "$PROJECT_REF" ]; then
        die 'marketing repo is linked to a different project; shared deployment refused'
    fi
    COMPOSED_WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-supabase-history.XXXXXX")"
    python3 "$SCRIPT_DIR/prepare-supabase-workdir.py" \
        --core "$REPO_ROOT/tools/app/supabase" \
        --marketing-repo "$MARKETING_REPO" \
        --destination "$COMPOSED_WORKDIR"
    DATABASE_WORKDIR="$COMPOSED_WORKDIR"
fi

DATABASE_COMMAND=(
    supabase
    --workdir "$DATABASE_WORKDIR"
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

if [ -n "$MARKETING_REPO" ]; then
    MARKETING_COMMAND=(bash "$MARKETING_REPO/scripts/deploy-supabase-functions.sh")
    if [ "$DRY_RUN" = true ]; then MARKETING_COMMAND+=(--dry-run); fi
    echo "==> Marketing Edge Functions"
    SUPABASE_PROJECT_REF="$PROJECT_REF" "${MARKETING_COMMAND[@]}"
fi

if [ "$DRY_RUN" = true ]; then
    echo "Combined dry run complete; nothing was deployed."
else
    echo "Combined Supabase deployment complete."
fi
