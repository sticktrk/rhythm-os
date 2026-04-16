#!/bin/bash
# Build the Rust addon binary
#
# Usage: ./scripts/build-rust.sh [--release] [--target amd64|aarch64|armv7|all]
# Output: dist/bin/{arch}/rhythm-addon

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Defaults
RELEASE=false
TARGET="native"

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            RELEASE=true
            shift
            ;;
        --target)
            TARGET="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release           Build in release mode (default: debug)"
            echo "  --target <target>   Target architecture: native, amd64, aarch64, armv7, or all"
            echo "  -h, --help          Show this help"
            echo ""
            echo "Output: dist/bin/{arch}/rhythm-addon"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Determine build flags
if [ "$RELEASE" = true ]; then
    PROFILE="release"
    CARGO_FLAGS="--release"
else
    PROFILE="debug"
    CARGO_FLAGS=""
fi

BUILD_VERSION="$("$SCRIPT_DIR/resolve-version.sh" addon)"

# Map target names to Rust target triples (bash 3.2 compatible)
get_rust_target() {
    case "$1" in
        amd64)   echo "x86_64-unknown-linux-musl" ;;
        aarch64) echo "aarch64-unknown-linux-musl" ;;
        armv7)   echo "armv7-unknown-linux-musleabihf" ;;
        *)       echo "" ;;
    esac
}

build_for_target() {
    local arch="$1"
    local rust_target
    rust_target=$(get_rust_target "$arch")

    echo "Building for $arch ($rust_target)..."

    # Ensure target is installed
    if ! rustup target list --installed | grep -q "$rust_target"; then
        echo "Adding target $rust_target..."
        rustup target add "$rust_target"
    fi

    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        cargo build $CARGO_FLAGS -p rhythm-addon --target "$rust_target"

    # Copy to dist
    local output_dir="$PROJECT_ROOT/dist/bin/$arch"
    mkdir -p "$output_dir"
    cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-addon" "$output_dir/"
    echo "Output: dist/bin/$arch/rhythm-addon"
}

build_native() {
    echo "Building for native target..."
    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        cargo build $CARGO_FLAGS -p rhythm-addon

    # Determine native arch (arm64 is macOS Apple Silicon)
    local native_arch
    case "$(uname -m)" in
        x86_64)       native_arch="amd64" ;;
        aarch64|arm64) native_arch="aarch64" ;;
        arm*)         native_arch="armv7" ;;
        *)            native_arch="$(uname -m)" ;;
    esac

    local output_dir="$PROJECT_ROOT/dist/bin/$native_arch"
    mkdir -p "$output_dir"
    cp "$PROJECT_ROOT/target/$PROFILE/rhythm-addon" "$output_dir/"
    echo "Output: dist/bin/$native_arch/rhythm-addon"
}

cd "$PROJECT_ROOT"

case "$TARGET" in
    native)
        build_native
        ;;
    all)
        for arch in amd64 aarch64 armv7; do
            build_for_target "$arch"
        done
        ;;
    amd64|aarch64|armv7)
        build_for_target "$TARGET"
        ;;
    *)
        echo "Unknown target: $TARGET"
        echo "Valid targets: native, amd64, aarch64, armv7, all"
        exit 1
        ;;
esac

echo ""
echo "Rust build complete"
