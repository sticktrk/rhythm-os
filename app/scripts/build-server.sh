#!/bin/bash
# Build the rhythm-server binary
#
# Usage: ./scripts/build-server.sh [--release] [--target <target>] [--clean] [--run [-- args...]]
# Output: dist/bin/{os}-{arch}/rhythm-server

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

# Defaults
RELEASE=false
TARGET="native"
CLEAN=false
RUN=false
BUNDLE=false
SKIP_FLUTTER=false
SERVER_ARGS=()

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
        --clean)
            CLEAN=true
            shift
            ;;
        --run)
            RUN=true
            shift
            ;;
        --bundle)
            BUNDLE=true
            shift
            ;;
        --skip-flutter)
            SKIP_FLUTTER=true
            shift
            ;;
        --)
            shift
            SERVER_ARGS=("$@")
            break
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release           Build in release mode (default: debug)"
            echo "  --target <target>   Target platform (default: native)"
            echo "  --clean             Clean before building"
            echo "  --run               Run the server after building (native only)"
            echo "  --bundle            Embed Flutter web UI into the binary"
            echo "  --skip-flutter      Skip Flutter web build (use existing dist/flutter_web/)"
            echo "  -h, --help          Show this help"
            echo ""
            echo "Targets:"
            echo "  native              Host platform (default)"
            echo "  macos-arm64         macOS Apple Silicon (aarch64-apple-darwin)"
            echo "  macos-x86_64        macOS Intel (x86_64-apple-darwin)"
            echo "  linux-amd64         Linux x86_64 (x86_64-unknown-linux-musl)"
            echo "  linux-aarch64       Linux ARM64 (aarch64-unknown-linux-musl)"
            echo "  all-macos           Both macOS targets"
            echo "  all-linux           Both Linux musl targets"
            echo "  all                 All 4 targets"
            echo ""
            echo "Output: dist/bin/{os}-{arch}/rhythm-server"
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

# Map target names to Rust target triples
get_rust_target() {
    case "$1" in
        macos-arm64)    echo "aarch64-apple-darwin" ;;
        macos-x86_64)   echo "x86_64-apple-darwin" ;;
        linux-amd64)    echo "x86_64-unknown-linux-musl" ;;
        linux-aarch64)  echo "aarch64-unknown-linux-musl" ;;
        *)              echo "" ;;
    esac
}

# Get the native platform output dir name (e.g., macos-arm64)
get_native_output_dir() {
    local os arch
    case "$(uname -s)" in
        Darwin) os="macos" ;;
        Linux)  os="linux" ;;
        *)      os="$(uname -s | tr '[:upper:]' '[:lower:]')" ;;
    esac
    case "$(uname -m)" in
        x86_64)         arch="x86_64" ; [ "$os" = "linux" ] && arch="amd64" ;;
        aarch64|arm64)  arch="arm64" ; [ "$os" = "linux" ] && arch="aarch64" ;;
        *)              arch="$(uname -m)" ;;
    esac
    echo "${os}-${arch}"
}

# Check cross-compilation prerequisites for Linux musl from macOS
check_musl_cross() {
    local target="$1"
    if [ "$(uname -s)" != "Darwin" ]; then
        return 0
    fi

    case "$target" in
        linux-amd64)
            if ! command -v x86_64-linux-musl-gcc &>/dev/null; then
                echo "Error: x86_64-linux-musl-gcc not found."
                echo "Install with: brew install filosottile/musl-cross/musl-cross"
                exit 1
            fi
            ;;
        linux-aarch64)
            if ! command -v aarch64-linux-musl-gcc &>/dev/null; then
                echo "Error: aarch64-linux-musl-gcc not found."
                echo "Install with: brew install filosottile/musl-cross/musl-cross --with-aarch64"
                exit 1
            fi
            ;;
    esac
}

# Set linker environment for cross-compilation
setup_cross_env() {
    local target="$1"
    if [ "$(uname -s)" != "Darwin" ]; then
        return 0
    fi

    case "$target" in
        linux-amd64)
            export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER="x86_64-linux-musl-gcc"
            export CC_x86_64_unknown_linux_musl="x86_64-linux-musl-gcc"
            ;;
        linux-aarch64)
            export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER="aarch64-linux-musl-gcc"
            export CC_aarch64_unknown_linux_musl="aarch64-linux-musl-gcc"
            ;;
    esac
}

build_for_target() {
    local target="$1"
    local rust_target
    rust_target=$(get_rust_target "$target")

    if [ -z "$rust_target" ]; then
        echo "Unknown target: $target"
        exit 1
    fi

    echo "Building for $target ($rust_target)..."

    # Check prerequisites for cross-compilation
    check_musl_cross "$target"
    setup_cross_env "$target"

    # Ensure target is installed
    if ! rustup target list --installed | grep -q "$rust_target"; then
        echo "Adding target $rust_target..."
        rustup target add "$rust_target"
    fi

    cargo build $CARGO_FLAGS -p rhythm-server --target "$rust_target"

    # Copy to dist
    local output_dir="$PROJECT_ROOT/dist/bin/$target"
    mkdir -p "$output_dir"
    cp "$RUST_ROOT/target/$rust_target/$PROFILE/rhythm-server" "$output_dir/"
    cp "$RUST_ROOT/target/$rust_target/$PROFILE/rhythm-cli" "$output_dir/"
    echo "Output: dist/bin/$target/{rhythm-server,rhythm-cli}"
}

build_native() {
    echo "Building for native target..."
    cargo build $CARGO_FLAGS -p rhythm-server

    local output_dir_name
    output_dir_name=$(get_native_output_dir)
    local output_dir="$PROJECT_ROOT/dist/bin/$output_dir_name"
    mkdir -p "$output_dir"
    cp "$RUST_ROOT/target/$PROFILE/rhythm-server" "$output_dir/"
    cp "$RUST_ROOT/target/$PROFILE/rhythm-cli" "$output_dir/"
    echo "Output: dist/bin/$output_dir_name/{rhythm-server,rhythm-cli}"
}

# Build from the Rust workspace (may be in a submodule)
RUST_ROOT="$PROJECT_ROOT"
if [ -d "$PROJECT_ROOT/rhythm-lighting" ]; then
    RUST_ROOT="$PROJECT_ROOT/rhythm-lighting"
fi
cd "$RUST_ROOT"

# Build and embed Flutter web if requested
if [ "$BUNDLE" = true ]; then
    if [ "$SKIP_FLUTTER" = false ]; then
        echo "=== Building Flutter web ==="
        "$SCRIPT_DIR/build-flutter-web.sh" --context standalone_web
        echo ""
    fi
    if [ ! -d "$PROJECT_ROOT/dist/flutter_web" ]; then
        echo "Error: dist/flutter_web/ not found. Run: ./scripts/build-flutter-web.sh"
        exit 1
    fi
    export FLUTTER_WEB_DIR="$PROJECT_ROOT/dist/flutter_web"
    CARGO_FLAGS="$CARGO_FLAGS --features embed-web"
    echo "=== Building server with embedded web UI ==="
fi

if [ "$CLEAN" = true ]; then
    echo "Cleaning rhythm-server..."
    cargo clean -p rhythm-server
fi

MACOS_TARGETS="macos-arm64 macos-x86_64"
LINUX_TARGETS="linux-amd64 linux-aarch64"

case "$TARGET" in
    native)
        build_native
        ;;
    all-macos)
        for t in $MACOS_TARGETS; do
            build_for_target "$t"
        done
        ;;
    all-linux)
        for t in $LINUX_TARGETS; do
            build_for_target "$t"
        done
        ;;
    all)
        for t in $MACOS_TARGETS $LINUX_TARGETS; do
            build_for_target "$t"
        done
        ;;
    macos-arm64|macos-x86_64|linux-amd64|linux-aarch64)
        build_for_target "$TARGET"
        ;;
    *)
        echo "Unknown target: $TARGET"
        echo "Valid targets: native, macos-arm64, macos-x86_64, linux-amd64, linux-aarch64, all-macos, all-linux, all"
        exit 1
        ;;
esac

echo ""
echo "Server build complete"

if [ "$RUN" = true ]; then
    if [ "$TARGET" != "native" ]; then
        echo "Error: --run only works with native target"
        exit 1
    fi
    local_dir="$PROJECT_ROOT/dist/bin/$(get_native_output_dir)"
    echo "Running rhythm-server..."
    exec "$local_dir/rhythm-server" "${SERVER_ARGS[@]}"
fi
