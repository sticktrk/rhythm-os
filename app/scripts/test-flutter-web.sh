#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(dirname "$SCRIPT_DIR")"
FLUTTER_WEB_DIR="$PROJECT_ROOT/flutter/rhythm_app"
WASM_PKG_DIR="$FLUTTER_WEB_DIR/web/pkg"

# Parse arguments
PORT=8080
DEVICE="chrome"
SKIP_WASM=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --port)
            PORT="$2"
            shift 2
            ;;
        --edge)
            DEVICE="edge"
            shift
            ;;
        --web-server)
            DEVICE="web-server"
            shift
            ;;
        --skip-wasm)
            SKIP_WASM=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [options]"
            echo ""
            echo "Options:"
            echo "  --port <port>    Web server port (default: 8080)"
            echo "  --edge           Use Edge instead of Chrome"
            echo "  --web-server     Run as web server only (no browser)"
            echo "  --skip-wasm      Skip WASM build check"
            echo "  -h, --help       Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Check/build WASM
if [ "$SKIP_WASM" = false ]; then
    if [ ! -d "$WASM_PKG_DIR" ] || [ -z "$(ls -A "$WASM_PKG_DIR" 2>/dev/null)" ]; then
        echo "WASM not built. Building now..."
        echo ""
        "$SCRIPT_DIR/build-wasm.sh"
        echo ""
    else
        echo "WASM already built at $WASM_PKG_DIR"
    fi
fi

cd "$FLUTTER_WEB_DIR"

echo ""
echo "Running Flutter web from $FLUTTER_WEB_DIR"
echo "Device: $DEVICE, Port: $PORT"
echo ""

# Add CORS headers required for SharedArrayBuffer (needed for WASM)
flutter run -d "$DEVICE" --web-port "$PORT" \
    --web-header "Cross-Origin-Opener-Policy=same-origin" \
    --web-header "Cross-Origin-Embedder-Policy=require-corp"
