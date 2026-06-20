#!/bin/bash
# Build Docker addon image (headless, no web UI)
#
# Usage: ./tools/os/scripts/build-addon.sh [--arch amd64|aarch64|armv7|all] [--push]
# Orchestrates: build-rust -> docker build
# Output: Docker image dtconcepts/rhythm-os-addon

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"

# Defaults
ARCH="amd64"
PUSH=false
SKIP_RUST=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --arch)
            ARCH="$2"
            shift 2
            ;;
        --push)
            PUSH=true
            shift
            ;;
        --skip-rust)
            SKIP_RUST=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --arch <arch>     Target architecture: amd64, aarch64, armv7, or all"
            echo "  --push            Push image to registry after build"
            echo "  --skip-rust       Skip Rust build (use existing binaries)"
            echo "  -h, --help        Show this help"
            echo ""
            echo "Output: Docker image dtconcepts/rhythm-os-addon"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Build Rust binaries
if [ "$SKIP_RUST" = false ]; then
    echo "=== Building Rust binaries ==="
    if [ "$ARCH" = "all" ]; then
        "$SCRIPT_DIR/build-rust.sh" --release --target all
    else
        "$SCRIPT_DIR/build-rust.sh" --release --target "$ARCH"
    fi
    echo ""
fi

# Copy dist outputs to addon for Docker build
echo "=== Preparing addon directory ==="
ADDON_DIR="$PROJECT_ROOT/install/addon"

# Copy binaries
rm -rf "$ADDON_DIR/bin"
cp -r "$PROJECT_ROOT/dist/bin" "$ADDON_DIR/bin"
echo "Copied binaries to install/addon/bin/"

# Build Docker image
echo ""
echo "=== Building Docker image ==="
cd "$ADDON_DIR"

# Map arch to Docker platform (bash 3.2 compatible)
get_docker_platform() {
    case "$1" in
        amd64)   echo "linux/amd64" ;;
        aarch64) echo "linux/arm64" ;;
        armv7)   echo "linux/arm/v7" ;;
        *)       echo "linux/amd64" ;;
    esac
}

if [ "$ARCH" = "all" ]; then
    PLATFORMS="linux/amd64,linux/arm64,linux/arm/v7"
else
    PLATFORMS=$(get_docker_platform "$ARCH")
fi

IMAGE_NAME="dtconcepts/rhythm-os-addon"

if [ "$PUSH" = true ]; then
    docker buildx build \
        --platform "$PLATFORMS" \
        --push \
        -t "$IMAGE_NAME:latest" \
        .
else
    # For local builds, can only do single platform
    docker build \
        --build-arg TARGETARCH="$ARCH" \
        -t "$IMAGE_NAME:latest" \
        .
fi

echo ""
echo "Docker build complete: $IMAGE_NAME"
