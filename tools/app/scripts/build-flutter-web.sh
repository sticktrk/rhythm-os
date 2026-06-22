#!/bin/bash
# Build Flutter web app
#
# Usage: ./tools/app/scripts/build-flutter-web.sh [--skip-wasm] [--context ha_addon|standalone_web]
# Output: dist/flutter_web/

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../app" && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"

# Defaults
SKIP_WASM=false
PLATFORM_CONTEXT="standalone_web"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-wasm)
            SKIP_WASM=true
            shift
            ;;
        --context)
            PLATFORM_CONTEXT="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --skip-wasm              Skip WASM build (use existing)"
            echo "  --context <context>      Platform context: ha_addon, standalone_web (default)"
            echo "  -h, --help               Show this help"
            echo ""
            echo "Output: dist/flutter_web/"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Build WASM first (unless skipped)
if [ "$SKIP_WASM" = false ]; then
    echo "Building WASM..."
    "$SCRIPT_DIR/build-wasm.sh"
    echo ""
fi

# Find flutter command
if command -v flutter &> /dev/null; then
    FLUTTER_CMD="flutter"
elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
    FLUTTER_CMD="$HOME/Documents/flutter/bin/flutter"
elif [ -x "/opt/flutter/bin/flutter" ]; then
    FLUTTER_CMD="/opt/flutter/bin/flutter"
else
    echo "Error: flutter not found. Install Flutter or set PATH."
    exit 1
fi

# Check for .env file with Supabase credentials
DART_DEFINES=""
if [ -f "$FLUTTER_APP/.env" ]; then
    DART_DEFINES="--dart-define-from-file=$FLUTTER_APP/.env"
    echo "Using Supabase config from .env"
fi

echo "Building Flutter web (PLATFORM_CONTEXT=$PLATFORM_CONTEXT)..."
(cd "$FLUTTER_APP" && "$FLUTTER_CMD" build web --release --dart-define=PLATFORM_CONTEXT=$PLATFORM_CONTEXT $DART_DEFINES)

# Patch base href for HA ingress (must be relative, not absolute root)
INDEX="$FLUTTER_APP/build/web/index.html"
sed -i '' 's|<base href="/">|<base href="./">|' "$INDEX" 2>/dev/null || \
sed -i 's|<base href="/">|<base href="./">|' "$INDEX"

# Copy to dist
FLUTTER_WEB_DEST="$PROJECT_ROOT/dist/flutter_web"
echo ""
echo "Copying Flutter web to dist/flutter_web/..."
rm -rf "$FLUTTER_WEB_DEST"
cp -r "$FLUTTER_APP/build/web" "$FLUTTER_WEB_DEST"

echo ""
echo "Flutter web build complete"
echo "Output: dist/flutter_web/"
