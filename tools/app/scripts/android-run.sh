#!/bin/bash
# Run the Rhythm Lighting Flutter app on an Android device or emulator.
#
# Usage: ./tools/app/scripts/android-run.sh [--device DEVICE_ID] [--debug|--profile|--release] [-- --extra-flutter-args]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"

MODE="debug"
DEVICE="${RHYTHM_ANDROID_DEVICE:-}"
RUN_SETUP=true
RUN_PUB=false
EXTRA_ARGS=()

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    cat <<USAGE
Options:
  --device DEVICE_ID  Device or emulator id from flutter devices
  --debug             Run in debug mode (default)
  --profile           Run in profile mode
  --release           Run in release mode
  --no-setup          Skip android-setup.sh
  --pub               Run flutter pub get before flutter run
  --                  Pass remaining args directly to flutter run
  -h, --help          Show this help

Environment overrides:
  RHYTHM_ANDROID_DEVICE  Flutter device selector/id
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

first_android_device() {
    "$FLUTTER_CMD" devices --machine \
        | sed -n '
            /"id":/ {
                s/.*"id": "\([^"]*\)".*/\1/
                h
            }
            /"targetPlatform": "android-/ {
                g
                p
                q
            }
        '
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --device|-d)
            DEVICE="${2:-}"
            if [ -z "$DEVICE" ] || [[ "$DEVICE" == --* ]]; then
                echo "Error: --device requires a value." >&2
                exit 1
            fi
            shift 2
            ;;
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
        --no-setup)
            RUN_SETUP=false
            shift
            ;;
        --pub)
            RUN_PUB=true
            shift
            ;;
        --)
            shift
            EXTRA_ARGS+=("$@")
            break
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

if [ "$RUN_PUB" = true ]; then
    "$FLUTTER_CMD" pub get
fi

if [ -z "$DEVICE" ]; then
    DEVICE="$(first_android_device)"
fi

if [ -z "$DEVICE" ]; then
    echo "Error: no Android device or emulator found." >&2
    echo "Start an Android emulator in Android Studio Device Manager or connect an Android device with USB debugging enabled." >&2
    echo "" >&2
    "$FLUTTER_CMD" devices
    exit 1
fi

RUN_ARGS=()
RUN_ARGS+=(--"$MODE")

RUN_ARGS+=(-d "$DEVICE")

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
    RUN_ARGS+=(--dart-define-from-file="$APP_BUILD_ENV_FILE")
fi

if [ ${#EXTRA_ARGS[@]} -gt 0 ]; then
    RUN_ARGS+=("${EXTRA_ARGS[@]}")
fi

"$FLUTTER_CMD" run "${RUN_ARGS[@]}"
