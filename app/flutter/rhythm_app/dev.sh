#!/bin/bash
# Flutter development script with hot reload and FRB bindings regeneration

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

WORKSPACE_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"

# Parse arguments
REGEN_BINDINGS=false
BUILD_WASM=false
PORT=8080

while [[ $# -gt 0 ]]; do
    case $1 in
        --regen|--bindings)
            REGEN_BINDINGS=true
            shift
            ;;
        --wasm)
            BUILD_WASM=true
            REGEN_BINDINGS=true  # WASM build requires bindings
            shift
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --regen, --bindings  Regenerate flutter_rust_bridge bindings before starting"
            echo "  --wasm               Build WASM (implies --regen)"
            echo "  --port PORT          Web server port (default: 8080)"
            echo "  -h, --help           Show this help"
            echo ""
            echo "Prerequisites:"
            echo "  - Flutter SDK with web support"
            echo "  - For --regen: flutter_rust_bridge_codegen (cargo install flutter_rust_bridge_codegen)"
            echo "  - For --wasm: wasm-pack, rust nightly with wasm32-unknown-unknown target"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Find flutter command and resolve to full path
if command -v flutter &> /dev/null; then
    FLUTTER_CMD="$(command -v flutter)"
    # Resolve symlinks to get actual path
    if [ -L "$FLUTTER_CMD" ]; then
        FLUTTER_CMD="$(readlink -f "$FLUTTER_CMD" 2>/dev/null || realpath "$FLUTTER_CMD")"
    fi
elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
    FLUTTER_CMD="$HOME/Documents/flutter/bin/flutter"
elif [ -x "/opt/flutter/bin/flutter" ]; then
    FLUTTER_CMD="/opt/flutter/bin/flutter"
else
    echo "Error: flutter not found. Install Flutter or set PATH."
    exit 1
fi

FLUTTER_BIN_DIR="$(dirname "$FLUTTER_CMD")"

# Find dart command - check alongside flutter first, then in cache
if [ -x "$FLUTTER_BIN_DIR/dart" ]; then
    DART_CMD="$FLUTTER_BIN_DIR/dart"
elif [ -x "$FLUTTER_BIN_DIR/cache/dart-sdk/bin/dart" ]; then
    DART_CMD="$FLUTTER_BIN_DIR/cache/dart-sdk/bin/dart"
elif command -v dart &> /dev/null; then
    DART_CMD="$(command -v dart)"
else
    echo "Error: dart not found."
    exit 1
fi

# Ensure dependencies are up to date
echo "Getting Flutter dependencies..."
"$FLUTTER_CMD" pub get

# Regenerate FRB bindings if requested
if [ "$REGEN_BINDINGS" = true ]; then
    echo ""
    echo "Regenerating flutter_rust_bridge bindings..."

    if ! command -v flutter_rust_bridge_codegen &> /dev/null; then
        echo "Installing flutter_rust_bridge_codegen..."
        cargo install flutter_rust_bridge_codegen
    fi

    PATH="$FLUTTER_BIN_DIR:$PATH" flutter_rust_bridge_codegen generate
    echo "Bindings regenerated."
fi

# Build WASM if requested
if [ "$BUILD_WASM" = true ]; then
    echo ""
    echo "Building WASM..."

    if ! command -v wasm-pack &> /dev/null; then
        echo "Error: wasm-pack not found. Install with: cargo install wasm-pack"
        exit 1
    fi

    # Check prerequisites
    if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
        echo "Adding wasm32-unknown-unknown target..."
        rustup target add wasm32-unknown-unknown
    fi

    if ! rustup component list --toolchain nightly --installed | grep -q rust-src; then
        echo "Adding rust-src component for nightly..."
        rustup component add rust-src --toolchain nightly
    fi

    "$DART_CMD" run flutter_rust_bridge build-web --release \
        --rust-root ../../rust/core/rhythm-core-ffi \
        --output web/pkg

    # Copy WASM files
    RUST_FFI_DIR="$WORKSPACE_ROOT/rust/core/rhythm-core-ffi"
    WASM_SRC="$RUST_FFI_DIR/web/pkg/pkg"
    WASM_DEST="$SCRIPT_DIR/web/pkg"
    if [ -d "$WASM_SRC" ]; then
        echo "Copying WASM to web/pkg..."
        mkdir -p "$WASM_DEST"
        cp "$WASM_SRC"/* "$WASM_DEST/"
    fi

    echo "WASM build complete."
fi

# Start Flutter web with hot reload
echo ""
echo "Starting Flutter web development server on port $PORT..."
echo "Press 'r' for hot reload, 'R' for hot restart, 'q' to quit"
echo ""

exec "$FLUTTER_CMD" run -d chrome --web-port="$PORT"
