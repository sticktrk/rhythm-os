#!/bin/bash
# Deploy one Supabase Edge Function, or the canonical function set.
#
# Usage:
#   ./tools/deploy-supabase-functions.sh report-bug
#   ./tools/deploy-supabase-functions.sh blog-post-intake
#   ./tools/deploy-supabase-functions.sh --all

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy-supabase-functions.sh [FUNCTION ... | --all] [--dry-run]

Deploy Supabase Edge Functions from the single canonical project at
tools/app/supabase/. Function authentication settings come from its config.toml.

Options:
  --all      Deploy every local function in the canonical project.
  --dry-run  Print the Supabase CLI command without running it.
  -h, --help Show this help.

Examples:
  ./tools/deploy-supabase-functions.sh report-bug
  ./tools/deploy-supabase-functions.sh blog-post-intake
  ./tools/deploy-supabase-functions.sh --all
  ./tools/deploy-supabase-functions.sh report-bug --dry-run

Authentication:
  Run `supabase login` and link the canonical workdir once, or set both
  SUPABASE_ACCESS_TOKEN and SUPABASE_PROJECT_REF (recommended for CI).

This script deliberately does not support `--prune` so a deployment cannot
silently delete a hosted function that is absent from the selected source ref.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DEPLOY_ALL=false
DRY_RUN=false
REQUESTED_FUNCTIONS=()

while [ $# -gt 0 ]; do
    case "$1" in
        --all)
            DEPLOY_ALL=true
            shift
            ;;
        --dry-run)
            DRY_RUN=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        -*)
            die "unknown option: $1"
            ;;
        *)
            REQUESTED_FUNCTIONS+=("$1")
            shift
            ;;
    esac
done

if [ "$DEPLOY_ALL" = true ] && [ "${#REQUESTED_FUNCTIONS[@]}" -gt 0 ]; then
    die "use named functions or --all, not both"
fi

if [ "$DEPLOY_ALL" = false ] && [ "${#REQUESTED_FUNCTIONS[@]}" -eq 0 ]; then
    die "name at least one function, or use --all"
fi

WORKDIR="tools/app"
FUNCTION_DIR="$REPO_ROOT/tools/app/supabase/functions"

AVAILABLE_FUNCTIONS=()
for function_path in "$FUNCTION_DIR"/*; do
    [ -d "$function_path" ] || continue
    function_name="$(basename "$function_path")"
    case "$function_name" in
        _*) continue ;;
    esac
    AVAILABLE_FUNCTIONS+=("$function_name")
done

if [ "${#AVAILABLE_FUNCTIONS[@]}" -eq 0 ]; then
    die "no functions found in $FUNCTION_DIR"
fi

if [ "$DEPLOY_ALL" = true ]; then
    REQUESTED_FUNCTIONS=("${AVAILABLE_FUNCTIONS[@]}")
else
    for function_name in "${REQUESTED_FUNCTIONS[@]}"; do
        case "$function_name" in
            _*|*/*)
                die "invalid function name: $function_name"
                ;;
        esac
        if [ ! -d "$FUNCTION_DIR/$function_name" ]; then
            die "function '$function_name' is not in the canonical project (available: ${AVAILABLE_FUNCTIONS[*]})"
        fi
    done
fi

command -v supabase >/dev/null 2>&1 || die "Supabase CLI is required: https://supabase.com/docs/guides/cli"

if [ -n "${SUPABASE_PROJECT_REF:-}" ]; then
    PROJECT_REF="$SUPABASE_PROJECT_REF"
    USE_EXPLICIT_PROJECT_REF=true
else
    USE_EXPLICIT_PROJECT_REF=false
    PROJECT_REF_FILE="$REPO_ROOT/$WORKDIR/supabase/.temp/project-ref"
    if [ ! -f "$PROJECT_REF_FILE" ]; then
        die "'$WORKDIR' is not linked; run: supabase --workdir $WORKDIR link --project-ref <project-ref>"
    fi
    PROJECT_REF="$(tr -d '[:space:]' < "$PROJECT_REF_FILE")"
    [ -n "$PROJECT_REF" ] || die "linked project ref is empty: $PROJECT_REF_FILE"
fi

COMMAND=(
    supabase
    --workdir "$WORKDIR"
    functions deploy
)
COMMAND+=("${REQUESTED_FUNCTIONS[@]}")
COMMAND+=(--use-api)
if [ "$USE_EXPLICIT_PROJECT_REF" = true ]; then
    COMMAND+=(--project-ref "$PROJECT_REF")
fi

echo "Supabase project: $PROJECT_REF"
echo "Supabase workdir: $WORKDIR"
echo "Functions:       ${REQUESTED_FUNCTIONS[*]}"
printf 'Command:         '
printf '%q ' "${COMMAND[@]}"
printf '\n'

if [ "$DRY_RUN" = true ]; then
    echo "Dry run only; nothing was deployed."
    exit 0
fi

cd "$REPO_ROOT"
"${COMMAND[@]}"
