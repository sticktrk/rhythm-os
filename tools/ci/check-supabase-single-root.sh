#!/bin/bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CANONICAL_ROOT="$REPO_ROOT/tools/app/supabase"

# The private marketing checkout owns its own project. It must not leak into
# the public product tree when someone changes the ignore or checkout layout.
if [ -n "$(git -C "$REPO_ROOT" ls-files -- rhythm-marketing/supabase)" ]; then
    echo 'Error: marketing Supabase source must be tracked only in its own repository' >&2
    exit 1
fi

for required in config.toml migrations functions; do
    if [ ! -e "$CANONICAL_ROOT/$required" ]; then
        echo "Error: canonical Supabase project is missing $CANONICAL_ROOT/$required" >&2
        exit 1
    fi
done

unexpected=()
while IFS= read -r candidate; do
    [ "$candidate" = "$CANONICAL_ROOT" ] && continue
    unexpected+=("${candidate#"$REPO_ROOT/"}")
done < <(
    find "$REPO_ROOT" \
        \( -path "$REPO_ROOT/.git" \
           -o -path "$REPO_ROOT/.claude" \
           -o -path "$REPO_ROOT/.repo-git-backups" \
           -o -path "$REPO_ROOT/.repo-import-work" \
           -o -path "$REPO_ROOT/.repo-working-backups" \
           -o -path "$REPO_ROOT/.release-evidence" \
           -o -path "$REPO_ROOT/.triage" \
           -o -path "$REPO_ROOT/rhythm-marketing" \
           -o -path "$REPO_ROOT/target" \
           -o -path '*/node_modules' \
           -o -path '*/build' \
           -o -path '*/.dart_tool' \) -prune \
        -o -type d -name supabase -print
)

if [ "${#unexpected[@]}" -gt 0 ]; then
    printf 'Error: Supabase project directories must live only at tools/app/supabase/:\n' >&2
    printf '  %s\n' "${unexpected[@]}" >&2
    exit 1
fi

echo "Supabase project root is canonical: tools/app/supabase/"
