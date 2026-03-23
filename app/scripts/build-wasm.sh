#!/bin/bash
# Build WASM for Flutter web
#
# Usage: ./scripts/build-wasm.sh
# Output: dist/wasm/, flutter/rhythm_app/web/pkg/
# Requires: wasm-pack, flutter_rust_bridge_codegen

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
FLUTTER_APP="$PROJECT_ROOT/flutter/rhythm_app"
RUST_FFI_DIR="$PROJECT_ROOT/rhythm-lighting/rust/core/rhythm-core-ffi"

# Find flutter command
find_flutter() {
    if command -v flutter &> /dev/null; then
        FLUTTER_CMD="$(command -v flutter)"
        if [ -L "$FLUTTER_CMD" ]; then
            FLUTTER_CMD="$(readlink -f "$FLUTTER_CMD" 2>/dev/null || greadlink -f "$FLUTTER_CMD" 2>/dev/null || echo "$FLUTTER_CMD")"
        fi
    elif [ -x "$HOME/Documents/flutter/bin/flutter" ]; then
        FLUTTER_CMD="$HOME/Documents/flutter/bin/flutter"
    elif [ -x "/opt/flutter/bin/flutter" ]; then
        FLUTTER_CMD="/opt/flutter/bin/flutter"
    else
        echo "Error: flutter not found. Install Flutter or set PATH."
        exit 1
    fi
    echo "$FLUTTER_CMD"
}

FLUTTER_CMD=$(find_flutter)
FLUTTER_BIN_DIR="$(dirname "$FLUTTER_CMD")"
DART_CMD="$FLUTTER_BIN_DIR/dart"
if [ ! -x "$DART_CMD" ]; then
    DART_CMD="$FLUTTER_BIN_DIR/cache/dart-sdk/bin/dart"
fi

echo "Using Flutter: $FLUTTER_CMD"
echo "Using Dart: $DART_CMD"
echo ""

# Check prerequisites
if ! command -v wasm-pack &> /dev/null; then
    echo "Error: wasm-pack not found. Install with: cargo install wasm-pack"
    exit 1
fi

if ! rustup target list --installed | grep -q wasm32-unknown-unknown; then
    echo "Adding wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown
fi

if ! rustup component list --toolchain nightly --installed 2>/dev/null | grep -q rust-src; then
    echo "Adding rust-src component for nightly..."
    rustup component add rust-src --toolchain nightly
fi

if ! command -v flutter_rust_bridge_codegen &> /dev/null; then
    echo "Installing flutter_rust_bridge_codegen..."
    cargo install flutter_rust_bridge_codegen
fi

# Ensure Flutter dependencies
echo "Getting Flutter dependencies..."
(cd "$FLUTTER_APP" && "$FLUTTER_CMD" pub get)

# Regenerate FRB Rust bridge code before WASM compilation
echo ""
echo "Regenerating flutter_rust_bridge bindings..."
(cd "$FLUTTER_APP" && PATH="$FLUTTER_BIN_DIR:$PATH" flutter_rust_bridge_codegen generate)

# Build WASM
echo ""
echo "Building WASM..."
(cd "$FLUTTER_APP" && "$DART_CMD" run flutter_rust_bridge build-web --release \
    --rust-root ../../rhythm-lighting/rust/core/rhythm-core-ffi \
    --output web/pkg)

# Sync Dart-side content hash with compiled WASM
# FRB's build-web can produce a different content hash than codegen.
# Re-run codegen after build-web so the Dart bindings match the WASM binary.
echo ""
echo "Syncing Dart bindings with compiled WASM..."
(cd "$FLUTTER_APP" && PATH="$FLUTTER_BIN_DIR:$PATH" flutter_rust_bridge_codegen generate)

# Copy WASM files
WASM_SRC="$RUST_FFI_DIR/web/pkg/pkg"
WASM_DEST="$FLUTTER_APP/web/pkg"
DIST_WASM="$PROJECT_ROOT/dist/wasm"

if [ -d "$WASM_SRC" ]; then
    echo ""
    echo "Copying WASM files..."

    # Copy to Flutter app
    mkdir -p "$WASM_DEST"
    cp "$WASM_SRC"/* "$WASM_DEST/"
    echo "  -> $WASM_DEST"

    # Copy to dist
    mkdir -p "$DIST_WASM"
    cp "$WASM_SRC"/* "$DIST_WASM/"
    echo "  -> $DIST_WASM"
fi

echo ""
echo "WASM build complete"
