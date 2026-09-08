#!/bin/bash
set -euo pipefail
REPO_ROOT="$(git rev-parse --show-toplevel)"
git -C "$REPO_ROOT" config core.hooksPath .githooks
echo "Installed local source-check hooks from .githooks."
