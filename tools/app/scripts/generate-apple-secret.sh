#!/bin/bash
# Generate Apple client secret JWT for Sign in with Apple OAuth
#
# Required .env variables:
#   APPLE_TEAM_ID     - Your Apple Developer Team ID
#   APPLE_SERVICE_ID  - Your Service ID (client_id)
#   APPLE_KEY_ID      - Your private key ID
#   APPLE_KEY_FILE    - Path to your .p8 private key file
#
# Optional:
#   APPLE_SECRET_EXPIRY - Expiry in days (default: 180, max: 180)

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"

# Load .env file
load_env() {
    local env_file="$1"
    while IFS= read -r line || [ -n "$line" ]; do
        # Skip comments and empty lines
        [[ "$line" =~ ^[[:space:]]*# ]] && continue
        [[ -z "${line// }" ]] && continue
        # Remove surrounding quotes and export
        if [[ "$line" =~ ^([A-Za-z_][A-Za-z0-9_]*)=(.*)$ ]]; then
            local key="${BASH_REMATCH[1]}"
            local value="${BASH_REMATCH[2]}"
            # Strip surrounding quotes
            value="${value#\"}"
            value="${value%\"}"
            value="${value#\'}"
            value="${value%\'}"
            export "$key=$value"
        fi
    done < "$env_file"
}

if [ -f "$PROJECT_ROOT/flutter/rhythm_app/.env" ]; then
    load_env "$PROJECT_ROOT/flutter/rhythm_app/.env"
elif [ -f "$PROJECT_ROOT/.env" ]; then
    load_env "$PROJECT_ROOT/.env"
elif [ -f "$SCRIPT_DIR/.env" ]; then
    load_env "$SCRIPT_DIR/.env"
fi

# Validate required variables
missing=()
[ -z "$APPLE_TEAM_ID" ] && missing+=("APPLE_TEAM_ID")
[ -z "$APPLE_SERVICE_ID" ] && missing+=("APPLE_SERVICE_ID")
[ -z "$APPLE_KEY_ID" ] && missing+=("APPLE_KEY_ID")
[ -z "$APPLE_KEY_FILE" ] && missing+=("APPLE_KEY_FILE")

if [ ${#missing[@]} -gt 0 ]; then
    echo "Error: Missing required environment variables:"
    printf '  - %s\n' "${missing[@]}"
    echo ""
    echo "Add these to your .env file or export them."
    exit 1
fi

# Resolve key file path (check flutter/rhythm_app first, then project root)
if [[ ! "$APPLE_KEY_FILE" = /* ]]; then
    if [ -f "$PROJECT_ROOT/flutter/rhythm_app/$APPLE_KEY_FILE" ]; then
        APPLE_KEY_FILE="$PROJECT_ROOT/flutter/rhythm_app/$APPLE_KEY_FILE"
    else
        APPLE_KEY_FILE="$PROJECT_ROOT/$APPLE_KEY_FILE"
    fi
fi

if [ ! -f "$APPLE_KEY_FILE" ]; then
    echo "Error: Private key file not found: $APPLE_KEY_FILE"
    exit 1
fi

EXPIRY_DAYS="${APPLE_SECRET_EXPIRY:-180}"

# Generate JWT using Ruby (commonly available on macOS)
if command -v ruby &> /dev/null; then
    SECRET=$(ruby -rjson -ropenssl -rbase64 <<RUBY
team_id = "$APPLE_TEAM_ID"
service_id = "$APPLE_SERVICE_ID"
key_id = "$APPLE_KEY_ID"
key_file = "$APPLE_KEY_FILE"
expiry_days = $EXPIRY_DAYS

key = OpenSSL::PKey::EC.new(File.read(key_file))
now = Time.now.to_i

header = { alg: 'ES256', kid: key_id }
claims = {
  iss: team_id,
  iat: now,
  exp: now + (86400 * expiry_days),
  aud: 'https://appleid.apple.com',
  sub: service_id
}

# Manual JWT encoding (no gem dependency)
def base64url(data)
  Base64.urlsafe_encode64(data).tr('=', '')
end

header_b64 = base64url(header.to_json)
claims_b64 = base64url(claims.to_json)
signing_input = "#{header_b64}.#{claims_b64}"

signature = key.sign(OpenSSL::Digest::SHA256.new, signing_input)

# Convert DER signature to raw r||s format for ES256
asn1 = OpenSSL::ASN1.decode(signature)
r = asn1.value[0].value.to_s(2).rjust(32, "\x00")[-32..-1]
s = asn1.value[1].value.to_s(2).rjust(32, "\x00")[-32..-1]
raw_sig = r + s

puts "#{signing_input}.#{base64url(raw_sig)}"
RUBY
)

    if [ -n "$SECRET" ]; then
        echo "Apple Client Secret (valid for $EXPIRY_DAYS days):"
        echo ""
        echo "$SECRET"
        echo ""
        echo "Add to your .env as:"
        echo "APPLE_CLIENT_SECRET=$SECRET"
    else
        echo "Error: Failed to generate JWT"
        exit 1
    fi
else
    echo "Error: Ruby not found. Install Ruby or use the Node.js alternative below."
    echo ""
    echo "Node.js alternative (requires jsonwebtoken package):"
    echo "  npm install jsonwebtoken"
    echo "  node -e \"..."
    exit 1
fi
