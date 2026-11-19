#!/usr/bin/env bash
# Pass the live contract saved by tools/ci/smoke-server.py --assistant-contract.
set -euo pipefail
root="$(cd "$(dirname "$0")/../.." && pwd)"
contract="$(cd "$(dirname "${1:?Supply the hub contract JSON}")" && pwd)/$(basename "$1")"
test -s "$contract"
export RHYTHM_ASSISTANT_CONTRACT="$contract"
(cd "$root/sdk" && dart test test/parity/assistant_contract.dart)
(cd "$root/admin-api" && dart test test/parity/assistant_contract.dart)
(cd "$root/admin-ui" && node --experimental-strip-types --test test/assistantContract.test.mjs)
(cd "$root/app/flutter/rhythm_app" && flutter test --no-pub test/parity/assistant_contract.dart)
