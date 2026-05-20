#!/bin/bash
#
# Promote a released rpiz build from the beta OTA feed to the stable feed.
#
# Beta is published automatically by CI for every v* tag at
#   $RHYTHM_UPDATES_BASE_DIR/rpiz/manifest.json
# Stable is populated only by this script — auto-updating appliances poll
#   $RHYTHM_UPDATES_BASE_DIR/rpiz-stable/manifest.json
# during their overnight window. The same archives are reused (no duplication);
# the stable manifest's package URL is rewritten to ../rpiz/v<version>/... so
# it resolves against the rpiz/ versioned directories on the same host.
#
# Usage:
#   ./scripts/promote-stable.sh                # promote whatever is currently in beta
#   ./scripts/promote-stable.sh --version 0.4.219-beta
#   ./scripts/promote-stable.sh --dry-run      # show planned manifest and target without uploading
#
# Requires the same .env credentials release.sh --upload uses:
#   RHYTHM_UPDATES_SSH_HOST, RHYTHM_UPDATES_SSH_USER, RHYTHM_UPDATES_BASE_DIR
#   RHYTHM_UPDATES_SSH_KEY_FILE or RHYTHM_UPDATES_SSH_KEY
# Optional public base URL for archive HEAD probes:
#   RHYTHM_UPDATES_PUBLIC_BASE_URL (default: https://dl.rhythm.lighting/server)

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

DRY_RUN=false
EXPECTED_VERSION=""
PLATFORM="rpiz"
PLATFORM_STABLE="rpiz-stable"
PUBLIC_BASE_URL_DEFAULT="https://dl.rhythm.lighting/server"

usage() {
    cat <<EOF
Usage: $0 [OPTIONS]

Promote the current rpiz beta OTA feed to the rpiz-stable feed.

Options:
  --version VERSION   Refuse to publish unless the beta manifest reports this
                      exact version (defensive check for "promote this build,
                      not whatever is in beta right now")
  --dry-run           Print the rewritten manifest and target path without
                      uploading anything
  -h, --help          Show this help
EOF
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --version)
            if [ $# -lt 2 ]; then
                echo "Error: --version requires a value" >&2
                exit 1
            fi
            EXPECTED_VERSION="${2#v}"
            if [[ "$EXPECTED_VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
                EXPECTED_VERSION="${EXPECTED_VERSION}-beta"
            fi
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
    if ! command -v "$1" >/dev/null 2>&1; then
        echo "Error: Required command not found: $1" >&2
        exit 1
    fi
}

require_command ssh
require_command scp
require_command ssh-keyscan
require_command jq
require_command curl

load_project_env() {
    local env_file="$PROJECT_ROOT/.env"
    if [ ! -f "$env_file" ]; then
        echo "Error: $env_file not found (need RHYTHM_UPDATES_* credentials)" >&2
        exit 1
    fi
    echo "Loading environment from .env"
    set -a
    # shellcheck disable=SC1090
    source "$env_file"
    set +a
}

require_env_value() {
    local name="$1"
    if [ -z "${!name:-}" ]; then
        echo "Error: Required environment variable not set: $name" >&2
        exit 1
    fi
}

resolve_project_path() {
    local path_value="$1"
    case "$path_value" in
        /*) echo "$path_value" ;;
        ~/*) echo "$HOME/${path_value#~/}" ;;
        *) echo "$PROJECT_ROOT/$path_value" ;;
    esac
}

load_project_env

require_env_value RHYTHM_UPDATES_SSH_HOST
require_env_value RHYTHM_UPDATES_SSH_USER
require_env_value RHYTHM_UPDATES_BASE_DIR

if [ -z "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ] && [ -z "${RHYTHM_UPDATES_SSH_KEY:-}" ]; then
    echo "Error: Set RHYTHM_UPDATES_SSH_KEY_FILE or RHYTHM_UPDATES_SSH_KEY in .env" >&2
    exit 1
fi

if [ -n "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ]; then
    RHYTHM_UPDATES_SSH_KEY_FILE="$(resolve_project_path "$RHYTHM_UPDATES_SSH_KEY_FILE")"
    if [ ! -f "$RHYTHM_UPDATES_SSH_KEY_FILE" ]; then
        echo "Error: RHYTHM_UPDATES_SSH_KEY_FILE does not exist: $RHYTHM_UPDATES_SSH_KEY_FILE" >&2
        exit 1
    fi
fi

PUBLIC_BASE_URL="${RHYTHM_UPDATES_PUBLIC_BASE_URL:-$PUBLIC_BASE_URL_DEFAULT}"
PUBLIC_BASE_URL="${PUBLIC_BASE_URL%/}"

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-promote-stable.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

SSH_KEY="$WORK_DIR/id_ed25519"
KNOWN_HOSTS="$WORK_DIR/known_hosts"

if [ -n "${RHYTHM_UPDATES_SSH_KEY_FILE:-}" ]; then
    cp "$RHYTHM_UPDATES_SSH_KEY_FILE" "$SSH_KEY"
else
    printf '%s\n' "$RHYTHM_UPDATES_SSH_KEY" > "$SSH_KEY"
fi
chmod 600 "$SSH_KEY"
ssh-keyscan -H "$RHYTHM_UPDATES_SSH_HOST" > "$KNOWN_HOSTS" 2>/dev/null

ssh_cmd() {
    ssh -i "$SSH_KEY" -o UserKnownHostsFile="$KNOWN_HOSTS" \
        "$RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST" "$@"
}

scp_get() {
    scp -i "$SSH_KEY" -o UserKnownHostsFile="$KNOWN_HOSTS" \
        "$RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$1" "$2"
}

scp_put() {
    scp -i "$SSH_KEY" -o UserKnownHostsFile="$KNOWN_HOSTS" \
        "$1" "$RHYTHM_UPDATES_SSH_USER@$RHYTHM_UPDATES_SSH_HOST:$2"
}

BETA_PATH="$RHYTHM_UPDATES_BASE_DIR/$PLATFORM/manifest.json"
STABLE_PATH="$RHYTHM_UPDATES_BASE_DIR/$PLATFORM_STABLE/manifest.json"
BETA_LOCAL="$WORK_DIR/beta-manifest.json"
STABLE_LOCAL="$WORK_DIR/stable-manifest.json"

echo "Fetching $BETA_PATH"
scp_get "$BETA_PATH" "$BETA_LOCAL"

BETA_VERSION="$(jq -r '.version' "$BETA_LOCAL")"
if [ -z "$BETA_VERSION" ] || [ "$BETA_VERSION" = "null" ]; then
    echo "Error: Beta manifest missing .version field" >&2
    exit 1
fi
echo "Beta manifest version: $BETA_VERSION"

if [ -n "$EXPECTED_VERSION" ] && [ "$EXPECTED_VERSION" != "$BETA_VERSION" ]; then
    echo "Error: --version $EXPECTED_VERSION does not match beta manifest version $BETA_VERSION" >&2
    exit 1
fi

# Rewrite relative URLs from v<version>/... to ../<PLATFORM>/v<version>/... so
# they resolve against the original rpiz/ directories (we don't duplicate the
# archives, just the manifest).
jq --arg platform "$PLATFORM" '
    def rewrite_url:
        if (type == "string") and (test("^[a-zA-Z][a-zA-Z0-9+.-]*://") | not)
        then "../" + $platform + "/" + .
        else .
        end;
    .package.url |= rewrite_url
    | .images = ((.images // []) | map(.url |= rewrite_url))
' "$BETA_LOCAL" > "$STABLE_LOCAL"

echo ""
echo "Rewritten stable manifest:"
jq '.' "$STABLE_LOCAL"

resolve_public_url() {
    local url="$1"
    case "$url" in
        *://*) echo "$url" ;;
        *)
            # Stable manifest sits at <PLATFORM_STABLE>/, so resolve relative
            # URLs against that directory. ../<PLATFORM>/... walks up to base.
            echo "$PUBLIC_BASE_URL/$PLATFORM_STABLE/$url"
            ;;
    esac
}

# Verify every URL the appliance may apply before publishing. A package-only
# probe is not enough when a promoted rpiz manifest includes rootfs images,
# because the appliance install path prefers the rootfs payload.
ARTIFACT_URLS=()
while IFS= read -r ARTIFACT_URL; do
    ARTIFACT_URLS+=("$ARTIFACT_URL")
done < <(jq -r '
    [.package.url] + ((.images // []) | map(.url))
    | .[]
    | select(type == "string" and length > 0)
' "$STABLE_LOCAL")

if [ "${#ARTIFACT_URLS[@]}" -eq 0 ]; then
    echo "Error: stable manifest has no package or image URLs to probe" >&2
    exit 1
fi

echo ""
for ARTIFACT_URL in "${ARTIFACT_URLS[@]}"; do
    ABS_URL="$(resolve_public_url "$ARTIFACT_URL")"
    echo "Probing $ABS_URL"
    HTTP_STATUS="$(curl -sIL -o /dev/null -w '%{http_code}' "$ABS_URL")"
    if [ "$HTTP_STATUS" != "200" ]; then
        echo "Error: artifact URL returned HTTP $HTTP_STATUS (expected 200): $ABS_URL" >&2
        echo "Refusing to publish — auto-updating appliances would 404." >&2
        exit 1
    fi
done
echo "All promoted artifact URLs respond 200."

if [ "$DRY_RUN" = true ]; then
    echo ""
    echo "[dry-run] Would upload $STABLE_LOCAL -> $STABLE_PATH"
    exit 0
fi

echo ""
echo "Publishing to $STABLE_PATH"
ssh_cmd "mkdir -p '$RHYTHM_UPDATES_BASE_DIR/$PLATFORM_STABLE'"
scp_put "$STABLE_LOCAL" "$STABLE_PATH"

echo ""
echo "Promoted $BETA_VERSION to the rpiz stable channel."
echo "Auto-updating appliances will pick it up during their next overnight window."
