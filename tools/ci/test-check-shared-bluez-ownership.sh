#!/usr/bin/env bash
set -euo pipefail
export LC_ALL=C

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
checker="${repo_root}/tools/ci/check-shared-bluez-ownership.sh"

# Git hooks export repository-local variables such as GIT_DIR. Those variables
# override `git -C`, so leaving them set here can make the synthetic `git add`
# replace the caller's real worktree index during a pre-push invariant run.
# Capture repository paths above, then isolate every Git command below.
while IFS= read -r git_env_var; do
  [[ -z "$git_env_var" ]] || unset "$git_env_var"
done < <(git rev-parse --local-env-vars)

tmp_root="$(mktemp -d "${TMPDIR:-/tmp}/rhythm-bluez-guard-test.XXXXXX")"
trap 'rm -rf "$tmp_root"' EXIT

mkdir -p \
  "$tmp_root/tools/ci/references" \
  "$tmp_root/tools/ci" \
  "$tmp_root/os/rust/integrations/rhythm-ble/src" \
  "$tmp_root/os/rust/bins/rhythm-linux-appliance/src"
cp "$checker" "$tmp_root/tools/ci/"

cat >"$tmp_root/os/rust/integrations/rhythm-ble/Cargo.toml" <<'EOF'
[package]
name = "rhythm-ble"
version = "0.0.0"

[dependencies]
bluer = "0.17"
EOF

cat >"$tmp_root/os/rust/integrations/rhythm-ble/src/bluez.rs" <<'EOF'
use bluer::Session;
use tokio::runtime::Runtime;

async fn shared_owner() {
    let _session = Session::new().await;
    let _runtime = Runtime::new();
}
EOF

cat >"$tmp_root/os/rust/bins/rhythm-linux-appliance/src/ble_provision.rs" <<'EOF'
use bluer::Session;
use tokio::runtime::Runtime;

async fn peripheral_only_exception() {
    let _session = Session::new().await;
    let _runtime = Runtime::new();
}
EOF

cat >"$tmp_root/os/rust/bins/rhythm-linux-appliance/Cargo.toml" <<'EOF'
[package]
name = "rhythm-linux-appliance"
version = "0.0.0"

[dependencies]
bluer = "0.17"
EOF

cat >"$tmp_root/tools/ci/references/bluez-owner-exceptions.tsv" <<'EOF'
# repo-relative path	categories	owner	expires	reason
os/rust/bins/rhythm-linux-appliance/Cargo.toml	dependency	rhythm-linux-appliance	2099-12-31	Synthetic peripheral dependency exception used by the guard contract test.
os/rust/bins/rhythm-linux-appliance/src/ble_provision.rs	session,runtime	rhythm-linux-appliance	2099-12-31	Synthetic peripheral-only exception used by the guard contract test.
EOF

git -C "$tmp_root" init -q
git -C "$tmp_root" add .

(
  cd "$tmp_root"
  tools/ci/check-shared-bluez-ownership.sh
) >"$tmp_root/pass.log"
grep -q 'shared BlueZ ownership invariant passed' "$tmp_root/pass.log"

mkdir -p "$tmp_root/os/rust/integrations/rhythm-vendor/src"
cat >"$tmp_root/os/rust/integrations/rhythm-vendor/Cargo.toml" <<'EOF'
[package]
name = "rhythm-vendor"
version = "0.0.0"

[dependencies]
bt = { package = "bluer", version = "0.17" }
EOF
cat >"$tmp_root/os/rust/integrations/rhythm-vendor/src/bluez.rs" <<'EOF'
use bluer as bt;
use bluer::{Adapter, Session as BluezSession};
use tokio::runtime::Runtime;

async fn competing_owner(adapter: Adapter) {
    let _session = BluezSession::new().await;
    let _other_session = bt::Session::new().await;
    let _runtime = Runtime::new();
    let _ = adapter.set_discovery_filter(Default::default()).await;
    // Exercise the lower-level discovery API too; a guard that only catches
    // `discover_devices_with_changes` would still allow a competing scanner.
    let _ = adapter.discover_devices().await;
}
EOF

if (
  cd "$tmp_root"
  tools/ci/check-shared-bluez-ownership.sh
) >"$tmp_root/fail.log" 2>&1; then
  echo "guard accepted a competing integration owner" >&2
  exit 1
fi
grep -q 'rhythm-vendor/Cargo.toml: dependency ownership' "$tmp_root/fail.log"
grep -q 'rhythm-vendor/src/bluez.rs: session ownership' "$tmp_root/fail.log"
grep -q 'rhythm-vendor/src/bluez.rs: discovery ownership' "$tmp_root/fail.log"
grep -q 'rhythm-vendor/src/bluez.rs: runtime ownership' "$tmp_root/fail.log"

rm -rf "$tmp_root/os/rust/integrations/rhythm-vendor"

# A driver does not need a direct bluer dependency to misuse the raw Adapter
# inferred inside BluezClient's callback. This is the regression that source
# scans gated only on `use bluer` or `bluer::` fail to detect.
mkdir -p "$tmp_root/os/rust/integrations/rhythm-inferred/src"
cat >"$tmp_root/os/rust/integrations/rhythm-inferred/Cargo.toml" <<'EOF'
[package]
name = "rhythm-inferred"
version = "0.0.0"

[dependencies]
rhythm-ble = { path = "../rhythm-ble" }
EOF
cat >"$tmp_root/os/rust/integrations/rhythm-inferred/src/lib.rs" <<'EOF'
use rhythm_ble::bluez::BluezClient;

fn competing_scanner(client: &BluezClient) -> anyhow::Result<()> {
    client.run_adapter_operation(|_session, adapter| async move {
        let _events = adapter.discover_devices().await?;
        Ok(())
    })
}
EOF

if (
  cd "$tmp_root"
  tools/ci/check-shared-bluez-ownership.sh
) >"$tmp_root/inferred.log" 2>&1; then
  echo "guard accepted discovery through an inferred raw Adapter" >&2
  exit 1
fi
grep -q 'rhythm-inferred/src/lib.rs: discovery ownership' "$tmp_root/inferred.log"

rm -rf "$tmp_root/os/rust/integrations/rhythm-inferred"
cat >>"$tmp_root/tools/ci/references/bluez-owner-exceptions.tsv" <<'EOF'
os/rust/integrations/rhythm-gone/src/bluez.rs	session	rhythm-gone	2099-12-31	Synthetic stale exception used to prove stale records are rejected.
EOF

if (
  cd "$tmp_root"
  tools/ci/check-shared-bluez-ownership.sh
) >"$tmp_root/stale.log" 2>&1; then
  echo "guard accepted a stale exception" >&2
  exit 1
fi
grep -q 'stale BlueZ exception path does not exist' "$tmp_root/stale.log"

echo "shared BlueZ ownership guard contract tests passed"
