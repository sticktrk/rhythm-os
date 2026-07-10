#!/bin/bash

set -euo pipefail

BASE=""
HEAD_REF="HEAD"
OUTPUT="plain"
ALL=false

usage() {
    cat <<'EOF'
Usage: detect-changed-surfaces.sh [--base REF] [--head REF] [--all] [--github-output]

Detect which CROSS product surfaces changed. With --github-output, append
key=value pairs to $GITHUB_OUTPUT for GitHub Actions job outputs.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --base)
            BASE="${2:?--base requires a ref}"
            shift 2
            ;;
        --head)
            HEAD_REF="${2:?--head requires a ref}"
            shift 2
            ;;
        --all)
            ALL=true
            shift
            ;;
        --github-output)
            OUTPUT="github"
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage >&2
            exit 1
            ;;
    esac
done

REPO_ROOT="$(git rev-parse --show-toplevel)"
cd "$REPO_ROOT"

if [ "$ALL" = false ] && [ -z "$BASE" ]; then
    if git rev-parse --verify "${HEAD_REF}^" >/dev/null 2>&1; then
        BASE="${HEAD_REF}^"
    else
        ALL=true
    fi
fi

if [ "$ALL" = false ]; then
    case "$BASE" in
        0000000000000000000000000000000000000000)
            ALL=true
            ;;
        *)
            if ! git rev-parse --verify "$BASE" >/dev/null 2>&1; then
                ALL=true
            fi
            ;;
    esac
fi

rust=false
flutter=false
sdk=false
admin_api=false
admin_ui=false
supabase=false

mark_all() {
    rust=true
    flutter=true
    sdk=true
    admin_api=true
    admin_ui=true
    supabase=true
}

if [ "$ALL" = true ]; then
    mark_all
    BASE=""
else
    CHANGED_FILES="$(git diff --name-only "$BASE" "$HEAD_REF")"
    while IFS= read -r path; do
        [ -n "$path" ] || continue
        case "$path" in
            .github/workflows/ci.yml|Cargo.toml|Cargo.lock|.cargo/*|os/*|runtime/*|tools/os/*|app/flutter/rhythm_app/rust/*|app/flutter/rhythm_app/rust_builder/*)
                rust=true
                ;;
        esac
        case "$path" in
            app/flutter/rhythm_app/*|app/flutter/rhythm_core/*|tools/app/scripts/*|sdk/*)
                flutter=true
                ;;
        esac
        case "$path" in
            sdk/*)
                sdk=true
                admin_api=true
                ;;
        esac
        case "$path" in
            admin-api/*)
                admin_api=true
                ;;
        esac
        case "$path" in
            admin-ui/*)
                admin_ui=true
                ;;
        esac
        case "$path" in
            tools/app/supabase/*)
                supabase=true
                ;;
        esac
    done <<EOF
$CHANGED_FILES
EOF
fi

emit() {
    key="$1"
    value="$2"
    if [ "$OUTPUT" = "github" ]; then
        [ -n "${GITHUB_OUTPUT:-}" ] || {
            echo "GITHUB_OUTPUT is required with --github-output" >&2
            exit 1
        }
        printf '%s=%s\n' "$key" "$value" >> "$GITHUB_OUTPUT"
    else
        printf '%s=%s\n' "$key" "$value"
    fi
}

emit base "$BASE"
emit rust "$rust"
emit flutter "$flutter"
emit sdk "$sdk"
emit admin_api "$admin_api"
emit admin_ui "$admin_ui"
emit supabase "$supabase"
