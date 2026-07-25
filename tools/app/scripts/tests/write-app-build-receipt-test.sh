#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
WRITER="$SCRIPT_DIR/../write-app-build-receipt.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-app-build-receipt-test.XXXXXX")"
cleanup() {
    rm -rf "$TEST_ROOT"
}
trap cleanup EXIT

ARTIFACT="$TEST_ROOT/Rhythm.ipa"
RECEIPT="$TEST_ROOT/receipts/apple.json"
printf 'test artifact\n' > "$ARTIFACT"

"$WRITER" \
    --store app-store-connect \
    --channel testflight \
    --version 4.0.303 \
    --build-number 3 \
    --commit 53033dd7b3e41d6b693a68bf39e77e02bbc44dec \
    --artifact "$ARTIFACT" \
    --output "$RECEIPT"

jq -e '
    .status == "uploaded" and
    .store == "app-store-connect" and
    .channel == "testflight" and
    .version == "4.0.303" and
    .build_number == "3" and
    .source_commit == "53033dd7b3e41d6b693a68bf39e77e02bbc44dec" and
    (.artifact_sha256 | test("^[0-9a-f]{64}$"))
' "$RECEIPT" >/dev/null
test "$(stat -f '%Lp' "$RECEIPT")" = "600"

if "$WRITER" \
    --store app-store-connect \
    --channel testflight \
    --version 4.0 \
    --build-number x \
    --commit short \
    --artifact "$ARTIFACT" \
    --output "$TEST_ROOT/invalid.json"; then
    echo "invalid receipt input should fail" >&2
    exit 1
fi

echo "write-app-build-receipt tests passed"
