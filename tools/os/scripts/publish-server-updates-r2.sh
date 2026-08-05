#!/usr/bin/env bash
# Publish a packaged OTA feed to Cloudflare R2.
#
# Versioned objects are immutable: an existing key must carry the same sha256
# metadata or the publish fails. Mutable aliases are uploaded next, and each
# manifest is uploaded last so a device never observes references to objects
# that have not finished uploading.

set -euo pipefail

SOURCE_DIR=""
BUCKET="${RHYTHM_R2_BUCKET:-${CLOUDFLARE_R2_BUCKET:-}}"
PREFIX="${RHYTHM_R2_PREFIX:-server}"
ENDPOINT="${RHYTHM_R2_ENDPOINT:-${CLOUDFLARE_S3_API_ENDPOINT:-}}"
DRY_RUN=false

usage() {
    cat <<EOF
Usage: $0 --source-dir PATH [OPTIONS]

Publish a packaged server update tree to Cloudflare R2.

Options:
  --source-dir PATH  Directory containing feed directories (required)
  --bucket NAME      R2 bucket (default: CLOUDFLARE_R2_BUCKET)
  --prefix PREFIX    Object key prefix (default: RHYTHM_R2_PREFIX or server)
  --endpoint URL     R2 S3 endpoint (default: CLOUDFLARE_S3_API_ENDPOINT)
  --dry-run          Print uploads without changing R2
  -h, --help         Show this help

AWS_ACCESS_KEY_ID and AWS_SECRET_ACCESS_KEY must contain an R2 API token's
S3 credentials. The token needs Object Read & Write access to the bucket.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --source-dir)
            SOURCE_DIR="${2:?--source-dir requires a value}"
            shift 2
            ;;
        --bucket)
            BUCKET="${2:?--bucket requires a value}"
            shift 2
            ;;
        --prefix)
            PREFIX="${2:?--prefix requires a value}"
            shift 2
            ;;
        --endpoint)
            ENDPOINT="${2:?--endpoint requires a value}"
            shift 2
            ;;
        --dry-run)
            DRY_RUN=true
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

require_command() {
    command -v "$1" >/dev/null 2>&1 || {
        echo "Error: Required command not found: $1" >&2
        exit 1
    }
}

sha256_file() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

PREFIX="${PREFIX#/}"
PREFIX="${PREFIX%/}"

if [ -z "$SOURCE_DIR" ] || [ ! -d "$SOURCE_DIR" ]; then
    echo "Error: --source-dir must name an existing directory" >&2
    exit 1
fi
if [ -z "$BUCKET" ]; then
    echo "Error: --bucket or CLOUDFLARE_R2_BUCKET is required" >&2
    exit 1
fi
if [ -z "$PREFIX" ]; then
    echo "Error: refusing to publish with an empty object prefix" >&2
    exit 1
fi
if [ -z "$ENDPOINT" ]; then
    echo "Error: --endpoint or CLOUDFLARE_S3_API_ENDPOINT is required" >&2
    exit 1
fi

require_command aws
require_command jq

SOURCE_DIR="$(cd "$SOURCE_DIR" && pwd)"
AWS_ARGS=(--endpoint-url "$ENDPOINT" --no-cli-pager)

object_key_for_file() {
    local file="$1"
    local relative="${file#"$SOURCE_DIR"/}"
    printf '%s/%s\n' "$PREFIX" "$relative"
}

object_exists() {
    local key="$1"
    local result

    result="$(aws "${AWS_ARGS[@]}" s3api list-objects-v2 \
        --bucket "$BUCKET" \
        --prefix "$key" \
        --max-keys 1 \
        --output json)"
    jq -e --arg key "$key" '.Contents[]? | select(.Key == $key)' \
        >/dev/null <<<"$result"
}

remote_sha256() {
    local key="$1"
    aws "${AWS_ARGS[@]}" s3api head-object \
        --bucket "$BUCKET" \
        --key "$key" \
        --query 'Metadata.sha256' \
        --output text
}

content_type_for_file() {
    case "$1" in
        *.json) echo application/json ;;
        *.tar.gz|*.tgz|*.gz) echo application/gzip ;;
        *) echo application/octet-stream ;;
    esac
}

upload_file() {
    local file="$1"
    local cache_control="$2"
    local immutable="$3"
    local key sha existing_sha content_type

    key="$(object_key_for_file "$file")"
    sha="$(sha256_file "$file")"
    content_type="$(content_type_for_file "$file")"

    if [ "$immutable" = true ] && object_exists "$key"; then
        existing_sha="$(remote_sha256 "$key")"
        if [ "$existing_sha" = "$sha" ]; then
            echo "Keeping immutable s3://$BUCKET/$key (sha256 already matches)"
            return
        fi
        echo "Error: refusing to overwrite immutable s3://$BUCKET/$key" >&2
        echo "Expected sha256 $sha, found metadata sha256 ${existing_sha:-missing}" >&2
        exit 1
    fi

    if [ "$DRY_RUN" = true ]; then
        echo "[dry-run] Upload $file -> s3://$BUCKET/$key ($cache_control)"
        return
    fi

    aws "${AWS_ARGS[@]}" s3 cp "$file" "s3://$BUCKET/$key" \
        --no-progress \
        --only-show-errors \
        --cache-control "$cache_control" \
        --content-type "$content_type" \
        --metadata "sha256=$sha"

    existing_sha="$(remote_sha256 "$key")"
    if [ "$existing_sha" != "$sha" ]; then
        echo "Error: R2 metadata verification failed for s3://$BUCKET/$key" >&2
        exit 1
    fi
    echo "Published s3://$BUCKET/$key"
}

manifest_count="$(find "$SOURCE_DIR" -type f -name manifest.json | wc -l | tr -d ' ')"
if [ "$manifest_count" -eq 0 ]; then
    echo "Error: no manifest.json found under $SOURCE_DIR" >&2
    exit 1
fi

# Upload payloads before mutable aliases. Version paths are content-addressed
# by the manifest's sha256 and may never be changed in place.
while IFS= read -r file; do
    [ -n "$file" ] || continue
    relative="${file#"$SOURCE_DIR"/}"
    case "$relative" in
        */v[0-9]*/*)
            upload_file "$file" 'public, max-age=31536000, immutable' true
            ;;
    esac
done < <(find "$SOURCE_DIR" -type f ! -name manifest.json | LC_ALL=C sort)

while IFS= read -r file; do
    [ -n "$file" ] || continue
    relative="${file#"$SOURCE_DIR"/}"
    case "$relative" in
        */v[0-9]*/*) ;;
        *) upload_file "$file" 'no-cache, must-revalidate' false ;;
    esac
done < <(find "$SOURCE_DIR" -type f ! -name manifest.json | LC_ALL=C sort)

# The manifest is the fleet's OTA authority and is always committed last.
while IFS= read -r file; do
    [ -n "$file" ] || continue
    upload_file "$file" 'no-cache, must-revalidate' false
done < <(find "$SOURCE_DIR" -type f -name manifest.json | LC_ALL=C sort)

echo "Published $manifest_count OTA feed manifest(s) to R2"
