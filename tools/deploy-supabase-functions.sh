#!/bin/bash
# Deploy one Supabase Edge Function, or an explicitly selected function set.
#
# Usage:
#   ./tools/deploy-supabase-functions.sh app report-bug
#   ./tools/deploy-supabase-functions.sh app --all
#   ./tools/deploy-supabase-functions.sh marketing blog-post-intake

set -euo pipefail

usage() {
    cat <<'EOF'
Usage: deploy-supabase-functions.sh SCOPE [FUNCTION ... | --all] [--dry-run]

Deploy Supabase Edge Functions from the correct directory and with the
authentication settings required by that function set.

Scopes:
  app        Functions in tools/app/supabase/functions
  marketing  Functions in rhythm-marketing/supabase/functions

Options:
  --all      Deploy every local function in the selected scope.
  --dry-run  Print the Supabase CLI command without running it.
  -h, --help Show this help.

Examples:
  ./tools/deploy-supabase-functions.sh app report-bug
  ./tools/deploy-supabase-functions.sh app --all
  ./tools/deploy-supabase-functions.sh marketing blog-post-intake
  ./tools/deploy-supabase-functions.sh app report-bug --dry-run

Authentication:
  Run `supabase login` and link each workdir once, or set both
  SUPABASE_ACCESS_TOKEN and SUPABASE_PROJECT_REF (recommended for CI).

This script deliberately does not support `--prune`. Both scopes deploy to the
same Supabase project, so pruning from either local directory could delete
functions owned by the other scope.
EOF
}

die() {
    echo "ERROR: $*" >&2
    exit 1
}

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

if [ $# -eq 0 ]; then
    usage >&2
    exit 1
fi

case "$1" in
    -h|--help)
        usage
        exit 0
        ;;
    app|marketing)
        SCOPE="$1"
        shift
        ;;
    *)
        die "scope must be 'app' or 'marketing'"
        ;;
esac

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

case "$SCOPE" in
    app)
        WORKDIR="tools/app"
        FUNCTION_DIR="$REPO_ROOT/tools/app/supabase/functions"
        DISABLE_JWT=false
        ;;
    marketing)
        WORKDIR="rhythm-marketing"
        FUNCTION_DIR="$REPO_ROOT/rhythm-marketing/supabase/functions"
        DISABLE_JWT=false
        ;;
esac

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
            die "function '$function_name' is not in the '$SCOPE' scope (available: ${AVAILABLE_FUNCTIONS[*]})"
        fi
    done
fi

if [ "$SCOPE" = marketing ]; then
    for function_name in "${REQUESTED_FUNCTIONS[@]}"; do
        case "$function_name" in
            *)
                die "JWT deployment policy is not defined for marketing function '$function_name'; update this script before deploying it"
                ;;
        esac
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
if [ "$DISABLE_JWT" = true ]; then
    COMMAND+=(--no-verify-jwt)
fi
if [ "$USE_EXPLICIT_PROJECT_REF" = true ]; then
    COMMAND+=(--project-ref "$PROJECT_REF")
fi

echo "Supabase project: $PROJECT_REF"
echo "Function scope:  $SCOPE"
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
