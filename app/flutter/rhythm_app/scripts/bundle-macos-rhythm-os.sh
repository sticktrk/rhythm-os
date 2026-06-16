#!/bin/sh
set -eu

APP_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WORKSPACE_ROOT="$(cd "$APP_ROOT/../../.." && pwd)"
RHYTHM_OS_RUST_DIR="${RHYTHM_OS_RUST_DIR:-$WORKSPACE_ROOT/rhythm-os-rust}"

if [ "${RHYTHM_SKIP_BUNDLED_SERVER_BUILD:-0}" = "1" ]; then
  echo "Skipping bundled Rhythm OS build"
  exit 0
fi

if [ ! -x "$RHYTHM_OS_RUST_DIR/scripts/build-server.sh" ]; then
  echo "error: rhythm-os-rust checkout not found at $RHYTHM_OS_RUST_DIR" >&2
  echo "Set RHYTHM_OS_RUST_DIR to the backend checkout path." >&2
  exit 1
fi

case "${CONFIGURATION:-Debug}" in
  Debug)
    BUILD_FLAG="--debug"
    ;;
  *)
    BUILD_FLAG="--release"
    ;;
esac

target_for_arch() {
  case "$1" in
    arm64) echo "macos-arm64" ;;
    x86_64) echo "macos-x86_64" ;;
    *)
      echo "error: unsupported macOS arch '$1' for bundled Rhythm OS" >&2
      exit 1
      ;;
  esac
}

ARCH_LIST="${ARCHS:-$(uname -m)}"
DIST_TARGETS=""
for arch in $ARCH_LIST; do
  target="$(target_for_arch "$arch")"
  case " $DIST_TARGETS " in
    *" $target "*) ;;
    *) DIST_TARGETS="$DIST_TARGETS $target" ;;
  esac
done

if [ -z "${BUILT_PRODUCTS_DIR:-}" ] || [ -z "${UNLOCALIZED_RESOURCES_FOLDER_PATH:-}" ]; then
  echo "error: Xcode output paths are not available" >&2
  exit 1
fi

DEST_DIR="$BUILT_PRODUCTS_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH/rhythm-os"
TMP_DIR="${TARGET_TEMP_DIR:-$APP_ROOT/build}/rhythm-os-bundle"
mkdir -p "$DEST_DIR" "$TMP_DIR"

for target in $DIST_TARGETS; do
  "$RHYTHM_OS_RUST_DIR/scripts/build-server.sh" "$BUILD_FLAG" --target "$target"
done

copy_or_lipo() {
  name="$1"
  output="$DEST_DIR/$name"
  set -- $DIST_TARGETS
  if [ "$#" -eq 1 ]; then
    cp "$RHYTHM_OS_RUST_DIR/dist/bin/$1/$name" "$output"
  else
    inputs=""
    for target in $DIST_TARGETS; do
      inputs="$inputs $RHYTHM_OS_RUST_DIR/dist/bin/$target/$name"
    done
    lipo -create $inputs -output "$output"
  fi
  chmod 755 "$output"
}

copy_or_lipo rhythm-server
copy_or_lipo rhythm-chipd
copy_or_lipo rhythm-cli

if command -v codesign >/dev/null 2>&1; then
  SIGN_IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:-}"
  if [ -n "$SIGN_IDENTITY" ] && [ "$SIGN_IDENTITY" != "-" ]; then
    for binary in rhythm-server rhythm-chipd rhythm-cli; do
      codesign --force --sign "$SIGN_IDENTITY" "$DEST_DIR/$binary" >/dev/null
    done
  fi
fi

echo "Bundled Rhythm OS binaries into $DEST_DIR"
