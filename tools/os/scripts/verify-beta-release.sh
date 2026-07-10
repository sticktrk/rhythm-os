#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"

# shellcheck source=lib/version.sh
source "$SCRIPT_DIR/lib/version.sh"

VERSION=""
REMOTE="origin"
UPDATES_BASE_URL="${RHYTHM_UPDATES_PUBLIC_BASE_URL:-https://dl.rhythm.lighting/server}"
DEVICE=""
TOKEN_FILE=""
DEVICE_TOKEN="${RHYTHM_DEVICE_TOKEN:-}"
SCENARIO="smoke"
RECEIPT=""
WAIT_SECONDS=0
POLL_SECONDS=10
DRY_RUN=false
JOURNEY_ID="${RHYTHM_JOURNEY_ID:-field-$(date -u +%Y%m%dT%H%M%SZ)-$$}"

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Verify that a beta tag is published and optionally prove the exact package on
a live rpiz. Writes a local JSON receipt without storing credentials.

Options:
  --version VERSION       X.Y.Z, vX.Y.Z, or beta/stable-suffixed form.
                          Defaults to the latest local beta tag.
  --remote NAME           Git remote containing the beta tag (default: origin).
  --updates-base-url URL  OTA base URL (default: $UPDATES_BASE_URL).
  --device URL            Device base URL, for example http://192.168.1.20:54448.
  --token-file FILE       Read the device bearer token from FILE.
                          RHYTHM_DEVICE_TOKEN is also supported.
  --scenario NAME         smoke, state, onboarding, or ota (default: smoke).
  --journey-id ID         Reuse this value as X-Request-Id across device checks.
  --receipt FILE          Receipt path (default: .release-evidence/vX.Y.Z-beta.json).
  --wait-seconds N        Poll for the public manifest for up to N seconds.
  --poll-seconds N        Poll interval, 1-60 seconds (default: 10).
  --dry-run               Print the verification plan without network calls.
  -h, --help              Show this help.
EOF
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            VERSION="${2:?--version requires a value}"
            shift 2
            ;;
        --remote)
            REMOTE="${2:?--remote requires a value}"
            shift 2
            ;;
        --updates-base-url)
            UPDATES_BASE_URL="${2:?--updates-base-url requires a value}"
            shift 2
            ;;
        --device)
            DEVICE="${2:?--device requires a value}"
            shift 2
            ;;
        --token-file)
            TOKEN_FILE="${2:?--token-file requires a value}"
            shift 2
            ;;
        --scenario)
            SCENARIO="${2:?--scenario requires a value}"
            shift 2
            ;;
        --journey-id)
            JOURNEY_ID="${2:?--journey-id requires a value}"
            shift 2
            ;;
        --receipt)
            RECEIPT="${2:?--receipt requires a value}"
            shift 2
            ;;
        --wait-seconds)
            WAIT_SECONDS="${2:?--wait-seconds requires a value}"
            shift 2
            ;;
        --poll-seconds)
            POLL_SECONDS="${2:?--poll-seconds requires a value}"
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

remote_tag_commit() {
    tag="$1"
    commit="$(git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" "refs/tags/$tag^{}" 2>/dev/null \
        | awk 'NR == 1 { print $1 }')"
    if [ -n "$commit" ]; then
        printf '%s\n' "$commit"
        return
    fi
    git -C "$REPO_ROOT" ls-remote --tags "$REMOTE" "refs/tags/$tag" 2>/dev/null \
        | awk 'NR == 1 { print $1 }'
}

require_command git
require_command curl
require_command jq

case "$SCENARIO" in
    smoke|state|onboarding|ota)
        ;;
    *)
        echo "Error: --scenario must be smoke, state, onboarding, or ota" >&2
        exit 1
        ;;
esac

if ! printf '%s' "$WAIT_SECONDS" | grep -Eq '^[0-9]+$' \
    || ! printf '%s' "$POLL_SECONDS" | grep -Eq '^[0-9]+$' \
    || [ "$POLL_SECONDS" -lt 1 ] \
    || [ "$POLL_SECONDS" -gt 60 ]; then
    echo "Error: wait/poll seconds must be integers and poll must be between 1 and 60" >&2
    exit 1
fi

if [ -z "$VERSION" ]; then
    BETA_TAG="$(git -C "$REPO_ROOT" tag --list 'v[0-9]*-beta' --sort=-version:refname | head -n 1)"
    [ -n "$BETA_TAG" ] || {
        echo "Error: no local beta tag found" >&2
        exit 1
    }
    CORE_VERSION="$(semver_core "$BETA_TAG")"
else
    CORE_VERSION="$(semver_core "$VERSION")"
    BETA_TAG="v${CORE_VERSION}-beta"
fi

EXPECTED_VERSION="${CORE_VERSION}-beta"
LOCAL_COMMIT="$(git -C "$REPO_ROOT" rev-list -n 1 "$BETA_TAG" 2>/dev/null || true)"
[ -n "$LOCAL_COMMIT" ] || {
    echo "Error: local beta tag not found: $BETA_TAG" >&2
    exit 1
}

if [ -z "$RECEIPT" ]; then
    RECEIPT="$REPO_ROOT/.release-evidence/${BETA_TAG}.json"
elif [ "${RECEIPT#/}" = "$RECEIPT" ]; then
    RECEIPT="$REPO_ROOT/$RECEIPT"
fi

if [ -n "$TOKEN_FILE" ]; then
    [ -r "$TOKEN_FILE" ] || {
        echo "Error: token file is not readable: $TOKEN_FILE" >&2
        exit 1
    }
    DEVICE_TOKEN="$(sed -n '1p' "$TOKEN_FILE")"
fi

DEVICE="${DEVICE%/}"
MANIFEST_URL="${UPDATES_BASE_URL%/}/rpiz/manifest.json"

echo "Beta verification plan"
echo "  Tag:      $BETA_TAG ($LOCAL_COMMIT)"
echo "  Manifest: $MANIFEST_URL"
echo "  Device:   ${DEVICE:-(feed only)}"
echo "  Scenario: $SCENARIO"
echo "  Journey:  $JOURNEY_ID"
echo "  Receipt:  $RECEIPT"

if [ "$DRY_RUN" = true ]; then
    exit 0
fi

REMOTE_COMMIT="$(remote_tag_commit "$BETA_TAG")"
[ -n "$REMOTE_COMMIT" ] || {
    echo "Error: beta tag is not present on $REMOTE: $BETA_TAG" >&2
    exit 1
}
if [ "$REMOTE_COMMIT" != "$LOCAL_COMMIT" ]; then
    echo "Error: local and remote beta tags differ ($LOCAL_COMMIT != $REMOTE_COMMIT)" >&2
    exit 1
fi

TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-beta-verify.XXXXXX")"
trap 'rm -rf "$TMP_DIR"' EXIT
MANIFEST_FILE="$TMP_DIR/manifest.json"
DEADLINE=$(( $(date +%s) + WAIT_SECONDS ))

while true; do
    cache_buster="verify=$(date +%s)"
    if curl -fsSL --max-time 30 "${MANIFEST_URL}?${cache_buster}" -o "$MANIFEST_FILE" \
        && jq -e . "$MANIFEST_FILE" >/dev/null 2>&1 \
        && [ "$(jq -r '.version // empty' "$MANIFEST_FILE")" = "$EXPECTED_VERSION" ] \
        && [ -n "$(jq -r '.package.url // empty' "$MANIFEST_FILE")" ]; then
        break
    fi

    if [ "$(date +%s)" -ge "$DEADLINE" ]; then
        published_version="$(jq -r '.version // "unavailable"' "$MANIFEST_FILE" 2>/dev/null || echo unavailable)"
        echo "Error: public beta manifest is not ready for $EXPECTED_VERSION (found $published_version)" >&2
        exit 1
    fi
    sleep "$POLL_SECONDS"
done

WARNINGS_JSON='[]'
DEVICE_JSON='null'

request_json() {
    endpoint="$1"
    output="$2"
    curl_args=(
        -sS
        --max-time 30
        -o "$output"
        -w '%{http_code}'
        -H "X-Request-Id: $JOURNEY_ID"
    )
    if [ -n "$DEVICE_TOKEN" ]; then
        curl_args+=( -H "Authorization: Bearer $DEVICE_TOKEN" )
    fi
    code="$(curl "${curl_args[@]}" "$DEVICE$endpoint")"
    case "$code" in
        2??)
            ;;
        *)
            echo "Error: device request $endpoint returned HTTP $code" >&2
            exit 1
            ;;
    esac
    jq -e . "$output" >/dev/null 2>&1 || {
        echo "Error: device request $endpoint did not return JSON" >&2
        exit 1
    }
}

if [ -n "$DEVICE" ]; then
    HEALTH_FILE="$TMP_DIR/health.json"
    OTA_FILE="$TMP_DIR/ota.json"
    STATE_FILE="$TMP_DIR/state.json"
    AUTH_FILE="$TMP_DIR/auth.json"
    REMOTE_FILE="$TMP_DIR/remote.json"

    request_json "/health" "$HEALTH_FILE"
    [ "$(jq -r '.status // empty' "$HEALTH_FILE")" = "healthy" ] || {
        echo "Error: device health response is not healthy" >&2
        exit 1
    }

    request_json "/api/ota/status" "$OTA_FILE"
    INSTALLED_VERSION="$(jq -r '.current_package_version // .current_version // empty' "$OTA_FILE")"
    if [ "$INSTALLED_VERSION" != "$EXPECTED_VERSION" ]; then
        echo "Error: device package version is $INSTALLED_VERSION, expected $EXPECTED_VERSION" >&2
        exit 1
    fi
    if [ "$(jq -r '.state // empty' "$OTA_FILE")" = "failed" ]; then
        echo "Error: device OTA state is failed: $(jq -r '.last_error // .message // "unknown"' "$OTA_FILE")" >&2
        exit 1
    fi

    STATE_JSON='null'
    AUTH_JSON='null'
    REMOTE_JSON='null'
    case "$SCENARIO" in
        state)
            request_json "/api/state?authoritative=true" "$STATE_FILE"
            STATE_JSON="$(jq -c . "$STATE_FILE")"
            ;;
        onboarding)
            request_json "/api/state?authoritative=true" "$STATE_FILE"
            request_json "/api/auth/status" "$AUTH_FILE"
            request_json "/api/remote-access/status" "$REMOTE_FILE"
            STATE_JSON="$(jq -c . "$STATE_FILE")"
            AUTH_JSON="$(jq -c . "$AUTH_FILE")"
            REMOTE_JSON="$(jq -c . "$REMOTE_FILE")"
            ;;
        smoke|ota)
            ;;
    esac

    DEVICE_JSON="$(jq -n \
        --arg url "$DEVICE" \
        --arg scenario "$SCENARIO" \
        --slurpfile health "$HEALTH_FILE" \
        --slurpfile ota "$OTA_FILE" \
        --argjson state "$STATE_JSON" \
        --argjson auth "$AUTH_JSON" \
        --argjson remote "$REMOTE_JSON" \
        '{url:$url, scenario:$scenario, health:$health[0], ota:$ota[0], state:$state, auth:$auth, remote_access:$remote}')"
else
    WARNINGS_JSON='["live device not checked"]'
fi

mkdir -p "$(dirname "$RECEIPT")"
RECEIPT_TMP="$RECEIPT.tmp.$$"
jq -n \
    --arg schema_version "1" \
    --arg generated_at "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    --arg journey_id "$JOURNEY_ID" \
    --arg beta_tag "$BETA_TAG" \
    --arg commit "$LOCAL_COMMIT" \
    --arg manifest_url "$MANIFEST_URL" \
    --slurpfile manifest "$MANIFEST_FILE" \
    --argjson device "$DEVICE_JSON" \
    --argjson warnings "$WARNINGS_JSON" \
    '{schema_version:($schema_version|tonumber), generated_at:$generated_at, journey_id:$journey_id, beta_tag:$beta_tag, commit:$commit, manifest_url:$manifest_url, manifest:$manifest[0], device:$device, warnings:$warnings, result:"verified"}' \
    > "$RECEIPT_TMP"
mv "$RECEIPT_TMP" "$RECEIPT"

echo "Verified $BETA_TAG against the public beta feed."
if [ -n "$DEVICE" ]; then
    echo "Verified device package $EXPECTED_VERSION for scenario $SCENARIO."
else
    echo "Device verification was not requested (advisory)."
fi
echo "Receipt: $RECEIPT"
