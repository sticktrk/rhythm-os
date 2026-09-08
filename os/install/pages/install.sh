#!/usr/bin/env bash
# The hosted macOS/Linux binary installer was retired with the legacy CDN
# artifact tree. Keep this endpoint explicit so cached documentation fails with
# a useful migration message instead of downloading stale binaries.

set -euo pipefail

cat >&2 <<'EOF'
The hosted Rhythm macOS/Linux binary installer has been retired.

Build rhythm-server from source:
  git clone https://github.com/sticktrk/rhythm-os.git
  cd cross/os
  cargo build -p rhythm-server --release

For a Rhythm Pi Zero appliance, flash the current production image:
  https://dl.rhythm.lighting/server/sdcard.img.gz
EOF
exit 1
