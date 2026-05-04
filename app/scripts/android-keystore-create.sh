#!/bin/bash
# Create an Android upload keystore and Gradle key.properties file.
#
# Usage: ./scripts/android-keystore-create.sh [--force]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ANDROID_DIR="$PROJECT_ROOT/flutter/rhythm_app/android"

KEY_ALIAS="${ANDROID_KEY_ALIAS:-upload}"
KEYSTORE_RELATIVE_PATH="${ANDROID_KEYSTORE_RELATIVE_PATH:-app/upload-keystore.jks}"
KEYSTORE_PATH="$ANDROID_DIR/$KEYSTORE_RELATIVE_PATH"
KEY_PROPERTIES="$ANDROID_DIR/key.properties"
VALIDITY_DAYS="${ANDROID_KEYSTORE_VALIDITY_DAYS:-10000}"
DNAME="${ANDROID_KEYSTORE_DNAME:-CN=Rhythm Lighting, OU=Mobile, O=Rhythm, L=New York, ST=New York, C=US}"
FORCE=false

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    cat <<USAGE
Options:
  --alias VALUE      Key alias (default: $KEY_ALIAS)
  --force            Overwrite an existing keystore/key.properties
  -h, --help         Show this help

Environment overrides:
  ANDROID_KEYSTORE_PASSWORD
  ANDROID_KEY_PASSWORD
  ANDROID_KEY_ALIAS
  ANDROID_KEYSTORE_RELATIVE_PATH
  ANDROID_KEYSTORE_VALIDITY_DAYS
  ANDROID_KEYSTORE_DNAME
USAGE
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --alias)
            KEY_ALIAS="${2:-}"
            if [ -z "$KEY_ALIAS" ] || [[ "$KEY_ALIAS" == --* ]]; then
                echo "Error: --alias requires a value." >&2
                exit 1
            fi
            shift 2
            ;;
        --force)
            FORCE=true
            shift
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            echo "Unknown option: $1" >&2
            usage
            exit 1
            ;;
    esac
done

if [ ! -d "$ANDROID_DIR" ]; then
    echo "Android scaffold not found. Run ./scripts/android-setup.sh first." >&2
    exit 1
fi

if ! command -v keytool > /dev/null 2>&1; then
    echo "Error: keytool not found. Install a JDK first." >&2
    exit 1
fi

if [ "$FORCE" = false ]; then
    if [ -f "$KEYSTORE_PATH" ]; then
        echo "Error: keystore already exists: $KEYSTORE_PATH" >&2
        echo "Use --force only if you intentionally want to replace it." >&2
        exit 1
    fi
    if [ -f "$KEY_PROPERTIES" ]; then
        echo "Error: key.properties already exists: $KEY_PROPERTIES" >&2
        echo "Use --force only if you intentionally want to replace it." >&2
        exit 1
    fi
fi

STORE_PASSWORD="${ANDROID_KEYSTORE_PASSWORD:-}"
KEY_PASSWORD="${ANDROID_KEY_PASSWORD:-}"

if [ -z "$STORE_PASSWORD" ]; then
    read -rsp "Keystore password: " STORE_PASSWORD
    echo ""
fi

if [ -z "$KEY_PASSWORD" ]; then
    read -rsp "Key password (press Enter to reuse keystore password): " KEY_PASSWORD
    echo ""
fi

if [ -z "$KEY_PASSWORD" ]; then
    KEY_PASSWORD="$STORE_PASSWORD"
fi

mkdir -p "$(dirname "$KEYSTORE_PATH")"

keytool -genkeypair \
    -v \
    -keystore "$KEYSTORE_PATH" \
    -alias "$KEY_ALIAS" \
    -keyalg RSA \
    -keysize 2048 \
    -validity "$VALIDITY_DAYS" \
    -storepass "$STORE_PASSWORD" \
    -keypass "$KEY_PASSWORD" \
    -dname "$DNAME"

umask 077
cat > "$KEY_PROPERTIES" <<PROPERTIES
storeFile=$KEYSTORE_RELATIVE_PATH
storePassword=$STORE_PASSWORD
keyAlias=$KEY_ALIAS
keyPassword=$KEY_PASSWORD
PROPERTIES

echo "Created keystore: $KEYSTORE_PATH"
echo "Created Gradle signing config: $KEY_PROPERTIES"
echo "Both files are ignored by android/.gitignore; keep a secure backup."
