#!/bin/bash
# Verify that rpiz rootfs fingerprints are independent of the caller's locale.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
FINGERPRINT_SCRIPT="$SCRIPT_DIR/../compute-rootfs-fingerprint.sh"

fingerprint() {
    local locale_name="$1"
    local image_mode="$2"
    LC_ALL="$locale_name" bash "$FINGERPRINT_SCRIPT" --image-mode "$image_mode"
}

for image_mode in dev prod; do
    expected="$(fingerprint C "$image_mode")"
    utf8="$(fingerprint en_US.UTF-8 "$image_mode")"

    case "$expected" in
        "v2-$image_mode-"????????????)
            ;;
        *)
            echo "FAIL $image_mode fingerprint has unexpected shape: $expected" >&2
            exit 1
            ;;
    esac

    if [ "$utf8" != "$expected" ]; then
        echo "FAIL $image_mode fingerprint changed with caller locale: $expected != $utf8" >&2
        exit 1
    fi

    echo "ok   $image_mode fingerprint is locale-independent: $expected"
done

echo "compute-rootfs-fingerprint-sim: all checks passed"
