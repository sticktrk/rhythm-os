#!/bin/bash
# Build the Rhythm Lighting Flutter app for Android.
#
# Usage: ./tools/app/scripts/android-build.sh [--debug|--profile|--release] [--apk|--aab] [options]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"

MODE="debug"
ARTIFACT="apk"
CLEAN=false
RUN_SETUP=true
BUILD_NAME=""
BUILD_NUMBER=""
TARGET_PLATFORM=""

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    cat <<USAGE
Options:
  --debug                  Build a debug APK (default)
  --profile                Build a profile APK
  --release                Build a release APK/AAB
  --apk                    Build an APK (default)
  --aab, --appbundle       Build an Android App Bundle; implies --release
  --split-per-abi          Split APK output per Android ABI
  --target-platform VALUE  Forward target platform, e.g. android-arm64
  --build-name VALUE       Override Flutter build name
  --build-number VALUE     Override Flutter build number
  --clean                  Run flutter clean before building
  --no-setup               Skip android-setup.sh
  -h, --help               Show this help
USAGE
}

find_flutter() {
    if command -v flutter > /dev/null 2>&1; then
        command -v flutter
    elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
        echo "$HOME/Documents/flutter/bin/flutter"
    elif [ -x "/opt/flutter/bin/flutter" ]; then
        echo "/opt/flutter/bin/flutter"
    else
        echo "Error: flutter not found. Install Flutter or add it to PATH." >&2
        exit 1
    fi
}

EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --debug)
            MODE="debug"
            shift
            ;;
        --profile)
            MODE="profile"
            shift
            ;;
        --release)
            MODE="release"
            shift
            ;;
        --apk)
            ARTIFACT="apk"
            shift
            ;;
        --aab|--appbundle)
            ARTIFACT="appbundle"
            MODE="release"
            shift
            ;;
        --split-per-abi)
            EXTRA_ARGS+=(--split-per-abi)
            shift
            ;;
        --target-platform)
            TARGET_PLATFORM="${2:-}"
            if [ -z "$TARGET_PLATFORM" ] || [[ "$TARGET_PLATFORM" == --* ]]; then
                echo "Error: --target-platform requires a value." >&2
                exit 1
            fi
            shift 2
            ;;
        --build-name)
            BUILD_NAME="${2:-}"
            if [ -z "$BUILD_NAME" ] || [[ "$BUILD_NAME" == --* ]]; then
                echo "Error: --build-name requires a value." >&2
                exit 1
            fi
            shift 2
            ;;
        --build-number)
            BUILD_NUMBER="${2:-}"
            if [ -z "$BUILD_NUMBER" ] || [[ "$BUILD_NUMBER" == --* ]]; then
                echo "Error: --build-number requires a value." >&2
                exit 1
            fi
            shift 2
            ;;
        --clean)
            CLEAN=true
            shift
            ;;
        --no-setup)
            RUN_SETUP=false
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

FLUTTER_CMD="$(find_flutter)"

if [ "$RUN_SETUP" = true ]; then
    "$SCRIPT_DIR/android-setup.sh"
fi

cd "$FLUTTER_APP"

if [ "$CLEAN" = true ]; then
    "$FLUTTER_CMD" clean
fi

"$FLUTTER_CMD" pub get

BUILD_ARGS=()
BUILD_ARGS+=(--"$MODE")

APP_BUILD_ENV_FILE="${RHYTHM_APP_BUILD_ENV_FILE:-${RHYTHM_CONFIG_DIR:-$HOME/.config/rhythm}/app-build.env}"
APP_BUILD_ENV_IS_EXTERNAL=true
if [ ! -f "$APP_BUILD_ENV_FILE" ] && [ -z "${RHYTHM_APP_BUILD_ENV_FILE:-}" ] && \
    [ -z "${RHYTHM_CONFIG_DIR:-}" ] && [ -f "$FLUTTER_APP/.env" ]; then
    APP_BUILD_ENV_FILE="$FLUTTER_APP/.env"
    APP_BUILD_ENV_IS_EXTERNAL=false
fi
if [ -f "$APP_BUILD_ENV_FILE" ]; then
    if [ "$APP_BUILD_ENV_IS_EXTERNAL" = true ] && \
        ! RHYTHM_APP_BUILD_ENV_FILE="$APP_BUILD_ENV_FILE" \
            python3 "$PROJECT_ROOT/../tools/config/validate.py" --profile app-build; then
        echo "Error: the external app-build profile is not ready." >&2
        exit 1
    fi
    BUILD_ARGS+=(--dart-define-from-file="$APP_BUILD_ENV_FILE")
fi

if [ -n "$BUILD_NAME" ]; then
    BUILD_ARGS+=(--build-name="$BUILD_NAME")
fi

if [ -n "$BUILD_NUMBER" ]; then
    BUILD_ARGS+=(--build-number="$BUILD_NUMBER")
fi

if [ -n "$TARGET_PLATFORM" ]; then
    BUILD_ARGS+=(--target-platform="$TARGET_PLATFORM")
fi

if [ ${#EXTRA_ARGS[@]} -gt 0 ]; then
    BUILD_ARGS+=("${EXTRA_ARGS[@]}")
fi

if [ "$ARTIFACT" = "appbundle" ]; then
    echo "Building Android App Bundle..."
    "$FLUTTER_CMD" build appbundle "${BUILD_ARGS[@]}"
    echo "Output: $FLUTTER_APP/build/app/outputs/bundle/release/"
else
    echo "Building Android APK..."
    "$FLUTTER_CMD" build apk "${BUILD_ARGS[@]}"
    echo "Output: $FLUTTER_APP/build/app/outputs/flutter-apk/"
fi
