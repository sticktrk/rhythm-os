#!/bin/bash
# Setup ESP-IDF development environment for ESP32-C6
# Based on: https://docs.espressif.com/projects/esp-idf/en/stable/esp32c6/get-started/linux-macos-setup.html
#
# This script installs ESP-IDF and required tools for building the Rhythm Lighting ESP32 firmware.
# Run once to set up your development environment.
#
# Usage: ./scripts/setup-esp32.sh

set -e

echo "=== ESP-IDF Setup for Rhythm Lighting ESP32-C6 ==="
echo ""

# Step 1: Install Prerequisites
echo "Step 1: Checking prerequisites..."

if ! command -v brew &> /dev/null; then
    echo "Error: Homebrew not found. Install from https://brew.sh"
    exit 1
fi

echo "Installing required packages via Homebrew..."
brew install cmake ninja dfu-util python3

# Also install llvm for libclang (needed by bindgen for Rust builds)
echo "Installing LLVM (for Rust bindgen)..."
brew install llvm

echo "Prerequisites installed."
echo ""

# Step 2: Get ESP-IDF
ESP_DIR="$HOME/esp"
IDF_DIR="$ESP_DIR/esp-idf"
IDF_VERSION="v5.5.2"

echo "Step 2: Getting ESP-IDF..."

if [ -d "$IDF_DIR" ]; then
    echo "ESP-IDF directory already exists at $IDF_DIR"
    echo "Checking version..."
    cd "$IDF_DIR"
    CURRENT_VERSION=$(git describe --tags 2>/dev/null || echo "unknown")
    echo "Current version: $CURRENT_VERSION"

    if [ "$CURRENT_VERSION" != "$IDF_VERSION" ]; then
        echo ""
        echo "Warning: Expected version $IDF_VERSION but found $CURRENT_VERSION"
        echo "To update, run:"
        echo "  cd $IDF_DIR && git fetch && git checkout $IDF_VERSION && git submodule update --init --recursive"
        echo ""
    fi
else
    echo "Cloning ESP-IDF $IDF_VERSION to $IDF_DIR..."
    mkdir -p "$ESP_DIR"
    cd "$ESP_DIR"
    git clone -b "$IDF_VERSION" --recursive https://github.com/espressif/esp-idf.git
fi

echo ""

# Step 3: Set up the Tools
echo "Step 3: Setting up ESP-IDF tools for ESP32-C6..."
cd "$IDF_DIR"
./install.sh esp32c6

echo ""

# Step 4: Set up Environment Variables
echo "Step 4: Environment setup..."
echo ""
echo "ESP-IDF installed successfully!"
echo ""
echo "To use ESP-IDF, run this command in your terminal before building:"
echo ""
echo "  . $IDF_DIR/export.sh"
echo ""
echo "Or add this alias to your ~/.zshrc or ~/.bashrc:"
echo ""
echo "  alias get_idf='. $IDF_DIR/export.sh'"
echo ""
echo "Then use 'get_idf' to activate the ESP-IDF environment."
echo ""

# Step 5: Install Rust ESP tools
echo "Step 5: Installing Rust tools for ESP32..."

if ! command -v cargo &> /dev/null; then
    echo "Error: Rust not found. Install from https://rustup.rs"
    exit 1
fi

echo "Installing ldproxy and espflash..."
cargo install ldproxy espflash

echo ""
echo "=== Setup Complete ==="
echo ""
echo "Next steps:"
echo "  1. Open a new terminal or run: . $IDF_DIR/export.sh"
echo "  2. Build the firmware: ./scripts/build-esp32.sh"
echo "  3. Flash to device: ./scripts/build-esp32.sh --flash"
echo ""
