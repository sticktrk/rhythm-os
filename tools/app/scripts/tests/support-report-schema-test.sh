#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../../.." && pwd)"
MIGRATION="$REPO_ROOT/tools/app/supabase/migrations/20260809040000_allow_pending_support_bundle_size.sql"
COMPLETION="$REPO_ROOT/tools/app/supabase/functions/report-bug-bundle-complete/index.ts"

grep -Eq 'ALTER COLUMN bundle_size_bytes DROP NOT NULL' "$MIGRATION"
grep -Eq "bundle_status <> 'uploaded' OR bundle_size_bytes > 0" "$MIGRATION"
grep -Eq 'if \(!fileName \|\| sizeBytes == null\)' "$COMPLETION"

echo "Pending support reports allow null bundle size while uploaded reports require a positive size."
