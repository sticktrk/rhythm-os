#!/bin/bash
# Write an immutable local receipt mapping a source commit and artifact digest
# to the store build identifier that was uploaded.

set -euo pipefail

STORE=""
CHANNEL=""
VERSION=""
BUILD_NUMBER=""
SOURCE_COMMIT=""
ARTIFACT=""
OUTPUT=""

usage() {
    cat <<'EOF'
Usage: write-app-build-receipt.sh \
  --store STORE --channel CHANNEL --version VERSION --build-number NUMBER \
  --commit SHA --artifact FILE --output FILE
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --store) STORE="${2:?--store requires a value}"; shift 2 ;;
        --channel) CHANNEL="${2:?--channel requires a value}"; shift 2 ;;
        --version) VERSION="${2:?--version requires a value}"; shift 2 ;;
        --build-number) BUILD_NUMBER="${2:?--build-number requires a value}"; shift 2 ;;
        --commit) SOURCE_COMMIT="${2:?--commit requires a SHA}"; shift 2 ;;
        --artifact) ARTIFACT="${2:?--artifact requires a file}"; shift 2 ;;
        --output) OUTPUT="${2:?--output requires a file}"; shift 2 ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Error: unknown option: $1" >&2; usage >&2; exit 1 ;;
    esac
done

if [ -z "$STORE" ] || [ -z "$CHANNEL" ] || [ -z "$VERSION" ] || \
   [ -z "$BUILD_NUMBER" ] || [ -z "$SOURCE_COMMIT" ] || \
   [ -z "$ARTIFACT" ] || [ -z "$OUTPUT" ]; then
    usage >&2
    exit 1
fi
if ! [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
    echo "Error: version must use X.Y.Z format." >&2
    exit 1
fi
if ! [[ "$BUILD_NUMBER" =~ ^[0-9]+$ ]]; then
    echo "Error: build number must be numeric." >&2
    exit 1
fi
if ! [[ "$SOURCE_COMMIT" =~ ^[0-9a-f]{40}$ ]]; then
    echo "Error: commit must be a full lowercase SHA." >&2
    exit 1
fi
if [ ! -f "$ARTIFACT" ]; then
    echo "Error: artifact not found: $ARTIFACT" >&2
    exit 1
fi

for required_command in jq shasum; do
    command -v "$required_command" >/dev/null 2>&1 || {
        echo "Error: required command not found: $required_command" >&2
        exit 1
    }
done

ARTIFACT_SHA256="$(shasum -a 256 "$ARTIFACT" | awk '{print $1}')"
ARTIFACT_NAME="$(basename "$ARTIFACT")"
UPLOADED_AT="$(date -u '+%Y-%m-%dT%H:%M:%SZ')"
mkdir -p "$(dirname "$OUTPUT")"
umask 077
TEMP_OUTPUT="$OUTPUT.tmp.$$"
trap 'rm -f "$TEMP_OUTPUT"' EXIT

jq -n \
    --arg store "$STORE" \
    --arg channel "$CHANNEL" \
    --arg version "$VERSION" \
    --arg build_number "$BUILD_NUMBER" \
    --arg source_commit "$SOURCE_COMMIT" \
    --arg artifact_name "$ARTIFACT_NAME" \
    --arg artifact_sha256 "$ARTIFACT_SHA256" \
    --arg uploaded_at "$UPLOADED_AT" \
    '{
        status: "uploaded",
        store: $store,
        channel: $channel,
        version: $version,
        build_number: $build_number,
        source_commit: $source_commit,
        artifact_name: $artifact_name,
        artifact_sha256: $artifact_sha256,
        uploaded_at: $uploaded_at
    }' > "$TEMP_OUTPUT"
mv "$TEMP_OUTPUT" "$OUTPUT"
trap - EXIT
echo "build_receipt=$OUTPUT"
