#!/bin/bash
# Local simulation for rpiz image-mode validation. No Buildroot build required.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECKER="$SCRIPT_DIR/../check-rpiz-image-mode.sh"

WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-image-mode.XXXXXX")"
trap 'rm -rf "$WORK_DIR"' EXIT

FAILURES=0

fail_case() {
    echo "FAIL $*"
    FAILURES=$((FAILURES + 1))
}

make_base_output() {
    local out="$1"

    mkdir -p "$out/target/etc/default" "$out/target/usr/bin" "$out/target/etc/init.d"
    cat > "$out/.config" <<'EOF'
BR2_PACKAGE_RHYTHM_CLOUDFLARED=y
EOF
    printf '#!/bin/sh\n' > "$out/target/usr/bin/cloudflared"
    printf '#!/bin/sh\n' > "$out/target/usr/bin/rhythm-host-recorder"
    printf '#!/bin/sh\n' > "$out/target/etc/init.d/rhythm-cloudflared"
    printf '#!/bin/sh\n' > "$out/target/etc/init.d/S42hostrecorder"
    chmod +x "$out/target/usr/bin/cloudflared" "$out/target/usr/bin/rhythm-host-recorder" \
        "$out/target/etc/init.d/rhythm-cloudflared" "$out/target/etc/init.d/S42hostrecorder"
    cat > "$out/target/etc/inittab" <<'EOF'
::sysinit:/etc/init.d/rcS
::respawn:/usr/bin/rhythm-host-recorder run --data-dir /data
::respawn:/usr/bin/rhythm-hardware-watchdog
::respawn:/usr/bin/rhythm-launch
EOF
    printf 'test-image\n' > "$out/target/etc/rhythm-image-version"
    printf 'v1-prod-test\n' > "$out/target/etc/rhythm-image-fingerprint"
}

valid_prod="$WORK_DIR/valid-prod"
make_base_output "$valid_prod"
cat > "$valid_prod/target/etc/default/rhythm" <<'EOF'
RHYTHM_DEV_MODE=0
RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1
RHYTHM_MATTER_ALLOW_TEST_PAA=0
EOF

if bash "$CHECKER" "$valid_prod" prod >/dev/null; then
    echo "ok   prod accepts stable Matter attestation bypass"
else
    fail_case "prod rejected stable Matter attestation bypass"
fi

paa_prod="$WORK_DIR/paa-prod"
cp -R "$valid_prod" "$paa_prod"
cat > "$paa_prod/target/etc/default/rhythm" <<'EOF'
RHYTHM_DEV_MODE=0
RHYTHM_MATTER_PAA_TRUST_STORE_PATH=/data/matter/paa-root-certs
RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=0
RHYTHM_MATTER_ALLOW_TEST_PAA=0
EOF

if bash "$CHECKER" "$paa_prod" prod >/dev/null 2>&1; then
    fail_case "prod accepted Matter PAA trust-store requirement"
else
    echo "ok   prod rejects Matter PAA trust-store requirement"
fi

echo ""
if [ "$FAILURES" -gt 0 ]; then
    echo "check-rpiz-image-mode-sim: $FAILURES failure(s)"
    exit 1
fi
echo "check-rpiz-image-mode-sim: all checks passed"
