#!/bin/bash
# Run the Rhythm OS addon locally for development
#
# Usage: ./tools/os/scripts/run-dev.sh [--skip-build] [--debug]
# Loads .env from project root, builds if needed, runs addon

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"

# Defaults
SKIP_BUILD=false
DEBUG=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --skip-build)
            SKIP_BUILD=true
            shift
            ;;
        --debug)
            DEBUG=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --skip-build    Skip all builds (use existing binaries)"
            echo "  --debug         Build/run debug instead of release"
            echo "  -h, --help      Show this help"
            echo ""
            echo "Prerequisites:"
            echo "  - Copy .env.example to .env and configure it"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

cd "$PROJECT_ROOT"

# Load environment variables
if [ -f .env ]; then
    echo "Loading environment from .env"
    set -a
    source .env
    set +a
else
    echo "Error: No .env file found. Copy .env.example to .env and configure it."
    exit 1
fi

# Determine native arch (arm64 is macOS Apple Silicon)
case "$(uname -m)" in
    x86_64)       NATIVE_ARCH="amd64" ;;
    aarch64|arm64) NATIVE_ARCH="aarch64" ;;
    arm*)         NATIVE_ARCH="armv7" ;;
    *)            NATIVE_ARCH="$(uname -m)" ;;
esac

BINARY="$PROJECT_ROOT/dist/bin/$NATIVE_ARCH/rhythm-addon"

# Build if needed
if [ "$SKIP_BUILD" = false ]; then
    echo ""
    echo "=== Building Rust addon ==="
    if [ "$DEBUG" = true ]; then
        "$SCRIPT_DIR/build-rust.sh"
    else
        "$SCRIPT_DIR/build-rust.sh" --release
    fi
fi

if [ ! -f "$BINARY" ]; then
    echo "Error: Binary not found at $BINARY"
    echo "Run: ./tools/os/scripts/build-rust.sh"
    exit 1
fi

echo ""
echo "Starting Rhythm OS addon..."
echo "  HA_HOST: $HA_HOST"
echo "  HA_PORT: $HA_PORT"
echo "  API:     http://localhost:${INGRESS_PORT:-8099}"
echo ""

exec "$BINARY"
