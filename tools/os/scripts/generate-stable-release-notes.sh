#!/bin/bash
# Compatibility entrypoint; beta and stable notes share one implementation.
set -euo pipefail
if [[ $# -lt 2 || ! "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+-stable$ ]]; then
    echo "Usage: $0 CURRENT_STABLE_TAG OUTPUT_FILE [release-note options]" >&2
    exit 1
fi
exec python3 "$(dirname "${BASH_SOURCE[0]}")/generate-release-notes.py" "$@"
