#!/bin/bash
# Build and flash ESP32-C6 firmware for Rhythm OS
#
# Usage: ./scripts/build-esp32.sh [OPTIONS]
#
# Options:
#   --release       Build in release mode (default: debug)
#   --flash         Flash to device after building (uses cargo run)
#   --monitor       Connect serial monitor only (no build, no flash)
#   --port PORT     Serial port (default: auto-detect)
#   --clean         Clean build directory first
#   -h, --help      Show this help

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
ESP32_DIR="$PROJECT_ROOT/rust/bins/rhythm-esp32"

# Defaults
RELEASE=false
FLASH=false
MONITOR=false
PORT=""
CLEAN=false
ZIGBEE=false

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            RELEASE=true
            shift
            ;;
        --flash)
            FLASH=true
            shift
            ;;
        --monitor)
            MONITOR=true
            shift
            ;;
        --port)
            PORT="$2"
            shift 2
            ;;
        --clean)
            CLEAN=true
            shift
            ;;
        --zigbee)
            ZIGBEE=true
            shift
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Build and flash ESP32-C6 firmware for Rhythm OS"
            echo ""
            echo "Options:"
            echo "  --release       Build in release mode (default: debug)"
            echo "  --flash         Flash to device and open monitor (uses cargo run)"
            echo "  --monitor       Connect serial monitor only (no build, no flash)"
            echo "  --port PORT     Serial port (default: auto-detect)"
            echo "  --clean         Clean build directory first"
            echo "  --zigbee        Enable Zigbee coordinator feature"
            echo "  -h, --help      Show this help"
            echo ""
            echo "Examples:"
            echo "  $0                      # Build only (debug)"
            echo "  $0 --flash              # Build and flash with monitor"
            echo "  $0 --release --flash    # Release build and flash"
            echo "  $0 --clean --flash      # Clean, build, and flash"
            echo "  $0 --monitor            # Just connect serial monitor"
            echo ""
            echo "Prerequisites:"
            echo "  1. Install ESP-IDF v5.5.2:"
            echo "     cd ~/esp && git clone --recursive https://github.com/espressif/esp-idf.git"
            echo "     cd esp-idf && ./install.sh esp32c6"
            echo ""
            echo "  2. Install Rust ESP tools:"
            echo "     cargo install ldproxy espflash"
            echo ""
            echo "WiFi Configuration:"
            echo "  Edit src/main.rs or set environment variables:"
            echo "     export WIFI_SSID=\"YourSSID\""
            echo "     export WIFI_PASS=\"YourPassword\""
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Find and source ESP-IDF environment
ESP_IDF_EXPORT=""
if [ -f "$HOME/esp/esp-idf/export.sh" ]; then
    ESP_IDF_EXPORT="$HOME/esp/esp-idf/export.sh"
elif [ -f "$HOME/export-esp.sh" ]; then
    ESP_IDF_EXPORT="$HOME/export-esp.sh"
elif [ -n "$IDF_PATH" ] && [ -f "$IDF_PATH/export.sh" ]; then
    ESP_IDF_EXPORT="$IDF_PATH/export.sh"
fi

if [ -z "$ESP_IDF_EXPORT" ]; then
    echo "Error: ESP-IDF environment not found"
    echo ""
    echo "Expected locations:"
    echo "  ~/esp/esp-idf/export.sh"
    echo "  ~/export-esp.sh"
    echo "  \$IDF_PATH/export.sh"
    echo ""
    echo "Install ESP-IDF:"
    echo "  cd ~/esp"
    echo "  git clone --recursive https://github.com/espressif/esp-idf.git"
    echo "  cd esp-idf && ./install.sh esp32c6"
    exit 1
fi

echo "Loading ESP-IDF environment from $ESP_IDF_EXPORT..."
source "$ESP_IDF_EXPORT"

# Set LIBCLANG_PATH for bindgen (required for clean builds)
if [ -z "$LIBCLANG_PATH" ]; then
    LLVM_PREFIX="$(brew --prefix llvm 2>/dev/null)"
    if [ -n "$LLVM_PREFIX" ] && [ -d "$LLVM_PREFIX/lib" ]; then
        export LIBCLANG_PATH="$LLVM_PREFIX/lib"
        echo "Set LIBCLANG_PATH=$LIBCLANG_PATH"
    fi
fi

# Check for required tools
if ! command -v ldproxy &> /dev/null; then
    echo "Error: ldproxy not found. Install with: cargo install ldproxy"
    exit 1
fi

if [ "$FLASH" = true ] || [ "$MONITOR" = true ]; then
    if ! command -v espflash &> /dev/null; then
        echo "Error: espflash not found. Install with: cargo install espflash"
        exit 1
    fi
fi

# Monitor mode: just connect the serial monitor, no build or flash
if [ "$MONITOR" = true ]; then
    echo ""
    echo "=== Connecting to ESP32 serial monitor ==="
    MONITOR_ARGS=""
    if [ -n "$PORT" ]; then
        MONITOR_ARGS="--port $PORT"
    fi
    espflash monitor --chip esp32c6 $MONITOR_ARGS
    exit 0
fi

cd "$ESP32_DIR"

# Auto-bump patch version in Cargo.toml
CARGO_TOML="$ESP32_DIR/Cargo.toml"
CURRENT_VERSION=$(grep -E '^version = "' "$CARGO_TOML" | head -1 | sed 's/version = "\(.*\)"/\1/')
if [ -n "$CURRENT_VERSION" ]; then
    MAJOR=$(echo "$CURRENT_VERSION" | cut -d. -f1)
    MINOR=$(echo "$CURRENT_VERSION" | cut -d. -f2)
    PATCH=$(echo "$CURRENT_VERSION" | cut -d. -f3)
    NEW_PATCH=$((PATCH + 1))
    NEW_VERSION="$MAJOR.$MINOR.$NEW_PATCH"
    sed -i '' "s/^version = \"$CURRENT_VERSION\"/version = \"$NEW_VERSION\"/" "$CARGO_TOML"
    echo "Version: $CURRENT_VERSION → $NEW_VERSION"
fi

# Load .env for WiFi credentials when flashing
if [ "$FLASH" = true ]; then
    for ENV_FILE in "$PROJECT_ROOT/.env" "$ESP32_DIR/.env"; do
        if [ -f "$ENV_FILE" ]; then
            # Export WIFI_SSID and WIFI_PASS if not already set
            if [ -z "$WIFI_SSID" ]; then
                val=$(grep -E '^WIFI_SSID=' "$ENV_FILE" | head -1 | cut -d'=' -f2-)
                [ -n "$val" ] && export WIFI_SSID="$val" && echo "Loaded WIFI_SSID from $ENV_FILE"
            fi
            if [ -z "$WIFI_PASS" ]; then
                val=$(grep -E '^WIFI_PASS=' "$ENV_FILE" | head -1 | cut -d'=' -f2-)
                [ -n "$val" ] && export WIFI_PASS="$val" && echo "Loaded WIFI_PASS from $ENV_FILE"
            fi
            break
        fi
    done
    if [ -n "$WIFI_SSID" ]; then
        echo "WiFi: SSID=$WIFI_SSID"
    fi
fi

# Clean if requested
if [ "$CLEAN" = true ]; then
    echo ""
    echo "=== Cleaning build directory ==="
    rm -rf target
fi

# Determine build mode and features
BUILD_MODE=""
FEATURES=""
if [ "$RELEASE" = true ]; then
    BUILD_MODE="--release"
fi
if [ "$ZIGBEE" = true ]; then
    FEATURES="--features zigbee"
fi

# Build and optionally flash
echo ""
if [ "$FLASH" = true ]; then
    echo "=== Building and flashing ESP32-C6 firmware ==="
    if [ "$RELEASE" = true ]; then
        echo "Mode: release"
        BUILD_DIR="target/riscv32imac-esp-espidf/release"
    else
        echo "Mode: debug"
        BUILD_DIR="target/riscv32imac-esp-espidf/debug"
    fi
    if [ "$ZIGBEE" = true ]; then
        echo "Zigbee: enabled"
    fi

    # Build first
    cargo build $BUILD_MODE $FEATURES

    # Find the esp-idf-sys build output directory
    ESP_IDF_BUILD_DIR=$(find "$BUILD_DIR/build/esp-idf-sys-"* -maxdepth 0 -type d 2>/dev/null | head -1)

    # Erase flash first for clean state
    echo ""
    echo "=== Erasing flash ==="
    espflash erase-flash --chip esp32c6

    # Find bootloader and partition table binaries
    BOOTLOADER_BIN="$ESP_IDF_BUILD_DIR/out/build/bootloader/bootloader.bin"
    PARTITION_BIN="$ESP_IDF_BUILD_DIR/out/build/partition_table/partition-table.bin"
    APP_ELF="$BUILD_DIR/rhythm-esp32"
    APP_BIN="$BUILD_DIR/rhythm-esp32.bin"

    # Convert ELF to binary using espflash
    echo ""
    echo "=== Converting ELF to binary ==="
    espflash save-image --chip esp32c6 "$APP_ELF" "$APP_BIN"

    # Flash all components using esptool.py for precise control
    echo ""
    echo "=== Flashing bootloader, partition table, and application ==="
    esptool.py --chip esp32c6 write_flash \
        0x0 "$BOOTLOADER_BIN" \
        0x8000 "$PARTITION_BIN" \
        0x20000 "$APP_BIN"

    # Open monitor
    echo ""
    echo "=== Opening monitor ==="
    espflash monitor --chip esp32c6
else
    echo "=== Building ESP32-C6 firmware ==="
    if [ "$RELEASE" = true ]; then
        echo "Mode: release"
    else
        echo "Mode: debug"
    fi
    if [ "$ZIGBEE" = true ]; then
        echo "Zigbee: enabled"
    fi
    if [ "$RELEASE" = true ]; then
        cargo build --release $FEATURES
        BINARY="target/riscv32imac-esp-espidf/release/rhythm-esp32"
    else
        cargo build $FEATURES
        BINARY="target/riscv32imac-esp-espidf/debug/rhythm-esp32"
    fi

    echo ""
    echo "Build complete: $ESP32_DIR/$BINARY"
    echo ""
    echo "To flash and monitor: $0 --flash"
    echo "To monitor only: $0 --monitor"
fi
