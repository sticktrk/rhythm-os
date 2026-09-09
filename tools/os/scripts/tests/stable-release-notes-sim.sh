#!/bin/bash
# Compatibility test entrypoint for the shared beta/stable notes generator.
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
exec python3 "$ROOT/tools/tests/test_release_notes.py"
