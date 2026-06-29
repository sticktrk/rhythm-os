#!/bin/bash
# Build the Rhythm OS native binaries.
#
# Usage: ./tools/os/scripts/build-server.sh [--release|--debug] [--target <target>] [--clean] [--run] [--data-dir <path>] [-- args...]
# Output: dist/bin/{target}/...

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$PROJECT_ROOT/target}"

# Defaults
BUILD_MODE="auto"
TARGET="native"
CLEAN=false
RUN=false
DEBUG_LOG=false
SERVER_ARGS=()

# Parse arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --release)
            BUILD_MODE="release"
            DEBUG_LOG=false
            shift
            ;;
        --debug)
            BUILD_MODE="debug"
            DEBUG_LOG=true
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
        --data-dir)
            SERVER_ARGS+=(--data-dir "$2")
            shift 2
            ;;
        --)
            shift
            SERVER_ARGS+=("$@")
            break
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --release           Build in release mode"
            echo "  --debug             Build in debug mode; with --run, sets --log-level debug"
            echo "  --target <target>   Target platform (default: native)"
            echo "  --clean             Clean before building"
            echo "  --run               Run the server after building (native only; default: debug)"
            echo "  --data-dir <path>   Override server data directory when used with --run"
            echo "  -h, --help          Show this help"
            echo ""
            echo "Targets:"
            echo "  native              Host platform (default)"
            echo "  macos-arm64         macOS Apple Silicon (aarch64-apple-darwin)"
            echo "  macos-x86_64        macOS Intel (x86_64-apple-darwin)"
            echo "  linux-amd64         Linux x86_64 (x86_64-unknown-linux-musl)"
            echo "  linux-aarch64       Linux ARM64 (aarch64-unknown-linux-musl)"
            echo "  rpiz                Raspberry Pi Zero / Zero W (arm-unknown-linux-musleabihf)"
            echo "  all-macos           Both macOS targets"
            echo "  all-linux           All Linux musl targets, including rpiz"
            echo "  all                 All supported targets"
            echo ""
            echo "Output: dist/bin/{target}/..."
            echo ""
            echo "Examples:"
            echo "  $0 --run --data-dir /tmp/rhythm-dev"
            echo "  $0 --run -- --port 54449"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Determine build flags
case "$BUILD_MODE" in
    release)
        PROFILE="release"
        CARGO_FLAGS="--release"
        ;;
    debug)
        PROFILE="debug"
        CARGO_FLAGS=""
        ;;
    auto)
        PROFILE="debug"
        CARGO_FLAGS=""
        ;;
    *)
        echo "Unknown build mode: $BUILD_MODE"
        exit 1
        ;;
esac

BUILD_VERSION="$("$SCRIPT_DIR/resolve-version.sh" server)"

server_args_include_log_level() {
    local arg
    for arg in "${SERVER_ARGS[@]}"; do
        case "$arg" in
            --log-level|--log-level=*)
                return 0
                ;;
        esac
    done
    return 1
}

# Map target names to Rust target triples
get_rust_target() {
    case "$1" in
        macos-arm64)    echo "aarch64-apple-darwin" ;;
        macos-x86_64)   echo "x86_64-apple-darwin" ;;
        linux-amd64)    echo "x86_64-unknown-linux-musl" ;;
        linux-aarch64)  echo "aarch64-unknown-linux-musl" ;;
        rpiz)           echo "arm-unknown-linux-musleabihf" ;;
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
        armv6l|arm1176*) arch="armv6l" ; [ "$os" = "linux" ] && arch="rpiz" ;;
        *)              arch="$(uname -m)" ;;
    esac
    echo "${os}-${arch}"
}

# Check cross-compilation prerequisites for Linux musl from macOS
check_musl_cross() {
    local target="$1"
    if [ "$(uname -s)" != "Darwin" ] && [ "$target" != "rpiz" ]; then
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
        rpiz)
            if ! command -v arm-unknown-linux-musleabihf-gcc &>/dev/null \
                && ! command -v arm-linux-musleabihf-gcc &>/dev/null; then
                echo "Error: no ARMv6 hard-float musl cross compiler on PATH."
                echo "Install one (e.g. crosstool-NG arm-unknown-linux-musleabihf, or"
                echo "brew install filosottile/musl-cross/musl-cross) and re-run."
                exit 1
            fi
            ;;
    esac
}

# Set linker environment for cross-compilation
setup_cross_env() {
    local target="$1"
    if [ "$(uname -s)" != "Darwin" ] && [ "$target" != "rpiz" ]; then
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
        rpiz)
            # Prefer the crosstool-NG "arm-unknown-linux-musleabihf-gcc" when present
            # since CHIP's libCHIP.a is typically built against that toolchain's
            # libstdc++; fall back to the musl-cross "arm-linux-musleabihf-gcc".
            # If the crosstool-NG toolchain is installed at its default location
            # but not on PATH, add it so `command -v` can find it below.
            local ctng_bin="${HOME}/x-tools/arm-unknown-linux-musleabihf/bin"
            if [ -x "$ctng_bin/arm-unknown-linux-musleabihf-gcc" ]; then
                case ":$PATH:" in
                    *":$ctng_bin:"*) ;;
                    *) export PATH="$ctng_bin:$PATH" ;;
                esac
            fi
            local rpiz_cross="arm-linux-musleabihf-gcc"
            if command -v arm-unknown-linux-musleabihf-gcc &>/dev/null; then
                rpiz_cross="arm-unknown-linux-musleabihf-gcc"
            fi
            export CARGO_TARGET_ARM_UNKNOWN_LINUX_MUSLEABIHF_LINKER="$rpiz_cross"
            export CC_arm_unknown_linux_musleabihf="$rpiz_cross"
            if [ -z "${RHYTHM_CHIP_SYSROOT:-}" ]; then
                local buildroot_sysroot="$PROJECT_ROOT/out/rpiz/staging"
                if [ -d "$buildroot_sysroot" ]; then
                    export RHYTHM_CHIP_SYSROOT="$buildroot_sysroot"
                fi
            fi
            ;;
    esac
}

CHIPD_FEATURE_ARGS=()

should_enable_chip_ffi() {
    local target="$1"

    if [ -n "${RHYTHM_CHIPD_FEATURES:-}" ]; then
        return 0
    fi

    if [ "$target" = "native" ]; then
        [ -n "${RHYTHM_CHIP_ROOT:-}" ] || [ -n "${RHYTHM_CHIP_OUT_DIR:-}" ] || [ -n "${RHYTHM_CHIP_LIB_DIR:-}" ]
        return $?
    fi

    [ -n "${RHYTHM_CHIP_OUT_DIR:-}" ] || [ -n "${RHYTHM_CHIP_LIB_DIR:-}" ]
}

collect_chipd_feature_args() {
    local target="$1"
    CHIPD_FEATURE_ARGS=()

    if [ -n "${RHYTHM_CHIPD_FEATURES:-}" ]; then
        CHIPD_FEATURE_ARGS+=(--features "$RHYTHM_CHIPD_FEATURES")
        return
    fi

    if should_enable_chip_ffi "$target"; then
        CHIPD_FEATURE_ARGS+=(--features chip-ffi)
    fi
}

should_use_cross() {
    case "$1" in
        rpiz|linux-amd64|linux-aarch64) ;;
        *) return 1 ;;
    esac
    [ "$(uname -s)" = "Linux" ] || return 1
    command -v cross &>/dev/null || return 1

    # For direct CHIP FFI builds, prefer the host toolchain over `cross` so
    # RHYTHM_CHIP_OUT_DIR / RHYTHM_CHIP_LIB_DIR resolve against the real host
    # filesystem instead of the container's /project mount.
    [ -z "${RHYTHM_CHIP_OUT_DIR:-}" ] && [ -z "${RHYTHM_CHIP_LIB_DIR:-}" ]
}

build_for_target() {
    local target="$1"
    local rust_target
    local builder="cargo"
    local package="rhythm-server"
    local bins=(rhythm-server rhythm-cli)
    rust_target=$(get_rust_target "$target")

    if [ -z "$rust_target" ]; then
        echo "Unknown target: $target"
        exit 1
    fi

    if [ "$target" = "rpiz" ]; then
        package="rhythm-linux-appliance"
        bins=(rhythm-linux-appliance)
    fi

    echo "Building for $target ($rust_target)..."

    # Prefer direct cargo+linker cross-compilation on macOS, where `cross`
    # can select an incompatible Linux host toolchain for ARMv6 targets.
    # On Linux, prefer `cross` for rpiz when available.
    if should_use_cross "$target"; then
        builder="cross"
    else
        check_musl_cross "$target"
        setup_cross_env "$target"
    fi

    # Ensure target is installed
    if ! rustup target list --installed | grep -q "$rust_target"; then
        echo "Adding target $rust_target..."
        rustup target add "$rust_target"
    fi

    local cargo_bin_flags=()
    local bin
    for bin in "${bins[@]}"; do
        cargo_bin_flags+=(--bin "$bin")
    done
    collect_chipd_feature_args "$target"

    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        "$builder" build $CARGO_FLAGS -p "$package" --target "$rust_target" "${cargo_bin_flags[@]}"

    # chip-ffi links to glib/dbus/avahi, which Buildroot only provides as shared libs
    # (avahi explicitly can't be built static). Drop +crt-static for rhythm-chipd on
    # musl targets so the linker can pick up the .so files; the Buildroot rootfs
    # already ships musl's dynamic loader and the matching shared libs.
    local chipd_rustflags_var=""
    local chipd_previous_rustflags=""
    local chipd_restore_rustflags=false
    if [ ${#CHIPD_FEATURE_ARGS[@]} -gt 0 ] \
        && printf '%s\n' "${CHIPD_FEATURE_ARGS[@]}" | grep -q chip-ffi \
        && [[ "$rust_target" == *-linux-musl* ]]; then
        chipd_rustflags_var="CARGO_TARGET_$(printf '%s' "$rust_target" | tr 'a-z-' 'A-Z_')_RUSTFLAGS"
        if [ "${!chipd_rustflags_var+set}" = set ]; then
            chipd_previous_rustflags="${!chipd_rustflags_var}"
            chipd_restore_rustflags=true
        fi
        local chipd_required_rustflags="-C target-feature=-crt-static"
        if [ -n "$chipd_previous_rustflags" ]; then
            export "$chipd_rustflags_var=$chipd_previous_rustflags $chipd_required_rustflags"
        else
            export "$chipd_rustflags_var=$chipd_required_rustflags"
        fi
    fi

    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        "$builder" build $CARGO_FLAGS -p rhythm-chipd --target "$rust_target" --bin rhythm-chipd "${CHIPD_FEATURE_ARGS[@]}"

    if [ -n "$chipd_rustflags_var" ]; then
        if [ "$chipd_restore_rustflags" = true ]; then
            export "$chipd_rustflags_var=$chipd_previous_rustflags"
        else
            unset "$chipd_rustflags_var"
        fi
    fi

    # Copy to dist
    local output_dir="$PROJECT_ROOT/dist/bin/$target"
    mkdir -p "$output_dir"
    if [ "$target" = "rpiz" ]; then
        cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-linux-appliance" "$output_dir/rhythm-server"
        cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-chipd" "$output_dir/"
        echo "Output: dist/bin/$target/{rhythm-server,rhythm-chipd}"
    else
        cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-server" "$output_dir/"
        cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-cli" "$output_dir/"
        cp "$PROJECT_ROOT/target/$rust_target/$PROFILE/rhythm-chipd" "$output_dir/"
        echo "Output: dist/bin/$target/{rhythm-server,rhythm-cli,rhythm-chipd}"
    fi
}

build_native() {
    echo "Building for native target..."

    local bins=(rhythm-server rhythm-cli)
    local cargo_bin_flags=()
    local bin

    if [ "$RUN" = true ]; then
        bins=(rhythm-server)
    fi

    for bin in "${bins[@]}"; do
        cargo_bin_flags+=(--bin "$bin")
    done
    collect_chipd_feature_args "native"

    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        cargo build $CARGO_FLAGS -p rhythm-server "${cargo_bin_flags[@]}"
    RHYTHM_BUILD_VERSION="$BUILD_VERSION" \
        cargo build $CARGO_FLAGS -p rhythm-chipd --bin rhythm-chipd "${CHIPD_FEATURE_ARGS[@]}"

    if [ "$RUN" = true ]; then
        echo "Built: target/$PROFILE/{rhythm-server,rhythm-chipd}"
        return
    fi

    local output_dir_name
    output_dir_name=$(get_native_output_dir)
    local output_dir="$PROJECT_ROOT/dist/bin/$output_dir_name"
    mkdir -p "$output_dir"
    for bin in "${bins[@]}"; do
        cp "$PROJECT_ROOT/target/$PROFILE/$bin" "$output_dir/"
    done
    cp "$PROJECT_ROOT/target/$PROFILE/rhythm-chipd" "$output_dir/"
    echo "Output: dist/bin/$output_dir_name/{rhythm-server,rhythm-cli,rhythm-chipd}"
}

cd "$PROJECT_ROOT"

if [ "$CLEAN" = true ]; then
    echo "Cleaning target artifacts..."
    cargo clean -p rhythm-server
    cargo clean -p rhythm-chipd
    cargo clean -p rhythm-linux-appliance
fi

MACOS_TARGETS="macos-arm64 macos-x86_64"
LINUX_TARGETS="linux-amd64 linux-aarch64 rpiz"

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
    macos-arm64|macos-x86_64|linux-amd64|linux-aarch64|rpiz)
        build_for_target "$TARGET"
        ;;
    *)
        echo "Unknown target: $TARGET"
        echo "Valid targets: native, macos-arm64, macos-x86_64, linux-amd64, linux-aarch64, rpiz, all-macos, all-linux, all"
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
    if [ "$DEBUG_LOG" = true ] && ! server_args_include_log_level; then
        SERVER_ARGS=(--log-level debug "${SERVER_ARGS[@]}")
    fi
    echo "Running rhythm-server..."
    exec "$PROJECT_ROOT/target/$PROFILE/rhythm-server" "${SERVER_ARGS[@]}"
fi
