#!/bin/bash
# Build a Raspberry Pi Zero SD-card image around the prebuilt Linux embedded appliance binary.
#
# Usage: ./scripts/build-rpiz-image.sh --buildroot-dir <path> [--release|--debug] [--dev|--prod] [--output-dir <path>] [--skip-server-build]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
DEFAULT_BUILDROOT_DIR="$PROJECT_ROOT/buildroot"
BUILDROOT_DIR="$DEFAULT_BUILDROOT_DIR"
OUTPUT_DIR="$PROJECT_ROOT/out/rpiz"
DEFAULT_OUTPUT_DIR="$PROJECT_ROOT/out/rpiz"
DEFAULT_DOCKER_OUTPUT_DIR="$PROJECT_ROOT/out/rpiz-docker"
OUTPUT_DIR_EXPLICIT=false
BUILD_MODE="release"
SKIP_SERVER_BUILD=false
WIFI_SSID="${RHYTHM_WIFI_SSID:-}"
WIFI_PSK="${RHYTHM_WIFI_PSK:-}"
WIFI_COUNTRY="${RHYTHM_WIFI_COUNTRY:-US}"
DEV_MODE="${RHYTHM_DEV_MODE:-1}"
DOCKER_BUILD=false
DOCKER_IMAGE="rhythm-rpiz-builder:local"
BUILDROOT_GIT_URL="${RHYTHM_BUILDROOT_GIT_URL:-https://git.buildroot.net/buildroot}"

is_truthy() {
    case "${1:-}" in
        1|true|TRUE|yes|YES|on|ON) return 0 ;;
        *) return 1 ;;
    esac
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --buildroot-dir)
            BUILDROOT_DIR="$2"
            shift 2
            ;;
        --output-dir)
            OUTPUT_DIR="$2"
            OUTPUT_DIR_EXPLICIT=true
            shift 2
            ;;
        --release)
            BUILD_MODE="release"
            shift
            ;;
        --debug)
            BUILD_MODE="debug"
            shift
            ;;
        --dev)
            DEV_MODE=1
            shift
            ;;
        --prod|--production)
            DEV_MODE=0
            shift
            ;;
        --skip-server-build)
            SKIP_SERVER_BUILD=true
            shift
            ;;
        --wifi-ssid)
            WIFI_SSID="$2"
            shift 2
            ;;
        --wifi-psk)
            WIFI_PSK="$2"
            shift 2
            ;;
        --wifi-country)
            WIFI_COUNTRY="$2"
            shift 2
            ;;
        --docker)
            DOCKER_BUILD=true
            shift
            ;;
        --docker-image)
            DOCKER_IMAGE="$2"
            shift 2
            ;;
        -h|--help)
            echo "Usage: $0 [OPTIONS]"
            echo ""
            echo "Options:"
            echo "  --buildroot-dir <path>  Path to a Buildroot checkout (default: $BUILDROOT_DIR)"
            echo "  --output-dir <path>     Buildroot output directory (default: $OUTPUT_DIR)"
            echo "  --release               Build dist/bin/rpiz/rhythm-server in release mode (default)"
            echo "  --debug                 Build dist/bin/rpiz/rhythm-server in debug mode first"
            echo "  --dev                   Build rpiz image in bring-up mode (default)"
            echo "  --prod, --production    Disable bring-up extras for a production image"
            echo "  --skip-server-build     Reuse existing dist/bin/rpiz/rhythm-server"
            echo "  --wifi-ssid <ssid>      Embed Wi-Fi SSID for Pi Zero W / Zero 2 W"
            echo "  --wifi-psk <psk>        Embed WPA/WPA2 passphrase"
            echo "  --wifi-country <code>   Wi-Fi regulatory country (default: $WIFI_COUNTRY)"
            echo "Environment:"
            echo "  RHYTHM_DEV_MODE=0       Disable rpiz bring-up mode; default is dev (Dropbear + known root password + Matter attestation bypass)"
            echo "  --docker                Run the Buildroot image step inside Docker"
            echo "  --docker-image <name>   Docker image tag to build/use (default: $DOCKER_IMAGE)"
            echo "  -h, --help              Show this help"
            exit 0
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

if { [ -n "$WIFI_SSID" ] && [ -z "$WIFI_PSK" ]; } || { [ -z "$WIFI_SSID" ] && [ -n "$WIFI_PSK" ]; }; then
    echo "Error: --wifi-ssid and --wifi-psk must be provided together"
    exit 1
fi

if [ -n "$WIFI_COUNTRY" ] && ! printf '%s' "$WIFI_COUNTRY" | grep -Eq '^[A-Za-z][A-Za-z]$'; then
    echo "Error: --wifi-country must be a 2-letter country code"
    exit 1
fi

run_server_build() {
    local server_flags
    server_flags=("--target" "rpiz")
    if [ "$BUILD_MODE" = "release" ]; then
        server_flags=("--release" "${server_flags[@]}")
    else
        server_flags=("--debug" "${server_flags[@]}")
    fi
    "$SCRIPT_DIR/build-server.sh" "${server_flags[@]}"
}

ensure_buildroot_checkout() {
    local buildroot_dir_abs default_buildroot_dir_abs

    if [ -f "$BUILDROOT_DIR/Makefile" ]; then
        return 0
    fi

    buildroot_dir_abs="$(cd "$(dirname "$BUILDROOT_DIR")" && pwd)/$(basename "$BUILDROOT_DIR")"
    default_buildroot_dir_abs="$DEFAULT_BUILDROOT_DIR"

    if [ -e "$BUILDROOT_DIR" ]; then
        echo "Error: $BUILDROOT_DIR exists but does not look like a Buildroot checkout"
        exit 1
    fi

    if [ "$buildroot_dir_abs" != "$default_buildroot_dir_abs" ]; then
        echo "Error: $BUILDROOT_DIR does not look like a Buildroot checkout"
        echo "The script only auto-clones the default path: $DEFAULT_BUILDROOT_DIR"
        exit 1
    fi

    if ! command -v git >/dev/null 2>&1; then
        echo "Error: git is required to clone Buildroot into $BUILDROOT_DIR"
        exit 1
    fi

    echo "Buildroot checkout not found at $BUILDROOT_DIR"
    echo "Cloning from $BUILDROOT_GIT_URL ..."
    git clone "$BUILDROOT_GIT_URL" "$BUILDROOT_DIR"
}

run_in_docker() {
    local project_root_abs buildroot_dir_abs output_dir_abs docker_args inner_args

    if ! command -v docker >/dev/null 2>&1; then
        echo "Error: docker is required for --docker"
        exit 1
    fi

    ensure_buildroot_checkout

    if [ "$SKIP_SERVER_BUILD" = false ]; then
        run_server_build
    fi

    if [ "$OUTPUT_DIR_EXPLICIT" = false ]; then
        OUTPUT_DIR="$DEFAULT_DOCKER_OUTPUT_DIR"
    fi

    project_root_abs="$(cd "$PROJECT_ROOT" && pwd)"
    buildroot_dir_abs="$(cd "$BUILDROOT_DIR" && pwd)"
    mkdir -p "$OUTPUT_DIR"
    output_dir_abs="$(cd "$OUTPUT_DIR" && pwd)"

    if [ -f "$OUTPUT_DIR/build/buildroot-config/conf" ] && file "$OUTPUT_DIR/build/buildroot-config/conf" 2>/dev/null | grep -q 'Mach-O'; then
        echo "Error: $OUTPUT_DIR contains macOS Buildroot host artifacts."
        echo "Use a fresh Docker output dir or remove the old one first."
        echo "Recommended:"
        echo "  rm -rf \"$OUTPUT_DIR\""
        echo "  ./scripts/build-rpiz-image.sh --release --buildroot-dir \"$BUILDROOT_DIR\" --docker"
        exit 1
    fi

    docker build \
        -t "$DOCKER_IMAGE" \
        -f "$PROJECT_ROOT/install/rpiz/docker/Dockerfile" \
        "$PROJECT_ROOT/install/rpiz/docker"

    docker_args=(
        run --rm -t
        -e HOME=/tmp/rhythm-home
        -e LANG=C.UTF-8
        -e LC_ALL=C.UTF-8
        --security-opt seccomp=unconfined
        -v "$project_root_abs:/workspace"
        -v "$buildroot_dir_abs:/buildroot"
        -v "$output_dir_abs:/output"
        -w /workspace
    )

    if command -v id >/dev/null 2>&1; then
        docker_args+=(--user "$(id -u):$(id -g)")
    fi

    if [ -n "$WIFI_SSID" ]; then
        docker_args+=(
            -e "RHYTHM_WIFI_SSID=$WIFI_SSID"
            -e "RHYTHM_WIFI_PSK=$WIFI_PSK"
            -e "RHYTHM_WIFI_COUNTRY=$WIFI_COUNTRY"
        )
    fi
    docker_args+=(-e "RHYTHM_DEV_MODE=$DEV_MODE")

    inner_args=("--buildroot-dir" "/buildroot" "--output-dir" "/output" "--skip-server-build")
    if [ "$BUILD_MODE" = "release" ]; then
        inner_args=("--release" "${inner_args[@]}")
    else
        inner_args=("--debug" "${inner_args[@]}")
    fi

    docker "${docker_args[@]}" "$DOCKER_IMAGE" bash -lc \
        "./scripts/build-rpiz-image.sh ${inner_args[*]}"
}

if [ "$DOCKER_BUILD" = true ]; then
    run_in_docker
    exit 0
fi

if [ "$(uname -s)" != "Linux" ]; then
    echo "Error: Buildroot image generation must run on Linux."
    echo "This script packages the rpiz binary into a Buildroot SD image, and"
    echo "Buildroot's host tooling does not support macOS as a build host."
    echo ""
    echo "Build the binary on macOS if you want:"
    echo "  ./scripts/build-server.sh --release --target rpiz"
    echo ""
    echo "Then run the image build from a Linux machine, Linux VM, or Docker:"
    echo "  ./scripts/build-rpiz-image.sh --release --buildroot-dir /path/to/buildroot --docker"
    exit 1
fi

ensure_buildroot_checkout

if [ "$SKIP_SERVER_BUILD" = false ]; then
    run_server_build
fi

SERVER_BINARY="$PROJECT_ROOT/dist/bin/rpiz/rhythm-server"
if [ ! -x "$SERVER_BINARY" ]; then
    echo "Error: Missing $SERVER_BINARY"
    echo "Build it first with: ./scripts/build-server.sh --release --target rpiz"
    exit 1
fi

if [ -n "$WIFI_SSID" ]; then
    WIFI_COUNTRY="$(printf '%s' "$WIFI_COUNTRY" | tr '[:lower:]' '[:upper:]')"
    echo "Embedding Wi-Fi config for SSID '$WIFI_SSID' (country $WIFI_COUNTRY)"
    export RHYTHM_WIFI_SSID="$WIFI_SSID"
    export RHYTHM_WIFI_PSK="$WIFI_PSK"
    export RHYTHM_WIFI_COUNTRY="$WIFI_COUNTRY"
fi
if is_truthy "$DEV_MODE"; then
    echo "Building rpiz image in dev mode (Dropbear, known root password, Matter attestation bypass)"
    export RHYTHM_DEV_MODE=1
else
    echo "Building rpiz image in production mode (no Dropbear, no known root password, Matter attestation enforced)"
    unset RHYTHM_DEV_MODE
fi

EXTERNAL_DIR="$PROJECT_ROOT/install/rpiz/buildroot"

make -C "$BUILDROOT_DIR" BR2_EXTERNAL="$EXTERNAL_DIR" O="$OUTPUT_DIR" rhythm_rpiz_defconfig
if is_truthy "${RHYTHM_DEV_MODE:-}"; then
    DEV_FRAGMENT="$OUTPUT_DIR/rhythm-dev.fragment"
    cat >"$DEV_FRAGMENT" <<'EOF'
BR2_TARGET_GENERIC_ROOT_PASSWD="rhythm"
BR2_PACKAGE_DROPBEAR=y
EOF
    cat "$DEV_FRAGMENT" >> "$OUTPUT_DIR/.config"
    make -C "$BUILDROOT_DIR" BR2_EXTERNAL="$EXTERNAL_DIR" O="$OUTPUT_DIR" olddefconfig
fi
# The prebuilt server comes from dist/bin/rpiz via a local-site package. Force
# that package to refresh each run so Buildroot does not reuse a stale unpacked
# copy when the host-side binary changes between image builds.
make -C "$BUILDROOT_DIR" BR2_EXTERNAL="$EXTERNAL_DIR" O="$OUTPUT_DIR" rhythm-prebuilt-dirclean
make -C "$BUILDROOT_DIR" BR2_EXTERNAL="$EXTERNAL_DIR" O="$OUTPUT_DIR"

rm -f "$OUTPUT_DIR/images/rootfs.ext2.gz" "$OUTPUT_DIR/images/sdcard.img.gz"
if command -v gzip >/dev/null 2>&1; then
    gzip -c -n "$OUTPUT_DIR/images/rootfs.ext2" > "$OUTPUT_DIR/images/rootfs.ext2.gz"
fi

echo ""
echo "RPi Zero image build complete"
echo "  Image: $OUTPUT_DIR/images/sdcard.img"
if [ -f "$OUTPUT_DIR/images/rootfs.ext2.gz" ]; then
    echo "  Rootfs OTA: $OUTPUT_DIR/images/rootfs.ext2.gz"
elif [ -f "$OUTPUT_DIR/images/rootfs.ext2" ]; then
    echo "  Rootfs OTA: $OUTPUT_DIR/images/rootfs.ext2"
fi
