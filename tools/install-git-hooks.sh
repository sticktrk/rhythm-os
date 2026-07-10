#!/bin/bash

set -euo pipefail

STRICT=false
if [ "${1:-}" = "--strict" ]; then
    STRICT=true
elif [ $# -gt 0 ]; then
    echo "Usage: $0 [--strict]" >&2
    exit 1
fi

REPO_ROOT="$(git rev-parse --show-toplevel)"
git -C "$REPO_ROOT" config core.hooksPath .githooks

if [ "$STRICT" = true ]; then
    git -C "$REPO_ROOT" config rhythm.strictHooks true
else
    git -C "$REPO_ROOT" config rhythm.strictHooks false
fi

echo "Installed CROSS hooks from .githooks (strict=$STRICT)."
echo "Hooks always block deterministic invariant failures and only warn on commit-message/test heuristics by default."
