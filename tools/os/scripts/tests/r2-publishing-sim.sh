#!/usr/bin/env bash
# Contract test for R2 publish ordering, immutability, cache metadata, and
# manifest-aware retention. Uses a fake AWS CLI; no network is accessed.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PUBLISHER="$SCRIPT_DIR/../publish-server-updates-r2.sh"
PRUNER="$SCRIPT_DIR/../prune-r2-releases.sh"
TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-r2-publish-test.XXXXXX")"
trap 'rm -rf "$TEMP_DIR"' EXIT

mkdir -p "$TEMP_DIR/bin" "$TEMP_DIR/fake/remote" "$TEMP_DIR/feed/rpiz/v1.2.3-beta"
touch "$TEMP_DIR/fake/objects" "$TEMP_DIR/fake/metadata" "$TEMP_DIR/fake/calls"

cat > "$TEMP_DIR/bin/aws" <<'FAKE_AWS'
#!/usr/bin/env bash
set -euo pipefail

printf '%s\n' "$*" >> "$FAKE_R2_ROOT/calls"

if [ "${1:-}" = "--endpoint-url" ]; then shift 2; fi
if [ "${1:-}" = "--no-cli-pager" ]; then shift; fi

value_after() {
    local wanted="$1"
    shift
    while [ $# -gt 0 ]; do
        if [ "$1" = "$wanted" ]; then
            printf '%s\n' "$2"
            return
        fi
        shift
    done
}

file_sha256() {
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "$1" | awk '{print $1}'
    else
        shasum -a 256 "$1" | awk '{print $1}'
    fi
}

replace_metadata() {
    local key="$1"
    local sha="$2"
    awk -F '\t' -v key="$key" '$1 != key' "$FAKE_R2_ROOT/metadata" > "$FAKE_R2_ROOT/metadata.new"
    printf '%s\t%s\n' "$key" "$sha" >> "$FAKE_R2_ROOT/metadata.new"
    mv "$FAKE_R2_ROOT/metadata.new" "$FAKE_R2_ROOT/metadata"
}

case "${1:-} ${2:-}" in
    "s3api list-objects-v2")
        prefix="$(value_after --prefix "$@")"
        jq -Rn --arg prefix "$prefix" \
            '[inputs | select(length > 0 and startswith($prefix)) | {Key: .}] | {Contents: .}' \
            < "$FAKE_R2_ROOT/objects"
        ;;
    "s3api head-object")
        key="$(value_after --key "$@")"
        sha="$(awk -F '\t' -v key="$key" '$1 == key { print $2; exit }' "$FAKE_R2_ROOT/metadata")"
        [ -n "$sha" ] || exit 1
        printf '%s\n' "$sha"
        ;;
    "s3api get-object")
        key="$(value_after --key "$@")"
        destination="${!#}"
        source_path="$FAKE_R2_ROOT/remote/$key"
        [ -f "$source_path" ] || exit 1
        cp "$source_path" "$destination"
        etag="\"$(file_sha256 "$source_path")\""
        jq -n --arg etag "$etag" '{ETag: $etag}'
        ;;
    "s3api put-object")
        if printf '%s\n' "$*" | grep -q -- '--generate-cli-skeleton input'; then
            jq -n '{IfMatch: "", IfNoneMatch: ""}'
            exit 0
        fi
        key="$(value_after --key "$@")"
        source_path="$(value_after --body "$@")"
        destination="$FAKE_R2_ROOT/remote/$key"
        if [ -n "${FAKE_R2_CONCURRENT_MANIFEST:-}" ] && [[ "$key" == */manifest.json ]]; then
            mkdir -p "$(dirname "$destination")"
            cp "$FAKE_R2_CONCURRENT_MANIFEST" "$destination"
        fi
        if_match="$(value_after --if-match "$@")"
        if_none_match="$(value_after --if-none-match "$@")"
        if [ -f "$destination" ]; then
            current_etag="\"$(file_sha256 "$destination")\""
        else
            current_etag=""
        fi
        if [ -n "$if_match" ] && [ "$if_match" != "$current_etag" ]; then
            exit 1
        fi
        if [ "$if_none_match" = "*" ] && [ -f "$destination" ]; then
            exit 1
        fi
        mkdir -p "$(dirname "$destination")"
        cp "$source_path" "$destination"
        grep -Fqx "$key" "$FAKE_R2_ROOT/objects" || printf '%s\n' "$key" >> "$FAKE_R2_ROOT/objects"
        metadata="$(value_after --metadata "$@")"
        replace_metadata "$key" "${metadata#sha256=}"
        jq -n '{ETag: "fake"}'
        ;;
    "s3 head-bucket")
        ;;
    "s3 cp")
        source_path="$3"
        destination="$4"
        if [[ "$source_path" == s3://* ]]; then
            key="${source_path#s3://*/}"
            cp "$FAKE_R2_ROOT/remote/$key" "$destination"
            exit 0
        fi
        key="${destination#s3://*/}"
        mkdir -p "$FAKE_R2_ROOT/remote/$(dirname "$key")"
        cp "$source_path" "$FAKE_R2_ROOT/remote/$key"
        grep -Fqx "$key" "$FAKE_R2_ROOT/objects" || printf '%s\n' "$key" >> "$FAKE_R2_ROOT/objects"
        metadata="$(value_after --metadata "$@")"
        replace_metadata "$key" "${metadata#sha256=}"
        ;;
    "s3 rm")
        ;;
    *)
        echo "Unexpected fake aws call: $*" >&2
        exit 1
        ;;
esac
FAKE_AWS
chmod +x "$TEMP_DIR/bin/aws"

printf 'versioned payload\n' > "$TEMP_DIR/feed/rpiz/v1.2.3-beta/rhythm-server-rpiz.tar.gz"
printf 'production factory image\n' > "$TEMP_DIR/feed/sdcard.img.gz"
cat > "$TEMP_DIR/feed/rpiz/manifest.json" <<'JSON'
{
  "version": "1.2.3-beta",
  "package": {"url": "v1.2.3-beta/rhythm-server-rpiz.tar.gz"},
  "images": []
}
JSON

export PATH="$TEMP_DIR/bin:$PATH"
export FAKE_R2_ROOT="$TEMP_DIR/fake"
export AWS_ACCESS_KEY_ID=test
export AWS_SECRET_ACCESS_KEY=test
export CLOUDFLARE_S3_API_ENDPOINT=https://test.r2.cloudflarestorage.com
export CLOUDFLARE_R2_BUCKET=rhythm-updates-test

"$PUBLISHER" --source-dir "$TEMP_DIR/feed"

versioned_line="$(grep -n 's3 cp .*v1.2.3-beta/rhythm-server-rpiz.tar.gz' "$TEMP_DIR/fake/calls" | tail -1 | cut -d: -f1)"
factory_line="$(grep -n 's3 cp .*sdcard.img.gz' "$TEMP_DIR/fake/calls" | tail -1 | cut -d: -f1)"
manifest_line="$(grep -n 's3api put-object .*manifest.json' "$TEMP_DIR/fake/calls" | tail -1 | cut -d: -f1)"
[ "$versioned_line" -lt "$factory_line" ]
[ "$factory_line" -lt "$manifest_line" ]
grep -q 'v1.2.3-beta/rhythm-server-rpiz.tar.gz.*public, max-age=31536000, immutable' "$TEMP_DIR/fake/calls"
grep -q 'sdcard.img.gz.*no-cache, must-revalidate' "$TEMP_DIR/fake/calls"
grep -q 'manifest.json.*no-cache, must-revalidate' "$TEMP_DIR/fake/calls"

# An idempotent rerun keeps the immutable object and republishes mutable keys.
: > "$TEMP_DIR/fake/calls"
"$PUBLISHER" --source-dir "$TEMP_DIR/feed"
if grep -q 's3 cp .*v1.2.3-beta/rhythm-server-rpiz.tar.gz' "$TEMP_DIR/fake/calls"; then
    echo "immutable payload was unexpectedly re-uploaded" >&2
    exit 1
fi

# Reusing a version key with different bytes is a hard failure.
printf 'changed payload\n' > "$TEMP_DIR/feed/rpiz/v1.2.3-beta/rhythm-server-rpiz.tar.gz"
if "$PUBLISHER" --source-dir "$TEMP_DIR/feed" >"$TEMP_DIR/mismatch.out" 2>&1; then
    echo "immutable mismatch unexpectedly succeeded" >&2
    exit 1
fi
grep -q 'refusing to overwrite immutable' "$TEMP_DIR/mismatch.out"

# A stale rerun cannot roll a feed manifest back to an older version.
printf 'versioned payload\n' > "$TEMP_DIR/feed/rpiz/v1.2.3-beta/rhythm-server-rpiz.tar.gz"
mkdir -p "$TEMP_DIR/feed/rpiz/v1.2.4-beta"
printf 'newer payload\n' > "$TEMP_DIR/feed/rpiz/v1.2.4-beta/rhythm-server-rpiz.tar.gz"
jq '.version = "1.2.4-beta" | .package.url = "v1.2.4-beta/rhythm-server-rpiz.tar.gz"' \
    "$TEMP_DIR/feed/rpiz/manifest.json" > "$TEMP_DIR/feed/rpiz/manifest.new"
mv "$TEMP_DIR/feed/rpiz/manifest.new" "$TEMP_DIR/feed/rpiz/manifest.json"
"$PUBLISHER" --source-dir "$TEMP_DIR/feed" >/dev/null

jq '.version = "1.2.3-beta" | .package.url = "v1.2.3-beta/rhythm-server-rpiz.tar.gz"' \
    "$TEMP_DIR/feed/rpiz/manifest.json" > "$TEMP_DIR/feed/rpiz/manifest.new"
mv "$TEMP_DIR/feed/rpiz/manifest.new" "$TEMP_DIR/feed/rpiz/manifest.json"
printf 'stale production factory image\n' > "$TEMP_DIR/feed/sdcard.img.gz"
if "$PUBLISHER" --source-dir "$TEMP_DIR/feed" >"$TEMP_DIR/stale-manifest.out" 2>&1; then
    echo "stale manifest publish unexpectedly succeeded" >&2
    exit 1
fi
grep -q 'refusing to replace newer manifest' "$TEMP_DIR/stale-manifest.out"
jq -e '.version == "1.2.4-beta"' "$TEMP_DIR/fake/remote/server/rpiz/manifest.json" >/dev/null
grep -q '^production factory image$' "$TEMP_DIR/fake/remote/server/sdcard.img.gz"

# A concurrent manifest change invalidates the conditional commit instead of
# being overwritten by a publisher that inspected an older object.
jq '.version = "1.2.5-beta" | .package.url = "v1.2.4-beta/rhythm-server-rpiz.tar.gz"' \
    "$TEMP_DIR/feed/rpiz/manifest.json" > "$TEMP_DIR/feed/rpiz/manifest.new"
mv "$TEMP_DIR/feed/rpiz/manifest.new" "$TEMP_DIR/feed/rpiz/manifest.json"
jq '.version = "1.2.6-beta"' "$TEMP_DIR/feed/rpiz/manifest.json" > "$TEMP_DIR/concurrent-manifest.json"
export FAKE_R2_CONCURRENT_MANIFEST="$TEMP_DIR/concurrent-manifest.json"
if "$PUBLISHER" --source-dir "$TEMP_DIR/feed" >"$TEMP_DIR/concurrent-manifest.out" 2>&1; then
    echo "concurrent manifest overwrite unexpectedly succeeded" >&2
    exit 1
fi
unset FAKE_R2_CONCURRENT_MANIFEST
grep -q 'manifest changed concurrently' "$TEMP_DIR/concurrent-manifest.out"
jq -e '.version == "1.2.6-beta"' "$TEMP_DIR/fake/remote/server/rpiz/manifest.json" >/dev/null

# Populate seven releases and protect the oldest through the live manifest.
: > "$TEMP_DIR/fake/objects"
: > "$TEMP_DIR/fake/metadata"
: > "$TEMP_DIR/fake/calls"
for version in 1 2 3 4 5 6 7; do
    key="server/rpiz/v1.0.${version}-beta/payload.tar.gz"
    printf '%s\n' "$key" >> "$TEMP_DIR/fake/objects"
    mkdir -p "$TEMP_DIR/fake/remote/$(dirname "$key")"
    printf 'payload %s\n' "$version" > "$TEMP_DIR/fake/remote/$key"
done
mkdir -p "$TEMP_DIR/fake/remote/server/rpiz" "$TEMP_DIR/fake/remote/server/rpiz-stable"
cat > "$TEMP_DIR/fake/remote/server/rpiz/manifest.json" <<'JSON'
{"package":{"url":"v1.0.7-beta/payload.tar.gz"},"images":[{"url":"v1.0.1-beta/rootfs.ext2.gz"}]}
JSON
cat > "$TEMP_DIR/fake/remote/server/rpiz-stable/manifest.json" <<'JSON'
{"package":{"url":"v1.0.1-stable/payload.tar.gz"},"images":[]}
JSON

"$PRUNER" --keep 2
grep -q 's3 rm s3://rhythm-updates-test/server/rpiz/v1.0.2-beta/' "$TEMP_DIR/fake/calls"
grep -q 's3 rm s3://rhythm-updates-test/server/rpiz/v1.0.5-beta/' "$TEMP_DIR/fake/calls"
if grep -q 's3 rm s3://rhythm-updates-test/server/rpiz/v1.0.1-beta/' "$TEMP_DIR/fake/calls"; then
    echo "manifest-referenced release was unexpectedly pruned" >&2
    exit 1
fi
if grep -q 's3 rm s3://rhythm-updates-test/server/rpiz/v1.0.[67]-beta/' "$TEMP_DIR/fake/calls"; then
    echo "retained release was unexpectedly pruned" >&2
    exit 1
fi

# Retention fails closed before deleting anything if either live manifest is
# unavailable or malformed, because that manifest may protect an old release.
for version in 1 2 3; do
    key="server/rpiz-stable/v1.0.${version}-stable/payload.tar.gz"
    printf '%s\n' "$key" >> "$TEMP_DIR/fake/objects"
    mkdir -p "$TEMP_DIR/fake/remote/$(dirname "$key")"
    printf 'stable payload %s\n' "$version" > "$TEMP_DIR/fake/remote/$key"
done

: > "$TEMP_DIR/fake/calls"
rm "$TEMP_DIR/fake/remote/server/rpiz-stable/manifest.json"
if "$PRUNER" --keep 2 >"$TEMP_DIR/missing-manifest.out" 2>&1; then
    echo "pruning unexpectedly succeeded without a live manifest" >&2
    exit 1
fi
grep -q 'refusing to prune without live manifest' "$TEMP_DIR/missing-manifest.out"
if grep -q 's3 rm ' "$TEMP_DIR/fake/calls"; then
    echo "objects were deleted without a complete manifest protection set" >&2
    exit 1
fi

: > "$TEMP_DIR/fake/calls"
printf '{not-json\n' > "$TEMP_DIR/fake/remote/server/rpiz-stable/manifest.json"
if "$PRUNER" --keep 2 >"$TEMP_DIR/invalid-manifest.out" 2>&1; then
    echo "pruning unexpectedly succeeded with an invalid live manifest" >&2
    exit 1
fi
grep -q 'refusing to prune with invalid live manifest' "$TEMP_DIR/invalid-manifest.out"
if grep -q 's3 rm ' "$TEMP_DIR/fake/calls"; then
    echo "objects were deleted with an invalid manifest protection set" >&2
    exit 1
fi

echo "R2 publishing simulation passed"
