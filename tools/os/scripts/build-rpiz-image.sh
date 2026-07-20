#!/bin/bash
# Build a Raspberry Pi Zero SD-card image around the prebuilt Linux appliance binary.
#
# Usage: ./tools/os/scripts/build-rpiz-image.sh --buildroot-dir <path> [--release|--debug] [--dev|--prod] [--output-dir <path>] [--skip-server-build]

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/../../../os" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../../.." && pwd)"
DEFAULT_BUILDROOT_DIR="$PROJECT_ROOT/buildroot"
BUILDROOT_DIR="$DEFAULT_BUILDROOT_DIR"
OUTPUT_DIR="$PROJECT_ROOT/out/rpiz"
DEFAULT_OUTPUT_DIR="$PROJECT_ROOT/out/rpiz"
DEFAULT_DOCKER_OUTPUT_DIR="$PROJECT_ROOT/out/rpiz-docker"
OUTPUT_DIR_EXPLICIT=false
BUILDROOT_DL_DIR="${RHYTHM_BUILDROOT_DL_DIR:-}"
BUILD_MODE="release"
SKIP_SERVER_BUILD=false
WIFI_SSID="${RHYTHM_WIFI_SSID:-}"
WIFI_PSK="${RHYTHM_WIFI_PSK:-}"
WIFI_COUNTRY="${RHYTHM_WIFI_COUNTRY:-US}"
DEV_MODE="${RHYTHM_DEV_MODE:-1}"
DOCKER_BUILD=false
BUILDER_LOCK_FILE="$PROJECT_ROOT/install/rpiz/builder-image.lock"
read_lock_field() {
    # Read key=value entries from builder-image.lock, skipping comments.
    [ -f "$BUILDER_LOCK_FILE" ] || return 0
    awk -F= -v key="$1" '
        /^[[:space:]]*#/ || /^[[:space:]]*$/ { next }
        $1 == key { sub(/^[^=]*=/, ""); print; exit }
    ' "$BUILDER_LOCK_FILE"
}
resolve_docker_image() {
    # Precedence: explicit env > builder-image.lock > :latest fallback.
    if [ -n "${RHYTHM_RPIZ_BUILDER_IMAGE:-}" ]; then
        echo "$RHYTHM_RPIZ_BUILDER_IMAGE"
        return
    fi
    local from_lock
    from_lock="$(read_lock_field image)"
    if [ -n "$from_lock" ]; then
        echo "$from_lock"
        return
    fi
    echo "dtconcepts/rhythm-rpiz-builder:latest"
}
DOCKER_IMAGE="$(resolve_docker_image)"
DOCKER_PULL=true
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
        --no-pull)
            DOCKER_PULL=false
            shift
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
            echo "  RHYTHM_BUILDROOT_DL_DIR Override Buildroot download directory (default: <output-dir>/dl)"
            echo "  --docker                Assemble the image inside dtconcepts/rhythm-rpiz-builder"
            echo "  --docker-image <name>   Docker image ref to use (default: $DOCKER_IMAGE)"
            echo "  --no-pull               Skip docker pull; use the locally cached image"
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
    local repo_root_abs output_dir_abs docker_args inner_args

    if ! command -v docker >/dev/null 2>&1; then
        echo "Error: docker is required for --docker"
        exit 1
    fi

    if [ "$OUTPUT_DIR_EXPLICIT" = false ]; then
        OUTPUT_DIR="$DEFAULT_DOCKER_OUTPUT_DIR"
    fi

    repo_root_abs="$(cd "$REPO_ROOT" && pwd)"
    mkdir -p "$OUTPUT_DIR"
    output_dir_abs="$(cd "$OUTPUT_DIR" && pwd)"

    if [ "$DOCKER_PULL" = true ]; then
        echo "Pulling $DOCKER_IMAGE"
        docker pull "$DOCKER_IMAGE"
    fi

    docker_args=(
        run --rm -t
        -e HOME=/tmp/rhythm-home
        -e LANG=C.UTF-8
        -e LC_ALL=C.UTF-8
        --security-opt seccomp=unconfined
        -v "$repo_root_abs:/workspace"
        -v "$output_dir_abs:/output"
        -w /workspace/os
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
    docker_args+=(-e "RHYTHM_IMAGE_FINGERPRINT=$RHYTHM_IMAGE_FINGERPRINT")
    if [ -n "${RHYTHM_IMAGE_VERSION:-}" ]; then
        docker_args+=(-e "RHYTHM_IMAGE_VERSION=$RHYTHM_IMAGE_VERSION")
    fi
    if [ -n "${RHYTHM_BUILD_VERSION:-}" ]; then
        docker_args+=(-e "RHYTHM_BUILD_VERSION=$RHYTHM_BUILD_VERSION")
    fi
    # Forward the baked output prefix so the inner seed step can rewrite
    # Buildroot's hardcoded paths (fakeroot wrapper, *.pc, *.la, libtool) to
    # $OUTPUT_DIR. Normally unset: the builder image bakes the prefix into the
    # /opt/rpiz-out/.rhythm-baked-prefix sidecar, which the inner run reads.
    # RHYTHM_BAKED_OUTPUT_PREFIX remains a manual override.
    if [ -n "${RHYTHM_BAKED_OUTPUT_PREFIX:-}" ]; then
        docker_args+=(-e "RHYTHM_BAKED_OUTPUT_PREFIX=$RHYTHM_BAKED_OUTPUT_PREFIX")
    fi

    # The builder image bakes Buildroot at /opt/buildroot. Chip prebuilts and
    # the cross toolchain are on PATH / in RHYTHM_CHIP_OUT_DIR via the
    # Dockerfile ENV block, so the inner build-server.sh + Buildroot run get
    # everything they need without more -e flags.
    inner_args=("--buildroot-dir" "/opt/buildroot" "--output-dir" "/output")
    if [ "$BUILD_MODE" = "release" ]; then
        inner_args=("--release" "${inner_args[@]}")
    else
        inner_args=("--debug" "${inner_args[@]}")
    fi

    # bash -c (not -lc): a login shell would re-source /etc/profile and blow
    # away the PATH / RHYTHM_CHIP_* ENV the Dockerfile bakes in, hiding the
    # ARMv6 musl toolchain from build-server.sh.
    docker "${docker_args[@]}" "$DOCKER_IMAGE" bash -c \
        "../tools/os/scripts/build-rpiz-image.sh ${inner_args[*]}"
}

# Rootfs fingerprint: stamped into the image (/etc/rhythm-image-fingerprint)
# and into OTA manifests so the CI release gate can decide binary-only vs
# full-image releases. Computed host-side so --docker runs stamp exactly the
# fingerprint the release gate compared against the published feed.
if is_truthy "$DEV_MODE"; then
    FINGERPRINT_MODE=dev
else
    FINGERPRINT_MODE=prod
fi
RHYTHM_IMAGE_FINGERPRINT="${RHYTHM_IMAGE_FINGERPRINT:-$("$SCRIPT_DIR/compute-rootfs-fingerprint.sh" --image-mode "$FINGERPRINT_MODE")}"
export RHYTHM_IMAGE_FINGERPRINT

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
    echo "  ./tools/os/scripts/build-server.sh --release --target rpiz"
    echo ""
    echo "Then run the image build from a Linux machine, Linux VM, or Docker:"
    echo "  ./tools/os/scripts/build-rpiz-image.sh --release --buildroot-dir /path/to/buildroot --docker"
    exit 1
fi

ensure_buildroot_checkout

# When running inside the builder image, seed OUTPUT_DIR from the baked
# Buildroot output so make resumes (host toolchain + per-package build trees)
# instead of starting over from scratch. Only activates when /opt/rpiz-out is
# present (builder image) AND the output dir has no host/ yet (cold start).
if [ -d /opt/rpiz-out ] && [ ! -d "$OUTPUT_DIR/host" ]; then
    if command -v rsync >/dev/null 2>&1; then
        echo "Seeding $OUTPUT_DIR from baked /opt/rpiz-out"
        mkdir -p "$OUTPUT_DIR"
        rsync -a /opt/rpiz-out/ "$OUTPUT_DIR/"

        # Buildroot's host/ tools (fakeroot wrapper, *.pc, *.la, libtool,
        # per-package Makefiles) embed the build-time absolute prefix. Rewrite
        # the seeded copy so the scripts find their libs at the new location.
        # Source of truth, in order: RHYTHM_BAKED_OUTPUT_PREFIX env (lets us
        # test against images that predate the sidecar), then the sidecar
        # written by build-rpiz-builder-image.sh.
        baked_prefix="${RHYTHM_BAKED_OUTPUT_PREFIX:-}"
        if [ -z "$baked_prefix" ] && [ -f "$OUTPUT_DIR/.rhythm-baked-prefix" ]; then
            baked_prefix="$(tr -d '[:space:]' < "$OUTPUT_DIR/.rhythm-baked-prefix")"
        fi
        if [ -n "$baked_prefix" ] && [ "$baked_prefix" != "$OUTPUT_DIR" ]; then
            echo "  Rewriting baked paths: $baked_prefix -> $OUTPUT_DIR"
            # -I skips binary files; -l gives files that match. Escape
            # slashes by using | as the sed delimiter.
            grep -rlI "$baked_prefix" "$OUTPUT_DIR" 2>/dev/null \
                | xargs -r sed -i "s|$baked_prefix|$OUTPUT_DIR|g"
        fi

        # images/ is excluded from the bake because sdcard.img / rootfs.* /
        # boot.vfat are regenerated every run and would just bloat the image.
        # But some packages (rpi-firmware, linux kernel) install their
        # artifacts *into* images/ during their install phase. Their
        # .stamp_images_installed survived in build/, so Buildroot thinks
        # they're done and skips the install. Drop those stamps so the
        # install phase re-runs and repopulates images/rpi-firmware/,
        # images/zImage, and any device trees.
        find "$OUTPUT_DIR/build" -maxdepth 2 -name '.stamp_images_installed' -delete 2>/dev/null || true
    else
        echo "Warning: /opt/rpiz-out is present but rsync is missing; Buildroot will rebuild from scratch."
    fi
fi

# Pin the binaries' self-reported version to the image version when the
# caller supplied one (CI passes it from the tag name): a release commit can
# carry both its -beta and -stable tags, so the git-describe fallback inside
# build-server.sh is ambiguous.
if [ -n "${RHYTHM_IMAGE_VERSION:-}" ]; then
    RHYTHM_BUILD_VERSION="${RHYTHM_BUILD_VERSION:-$RHYTHM_IMAGE_VERSION}"
    export RHYTHM_BUILD_VERSION
fi

if [ "$SKIP_SERVER_BUILD" = false ]; then
    run_server_build
fi

SERVER_BINARY="$PROJECT_ROOT/dist/bin/rpiz/rhythm-server"
if [ ! -x "$SERVER_BINARY" ]; then
    echo "Error: Missing $SERVER_BINARY"
    echo "Build it first with: ./tools/os/scripts/build-server.sh --release --target rpiz"
    exit 1
fi
HOST_RECORDER_BINARY="$PROJECT_ROOT/dist/bin/rpiz/rhythm-host-recorder"
if [ ! -x "$HOST_RECORDER_BINARY" ]; then
    echo "Error: Missing $HOST_RECORDER_BINARY"
    echo "Build it first with: ./tools/os/scripts/build-server.sh --release --target rpiz"
    exit 1
fi
RHYTHM_IMAGE_VERSION="${RHYTHM_IMAGE_VERSION:-$("$SCRIPT_DIR/resolve-version.sh" server)}"
export RHYTHM_IMAGE_VERSION

if [ -n "$WIFI_SSID" ]; then
    WIFI_COUNTRY="$(printf '%s' "$WIFI_COUNTRY" | tr '[:lower:]' '[:upper:]')"
    echo "Embedding Wi-Fi config for SSID '$WIFI_SSID' (country $WIFI_COUNTRY)"
    export RHYTHM_WIFI_SSID="$WIFI_SSID"
    export RHYTHM_WIFI_PSK="$WIFI_PSK"
    export RHYTHM_WIFI_COUNTRY="$WIFI_COUNTRY"
fi
if is_truthy "$DEV_MODE"; then
    echo "Building rpiz image in dev mode (Dropbear, known root password, Matter attestation bypass)"
    IMAGE_MODE=dev
    export RHYTHM_DEV_MODE=1
else
    echo "Building rpiz image in production mode (no Dropbear, no known root password, Matter attestation bypass)"
    IMAGE_MODE=prod
    unset RHYTHM_DEV_MODE
fi

EXTERNAL_DIR="$PROJECT_ROOT/install/rpiz/buildroot"

if [ -z "$BUILDROOT_DL_DIR" ]; then
    BUILDROOT_DL_DIR="$OUTPUT_DIR/dl"
fi
mkdir -p "$BUILDROOT_DL_DIR"

buildroot_make() {
    make \
        -C "$BUILDROOT_DIR" \
        BR2_EXTERNAL="$EXTERNAL_DIR" \
        O="$OUTPUT_DIR" \
        BR2_DL_DIR="$BUILDROOT_DL_DIR" \
        "$@"
}

buildroot_make rhythm_rpiz_defconfig
if is_truthy "${RHYTHM_DEV_MODE:-}"; then
    DEV_FRAGMENT="$OUTPUT_DIR/rhythm-dev.fragment"
    cat >"$DEV_FRAGMENT" <<'EOF'
BR2_TARGET_GENERIC_ROOT_PASSWD="rhythm"
BR2_PACKAGE_DROPBEAR=y
EOF
    cat "$DEV_FRAGMENT" >> "$OUTPUT_DIR/.config"
    buildroot_make olddefconfig
fi
# The prebuilt server comes from dist/bin/rpiz via a local-site package. Force
# that package to refresh each run so Buildroot does not reuse a stale unpacked
# copy when the host-side binary changes between image builds.
buildroot_make rhythm-prebuilt-dirclean
buildroot_make
"$SCRIPT_DIR/check-rpiz-image-mode.sh" "$OUTPUT_DIR" "$IMAGE_MODE"

rm -f "$OUTPUT_DIR/images/rootfs.ext2.gz" "$OUTPUT_DIR/images/sdcard.img.gz"
if command -v gzip >/dev/null 2>&1; then
    gzip -c -n "$OUTPUT_DIR/images/rootfs.ext2" > "$OUTPUT_DIR/images/rootfs.ext2.gz"
    gzip -c -n "$OUTPUT_DIR/images/sdcard.img" > "$OUTPUT_DIR/images/sdcard.img.gz"
fi

echo ""
echo "RPi Zero image build complete"
if [ -f "$OUTPUT_DIR/images/sdcard.img.gz" ]; then
    echo "  Image: $OUTPUT_DIR/images/sdcard.img.gz"
else
    echo "  Image: $OUTPUT_DIR/images/sdcard.img"
fi
if [ -f "$OUTPUT_DIR/images/rootfs.ext2.gz" ]; then
    echo "  Rootfs OTA: $OUTPUT_DIR/images/rootfs.ext2.gz"
elif [ -f "$OUTPUT_DIR/images/rootfs.ext2" ]; then
    echo "  Rootfs OTA: $OUTPUT_DIR/images/rootfs.ext2"
fi
