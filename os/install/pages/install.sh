#!/usr/bin/env bash
# Rhythm Lighting bootstrap installer.
#
# Usage:
#   curl -fsSL https://get.rhythm.lighting/install.sh | bash
#   curl -fsSL https://get.rhythm.lighting/install.sh | bash -s -- --user --port 8080
#   curl -fsSL https://get.rhythm.lighting/install.sh | bash -s -- --uninstall
#
# Detects platform, downloads the matching release tarball from the Rhythm CDN, verifies
# its SHA256, and hands off to the install/install.sh bundled inside the tarball
# (which sets up systemd or launchd).
#
# Env overrides (mostly for testing):
#   RHYTHM_VERSION          Pin a specific tag, e.g. v0.4.176-beta.
#   RHYTHM_BASE_URL         Pages base (default https://get.rhythm.lighting).
#   RHYTHM_RELEASE_BASE_URL Release-download base.
#   RHYTHM_NO_TELEMETRY=1   Skip the post-install hit-counter ping.
#   RHYTHM_NO_START=1       Install but do not start the service.

set -euo pipefail

DEFAULT_BASE_URL="https://get.rhythm.lighting"
DEFAULT_RELEASE_BASE_URL="https://dl.rhythm.lighting/server/install"

BASE_URL="${RHYTHM_BASE_URL:-$DEFAULT_BASE_URL}"
RELEASE_BASE_URL="${RHYTHM_RELEASE_BASE_URL:-$DEFAULT_RELEASE_BASE_URL}"

err() { printf '%s\n' "$*" >&2; }
die() { err "$*"; exit 1; }

require() {
    command -v "$1" >/dev/null 2>&1 || die "Required tool '$1' not found in PATH."
}

# --- Argument parsing (forwarded to install.sh, plus a few bootstrap-only flags) ---

INSTALL_ARGS=()
UNINSTALL=false
MODE="system"
PINNED_VERSION="${RHYTHM_VERSION:-}"

while [ $# -gt 0 ]; do
    case "$1" in
        --user)
            MODE="user"
            INSTALL_ARGS+=(--user)
            shift
            ;;
        --uninstall)
            UNINSTALL=true
            INSTALL_ARGS+=(--uninstall)
            shift
            ;;
        --no-start)
            export RHYTHM_NO_START=1
            INSTALL_ARGS+=(--no-start)
            shift
            ;;
        --version)
            PINNED_VERSION="$2"
            shift 2
            ;;
        --port|--log-level)
            INSTALL_ARGS+=("$1" "$2")
            shift 2
            ;;
        -h|--help)
            cat <<EOF
Rhythm Lighting installer (bootstrap)

Usage:
  curl -fsSL ${BASE_URL}/install.sh | bash [-s -- OPTIONS]

Options:
  --user              Install as a user-level systemd service (Linux only).
  --port PORT         Override the default port (54448).
  --log-level LEVEL   Override the default log level (info).
  --no-start          Install the service but do not start it.
  --version TAG       Pin a specific release tag (default: latest beta).
  --uninstall         Remove the service and binaries.
  -h, --help          Show this help.

Environment:
  RHYTHM_NO_TELEMETRY=1  Skip the post-install hit-counter ping.
EOF
            exit 0
            ;;
        *)
            die "Unknown option: $1"
            ;;
    esac
done

# --- Platform detection ---

detect_platform() {
    local os arch
    case "$(uname -s)" in
        Darwin) os="macos" ;;
        Linux)  os="linux" ;;
        *)      die "Unsupported OS: $(uname -s). Supported: macOS, Linux." ;;
    esac

    case "$(uname -m)" in
        x86_64|amd64)
            [ "$os" = "macos" ] && arch="x86_64" || arch="amd64"
            ;;
        arm64|aarch64)
            [ "$os" = "macos" ] && arch="arm64" || arch="aarch64"
            ;;
        armv6l|arm1176*)
            die "Detected Raspberry Pi Zero hardware. The curl|bash installer does not support rpiz; flash the SD-card image instead. See https://docs.rhythm.lighting/appliance"
            ;;
        *)
            die "Unsupported architecture: $(uname -m). Supported: x86_64, arm64/aarch64."
            ;;
    esac

    echo "${os}-${arch}"
}

PLATFORM="$(detect_platform)"

# --- Tool prerequisites ---

require uname
require curl
require tar
require mktemp
if command -v sha256sum >/dev/null 2>&1; then
    SHA_TOOL="sha256sum"
elif command -v shasum >/dev/null 2>&1; then
    SHA_TOOL="shasum -a 256"
else
    die "Need sha256sum or shasum on PATH to verify the download."
fi

# --- Concurrency lock (best-effort; we're a one-shot installer) ---

LOCK_FILE="${TMPDIR:-/tmp}/rhythm-install.lock"
if command -v flock >/dev/null 2>&1; then
    exec 9>"$LOCK_FILE"
    if ! flock -n 9; then
        die "Another rhythm-server install appears to be running (lock $LOCK_FILE). Wait for it to finish or remove the lock file."
    fi
fi

# --- Resolve version ---

resolve_version() {
    if [ -n "$PINNED_VERSION" ]; then
        echo "$PINNED_VERSION"
        return
    fi
    local v
    v="$(curl -fsSL --max-time 10 "${RELEASE_BASE_URL}/latest.txt" || true)"
    v="$(printf '%s' "$v" | tr -d '[:space:]')"
    if [ -z "$v" ]; then
        v="$(curl -fsSL --max-time 10 "${BASE_URL}/latest.txt" || true)"
    fi
    v="$(printf '%s' "$v" | tr -d '[:space:]')"
    if [ -z "$v" ]; then
        die "Could not resolve latest version from ${RELEASE_BASE_URL}/latest.txt or ${BASE_URL}/latest.txt. Pin one with --version vX.Y.Z-beta."
    fi
    echo "$v"
}

VERSION="$(resolve_version)"
case "$VERSION" in
    v*) ;;
    *)  VERSION="v${VERSION}" ;;
esac

# --- Download + verify ---

ARCHIVE="rhythm-server-${VERSION#v}-${PLATFORM}.tar.gz"
ARCHIVE_URL="${RELEASE_BASE_URL}/${VERSION}/${ARCHIVE}"
SHA_URL="${ARCHIVE_URL}.sha256"

WORK_DIR="$(mktemp -d -t rhythm-install.XXXXXX)"
cleanup() { rm -rf "$WORK_DIR"; }
trap cleanup EXIT

echo "Rhythm Lighting installer"
echo "  Platform: $PLATFORM"
echo "  Version:  $VERSION"
if [ "$UNINSTALL" = true ]; then
    echo "  Mode:     $MODE (uninstall)"
else
    echo "  Mode:     $MODE"
fi
echo ""

echo "Downloading $ARCHIVE..."
curl -fsSL --retry 3 --retry-delay 2 -o "$WORK_DIR/$ARCHIVE" "$ARCHIVE_URL" \
    || die "Download failed: $ARCHIVE_URL"

echo "Verifying checksum..."
if ! curl -fsSL --retry 3 --retry-delay 2 -o "$WORK_DIR/$ARCHIVE.sha256" "$SHA_URL" 2>/dev/null; then
    err "Warning: checksum file not available at $SHA_URL — trusting TLS only."
else
    expected="$(awk '{print $1}' "$WORK_DIR/$ARCHIVE.sha256")"
    actual="$(cd "$WORK_DIR" && $SHA_TOOL "$ARCHIVE" | awk '{print $1}')"
    if [ -z "$expected" ] || [ -z "$actual" ]; then
        die "Checksum verification failed (empty digest)."
    fi
    if [ "$expected" != "$actual" ]; then
        err "Checksum mismatch."
        err "  expected: $expected"
        err "  actual:   $actual"
        exit 1
    fi
fi

echo "Extracting..."
tar -xzf "$WORK_DIR/$ARCHIVE" -C "$WORK_DIR"

# Tarball top-level directory (e.g. rhythm-server-0.4.176-beta-macos-arm64/).
ROOT="$(find "$WORK_DIR" -mindepth 1 -maxdepth 1 -type d -print -quit)"
[ -d "$ROOT" ] || die "Tarball had no top-level directory."

INSTALL_SCRIPT="$ROOT/install/install.sh"
SERVER_BIN="$ROOT/bin/rhythm-server"
[ -x "$INSTALL_SCRIPT" ] || die "Tarball is missing install/install.sh"
[ -f "$SERVER_BIN" ] || die "Tarball is missing bin/rhythm-server"

echo ""
INSTALL_ARGS+=(--binary "$SERVER_BIN")
"$INSTALL_SCRIPT" "${INSTALL_ARGS[@]}" </dev/null

# --- Telemetry (opt-out, fire-and-forget, never fails the install) ---

if [ -z "${RHYTHM_NO_TELEMETRY:-}" ]; then
    EVENT="install"
    [ "$UNINSTALL" = true ] && EVENT="uninstall"
    PING_URL="${BASE_URL}/installed.txt?p=${PLATFORM}&v=${VERSION}&os=$(uname -s)&m=${MODE}&e=${EVENT}"
    curl --max-time 2 -fsS "$PING_URL" -o /dev/null 2>/dev/null || true
fi
