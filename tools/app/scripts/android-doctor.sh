#!/bin/bash
# Check the local Android toolchain needed to build Rhythm Lighting.
#
# Usage: ./tools/app/scripts/android-doctor.sh [--accept-licenses] [--install-rust-targets]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"

ACCEPT_LICENSES=false
INSTALL_RUST_TARGETS=false
STATUS=0
ANDROID_SDK=""

ANDROID_RUST_TARGETS=(
    aarch64-linux-android
    armv7-linux-androideabi
    i686-linux-android
    x86_64-linux-android
)

usage() {
    sed -n '2,/^$/p' "$0" | sed 's/^# \{0,1\}//'
    cat <<USAGE
Options:
  --accept-licenses       Run flutter doctor --android-licenses
  --install-rust-targets  Install Rust targets used by Android builds
  -h, --help              Show this help
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
        echo ""
    fi
}

check_command() {
    local label="$1"
    local command_name="$2"

    if command -v "$command_name" > /dev/null 2>&1; then
        echo "[ok] $label: $(command -v "$command_name")"
    else
        echo "[missing] $label: $command_name is not on PATH"
        STATUS=1
    fi
}

flutter_configured_android_sdk() {
    "$FLUTTER_CMD" config --list 2>/dev/null \
        | sed -nE "s/.*'android-sdk': '([^']+)'.*/\1/p; s/^[[:space:]]*android-sdk:[[:space:]]*(.+)$/\1/p" \
        | head -1
}

resolve_android_sdk() {
    if [ -n "${ANDROID_HOME:-}" ]; then
        echo "$ANDROID_HOME"
    elif [ -n "${ANDROID_SDK_ROOT:-}" ]; then
        echo "$ANDROID_SDK_ROOT"
    else
        flutter_configured_android_sdk
    fi
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --accept-licenses)
            ACCEPT_LICENSES=true
            shift
            ;;
        --install-rust-targets)
            INSTALL_RUST_TARGETS=true
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
if [ -z "$FLUTTER_CMD" ]; then
    echo "[missing] Flutter: install Flutter or add it to PATH"
    exit 1
fi

echo "Flutter app: $FLUTTER_APP"
echo "Flutter: $FLUTTER_CMD"
echo ""

"$FLUTTER_CMD" --version
echo ""

check_command "Java" java
check_command "keytool" keytool
check_command "Rust" rustup
check_command "Cargo" cargo
echo ""

ANDROID_SDK="$(resolve_android_sdk)"

if [ -n "${ANDROID_HOME:-}" ]; then
    echo "[ok] ANDROID_HOME=$ANDROID_HOME"
elif [ -n "${ANDROID_SDK_ROOT:-}" ]; then
    echo "[ok] ANDROID_SDK_ROOT=$ANDROID_SDK_ROOT"
elif [ -n "$ANDROID_SDK" ]; then
    echo "[ok] Flutter Android SDK=$ANDROID_SDK"
else
    echo "[missing] Android SDK env: ANDROID_HOME or ANDROID_SDK_ROOT is not set"
    STATUS=1
fi

if [ -n "$ANDROID_SDK" ] && [ -x "$ANDROID_SDK/platform-tools/adb" ]; then
    echo "[ok] adb: $ANDROID_SDK/platform-tools/adb"
elif command -v adb > /dev/null 2>&1; then
    echo "[ok] adb: $(command -v adb)"
else
    echo "[missing] adb: install Android SDK platform-tools"
    STATUS=1
fi

if [ -n "$ANDROID_SDK" ] && [ -x "$ANDROID_SDK/cmdline-tools/latest/bin/sdkmanager" ]; then
    echo "[ok] sdkmanager: $ANDROID_SDK/cmdline-tools/latest/bin/sdkmanager"
elif command -v sdkmanager > /dev/null 2>&1; then
    echo "[ok] sdkmanager: $(command -v sdkmanager)"
else
    echo "[missing] sdkmanager: install Android SDK command-line tools"
    STATUS=1
fi

if command -v rustup > /dev/null 2>&1; then
    echo ""
    echo "Rust Android targets:"
    for target in "${ANDROID_RUST_TARGETS[@]}"; do
        if rustup target list --installed | grep -qx "$target"; then
            echo "[ok] $target"
        else
            echo "[missing] $target"
            STATUS=1
        fi
    done
fi

if [ "$INSTALL_RUST_TARGETS" = true ]; then
    echo ""
    echo "Installing Rust Android targets..."
    rustup target add "${ANDROID_RUST_TARGETS[@]}"
fi

if [ "$ACCEPT_LICENSES" = true ]; then
    echo ""
    "$FLUTTER_CMD" doctor --android-licenses
fi

echo ""
echo "Flutter doctor:"
set +e
"$FLUTTER_CMD" doctor -v
DOCTOR_STATUS=$?
set -e

if [ $DOCTOR_STATUS -ne 0 ]; then
    STATUS=1
fi

echo ""
if [ $STATUS -ne 0 ]; then
    echo "Android setup is incomplete."
    echo "In Android Studio, install Android SDK Command-line Tools from Settings > Languages & Frameworks > Android SDK > SDK Tools."
    echo "Then run:"
    if [ -n "$ANDROID_SDK" ]; then
        echo "  export ANDROID_HOME=\"$ANDROID_SDK\""
        echo "  export PATH=\"\$ANDROID_HOME/platform-tools:\$ANDROID_HOME/cmdline-tools/latest/bin:\$PATH\""
    else
        echo "  flutter config --android-sdk <path-to-android-sdk>"
    fi
    echo "  ./tools/app/scripts/android-doctor.sh --accept-licenses --install-rust-targets"
    exit 1
fi

echo "Android setup looks ready."
