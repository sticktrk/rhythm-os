# Build & Deploy Scripts

This directory contains all build and deployment scripts for Rhythm OS.

## Quick Reference

```bash
# Development
./tools/os/scripts/run-dev.sh                    # Build and run addon locally

# Triage
./tools/triage-bug.sh [issue-number]             # Download and summarize app bug-report debug bundle
./tools/triage-features.sh [issue-number]        # Scaffold a product brief for feature delivery

# Build individual components
./tools/os/scripts/build-server.sh               # Build the server
./tools/os/scripts/build-rust.sh                 # Build Rust addon
./tools/os/scripts/build-rpiz-image.sh           # Build Pi Zero SD image

# Refresh the rpiz CI/local builder image (content-addressed; no-op if unchanged)
./tools/os/scripts/build/refresh-builder-image.sh --push

# Deploy (full OTA guide: docs/ota.md)
./tools/os/scripts/deploy-addon.sh               # Push addon to Docker Hub
./tools/os/scripts/deploy-addon.sh --local       # Deploy to local HA for testing
./tools/os/scripts/release.sh                    # Tag + push a beta release (CI publishes; image auto-detected)
./tools/os/scripts/release.sh --promote-stable   # Promote latest beta tag to the stable feed
./tools/os/scripts/verify-beta-release.sh        # Prove beta feed and optional bench device; write receipt
./tools/os/scripts/release.sh --upload           # Escape hatch: build + upload the rpiz OTA feed locally
```

Anything under `tools/os/scripts/build/` deals with producing the prebuilt Docker
*builder* image (`dtconcepts/rhythm-rpiz-builder`) that the rpiz flow and its
CI workflow share. See [`tools/os/scripts/build/README.md`](build/README.md).

## Helper scripts

Mostly invoked by the main flows above, but usable standalone:

| Script | Purpose |
|--------|---------|
| `check-rpiz-image-mode.sh` | Validate that rpiz Buildroot output matches the requested image posture (dev vs prod) and the expected fingerprint |
| `compute-rootfs-fingerprint.sh` | Content-hash the rootfs inputs; drives the CI binary-vs-image release gate |
| `package-server-updates.sh` | Package rhythm-server binaries (+ optional images) into a static OTA feed manifest |
| `promote-stable.sh` | Promote a tested beta release to stable by creating the matching `-stable` tag |
| `prune-server-releases.sh` | Prune old versioned release directories from the dl.rhythm.lighting server repo (manifest-referenced dirs are kept) |
| `push-rpiz-dev.sh` | Fast dev loop: cross-compile the appliance binary, scp it to a device, respawn init |
| `verify-beta-release.sh` | Verify remote beta tag/public feed and optionally exact package + journey endpoints on a live rpiz |
| `resolve-version.sh` | Resolve the current version for a shipped Rhythm artifact from Git tags |
| `lib/` | Shared helpers sourced by the scripts above (`version.sh` semver/channel/feed, `artifact.sh` sha256/size/json) |
| `tests/package-feed-sim.sh` | Local OTA feed lifecycle simulation (image release → binary carry-forward → dry-run); runs in CI |
| `tests/rpiz-hardware-watchdog-sim.sh` | Verify the rpiz watchdog starts from init, emits keepalives, and disarms cleanly; runs in CI |

Triage scripts are intentionally not duplicated under `tools/os/scripts`. Use
the canonical CROSS root entrypoints: `./tools/triage-bug.sh` and
`./tools/triage-features.sh`.

---

## Development

### run-dev.sh

Run the addon locally for development and testing.

```bash
./tools/os/scripts/run-dev.sh                    # Full build + run
./tools/os/scripts/run-dev.sh --skip-build       # Skip builds, just run
./tools/os/scripts/run-dev.sh --debug            # Debug build instead of release
```

**Prerequisites:**
- Copy `.env.example` to `.env` in project root
- Configure `HA_HOST`, `HA_TOKEN`, etc.

**Environment variables:**
| Variable | Description | Default |
|----------|-------------|---------|
| `HA_HOST` | Home Assistant hostname | `localhost` |
| `HA_PORT` | Home Assistant port | `8123` |
| `HA_TOKEN` | Long-lived access token | (required) |
| `HA_USE_SSL` | Use HTTPS | `false` |
| `INGRESS_PORT` | Local web UI port | `8099` |

---

## Build Scripts

### build-rust.sh

Build the Rust addon binary.

```bash
./tools/os/scripts/build-rust.sh                         # Debug build, native arch
./tools/os/scripts/build-rust.sh --release               # Release build, native arch
./tools/os/scripts/build-rust.sh --release --target amd64      # Specific arch
./tools/os/scripts/build-rust.sh --release --target all        # All architectures
```

**Options:**
| Flag | Description |
|------|-------------|
| `--release` | Build in release mode (optimized) |
| `--target <arch>` | Target: `native`, `amd64`, `aarch64`, `armv7`, `all` |

**Output:** `dist/bin/{arch}/rhythm-addon`

### build-server.sh

Build the native Rhythm OS binaries and package them into `dist/bin/...`.

```bash
./tools/os/scripts/build-server.sh                              # Debug build, native target
./tools/os/scripts/build-server.sh --release                    # Release build, native target
./tools/os/scripts/build-server.sh --release --target rpiz      # Raspberry Pi Zero / Zero W
./tools/os/scripts/build-server.sh --release --target all-linux # All Linux targets
RHYTHM_CHIP_OUT_DIR=/path/to/connectedhomeip/out/rpiz ./tools/os/scripts/build-server.sh --release --target rpiz
```

**Options:**
| Flag | Description |
|------|-------------|
| `--release` | Build in release mode (optimized) |
| `--debug` | Build in debug mode |
| `--target <target>` | Target: `native`, `macos-arm64`, `macos-x86_64`, `linux-amd64`, `linux-aarch64`, `rpiz`, `all-macos`, `all-linux`, `all` |
| `--clean` | Clean before building |
| `--run` | Run the native server after building |

**Output:** `dist/bin/{target}/{rhythm-server,rhythm-cli}`
For `rpiz`, the build comes from the `rhythm-linux-appliance` crate and writes `dist/bin/rpiz/rhythm-server`.
If `RHYTHM_CHIP_OUT_DIR` or `RHYTHM_CHIP_LIB_DIR` is set, the helper automatically builds `rhythm-chipd` with `--features chip-ffi`. For native builds, `RHYTHM_CHIP_ROOT` also enables the direct bridge.
The native Matter bridge also accepts `RHYTHM_MATTER_CONTROLLER_VENDOR_ID` to override the controller vendor ID used by `rhythm-chipd`. It defaults to `0xFFF1` for development and accepts either hex (`0xFFF1`) or decimal.
`RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1` makes `rhythm-chipd` skip Matter DAC/PAA verification. Until production PAA provisioning is wired, rpiz dev and prod images both set it at boot.

### build-rpiz-image.sh

Build a Raspberry Pi Zero SD-card image using the Buildroot external tree in `install/rpiz/buildroot`.

```bash
./tools/os/scripts/build-rpiz-image.sh --release
./tools/os/scripts/build-rpiz-image.sh --release --prod
./tools/os/scripts/build-rpiz-image.sh --skip-server-build
./tools/os/scripts/build-rpiz-image.sh --release --docker
./tools/os/scripts/build-rpiz-image.sh --release --wifi-ssid "MyNet" --wifi-psk "secretpass"
```

**Options:**
| Flag | Description |
|------|-------------|
| `--buildroot-dir <path>` | Path to a Buildroot checkout. Default is `./buildroot`, and that path is auto-cloned if missing |
| `--output-dir <path>` | Buildroot output directory (default: `out/rpiz`) |
| `--release` | Build the server binary in release mode before packaging |
| `--debug` | Build the server binary in debug mode before packaging |
| `--dev` | Build the rpiz bring-up image. This is the default |
| `--prod`, `--production` | Disable the rpiz bring-up extras for a production image |
| `--skip-server-build` | Reuse an existing `dist/bin/rpiz/rhythm-server` |
| `--wifi-ssid <ssid>` | Embed Wi-Fi SSID for Pi Zero W / Zero 2 W |
| `--wifi-psk <psk>` | Embed WPA/WPA2 passphrase |
| `--wifi-country <code>` | Two-letter country code, default `US` |
| `--docker` | Run the Buildroot image step inside Docker |
| `--docker-image <name>` | Docker image tag to build/use |

**Output:** `out/rpiz/images/sdcard.img`
With `--docker` and no explicit `--output-dir`, the default becomes `out/rpiz-docker/images/sdcard.img`.
By default, rpiz image builds include the bring-up extras: Dropbear SSH, root password `rhythm`, and Matter device attestation bypass. Use `--prod` or `RHYTHM_DEV_MODE=0` to turn off Dropbear and the known root password. Production images still bake Matter device attestation bypass until the fleet has production PAA trust-store provisioning.
Production images bake `/etc/default/rhythm` with `RHYTHM_DEV_MODE=0`,
`RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1`, and test PAA roots disabled.

`--docker` pulls `dtconcepts/rhythm-rpiz-builder:latest` from Docker Hub and
runs the full cross-compile + image packaging inside it. Override the image
ref with `--docker-image <ref>`; skip the pull with `--no-pull`. To refresh
the image when its baked inputs change, see
[`tools/os/scripts/build/`](build/README.md).

### build/refresh-builder-image.sh

Hashes the slow-path inputs (CHIP prebuilts, ARMv6 musl toolchain, Buildroot
checkout + external tree, Dockerfile, packaging script) into a content tag
like `v1-aa91843f7d73`, checks Docker Hub for it, and only builds+pushes when
that tag is missing. Updates `install/rpiz/builder-image.lock`, which is what
both your local `--docker` runs and `.github/workflows/rpiz-sd-image.yml` pull.

```bash
./tools/os/scripts/build/refresh-builder-image.sh            # dry-check: report if push needed
./tools/os/scripts/build/refresh-builder-image.sh --push     # build + push iff the tag is missing
./tools/os/scripts/build/refresh-builder-image.sh --force    # force a rebuild even when tag exists
```

`./tools/os/scripts/release.sh` runs this with `--push` before tagging so normal release
cycles are a no-op unless CHIP/Buildroot/defconfig actually changed.

### build/build-rpiz-builder-image.sh

Low-level packaging script invoked by `refresh-builder-image.sh`. Stages the
current host's chip source (with pruning), `out/rpiz-arm-musl/` prebuilts,
`~/x-tools/arm-unknown-linux-musleabihf` toolchain, `buildroot/` checkout, and
full `out/rpiz/` Buildroot output into a clean Docker build context, then runs
`docker build` (+ optional `docker push`). Must run on Linux. Useful to drive
by hand when experimenting with the Dockerfile:

```bash
./tools/os/scripts/build/build-rpiz-builder-image.sh --image-tag dtconcepts/rhythm-rpiz-builder:dev
```

See [`tools/os/scripts/build/README.md`](build/README.md) for the full flow, the hashed
inputs, and the `third_party/` pruning knobs used to keep image size down.

---

## Deployment Scripts

### release.sh

Create a Git release tag; the tag push drives the CI release pipeline
(`ci.yml`), which publishes the GitHub release assets and the OTA feed.

**The full OTA model — channels, binary vs image, fingerprint gate, manifest
schema, runbooks — lives in [`docs/ota.md`](../../../docs/ota.md).** Short
version:

- Every release is tagged `vX.Y.Z-beta` and publishes the `rpiz/` (beta)
  feed. `--promote-stable` re-tags the same commit `vX.Y.Z-stable`, which
  publishes the `rpiz-stable/` feed that fleet auto-update consumes.
- CI decides binary-only vs full-image automatically by comparing the rootfs
  fingerprint against the published feed. `--with-image` forces the image
  build (it embeds a `[with-image]` marker in the tag message).
- `--upload` is the escape hatch when GitHub Actions is down: builds,
  packages, and uploads the feed locally using `.env` `RHYTHM_UPDATES_*`
  credentials (`RHYTHM_UPDATES_SSH_KEY_FILE` or `RHYTHM_UPDATES_SSH_KEY`).

```bash
./tools/os/scripts/release.sh                    # Next patch beta release
./tools/os/scripts/release.sh --minor            # Next minor beta release
./tools/os/scripts/release.sh --version 0.4.1    # Explicit version
./tools/os/scripts/release.sh --promote-stable   # vX.Y.Z-beta -> vX.Y.Z-stable
./tools/os/scripts/release.sh --with-image       # Force a full rootfs image build
./tools/os/scripts/release.sh --upload           # Local build + feed upload (no CI)
./tools/os/scripts/release.sh --dry-run          # Preview without changing git state
```

Run `release.sh --help` for the full flag list.

**Behavior:**
- Requires a clean tracked worktree before tagging.
- Updates the root workspace version in `Cargo.toml` before tagging and mechanically syncs only local `rhythm-*` package versions in the root `Cargo.lock`. It does not run Cargo dependency resolution, which avoids unrelated `rhythm-chipd` lockfile churn on macOS release hosts.
- Creates the release commit automatically when those version files change.
- Pushes the current branch and the new tag to `origin` by default.
- Stable promotion verifies the matching remote beta tag and public beta
  manifest before creating the stable tag. Add `--device URL --token-file FILE`
  for a bench receipt. Emergency skips require
  `--skip-beta-verification --reason TEXT`.

### Versioning

Rhythm OS now uses two versioning tracks:

- Workspace Rust binaries derive their build version from Git tags.
- `rhythm-addon` remains separate and uses `install/addon/config.yaml`.

For workspace/server/appliance builds:

- Tagged release builds resolve to the exact tag version only when the tracked worktree is clean, for example `v0.4.0` -> `0.4.0`.
- Beta release tags are named `vX.Y.Z-beta`. Stable releases are promoted from beta with `./tools/os/scripts/release.sh --promote-stable [X.Y.Z]`, which creates `vX.Y.Z-stable` and lets CI rebuild the artifact with `-stable` embedded in the binary version. CI builds a full rootfs image automatically when the rootfs fingerprint changed.
- Dirty tagged builds append `.dirty`, for example `v0.4.0` with local edits -> `0.4.0.dirty`.
- Untagged builds resolve to a Git-derived prerelease, for example `0.4.0-beta.dev.66.g1b40e459`.
- Dirty untagged builds append `.dirty`, for example `0.4.0-beta.dev.66.g1b40e459.dirty`.
- After a release tag exists, untagged builds move to the next patch line automatically. After `v0.4.0`, the next dev builds become `0.4.1-beta.dev.N.g<sha>`.

For the addon:

- `deploy-addon.sh` still owns addon versioning.
- If you do not pass `--version`, it auto-bumps `install/addon/config.yaml`.
- If you want an explicit addon version for a release, pass `--version <semver>`.

### deploy-addon.sh

Deploy the addon to Docker Hub or directly to a Home Assistant instance.

#### Docker Hub (Production)

```bash
./tools/os/scripts/deploy-addon.sh                   # Build all + push to Docker Hub
./tools/os/scripts/deploy-addon.sh --version 0.4.0   # Explicit addon version
./tools/os/scripts/deploy-addon.sh --skip-build      # Use existing builds
./tools/os/scripts/deploy-addon.sh --dry-run         # Show what would happen
```

**Prerequisites:**
- Docker with buildx support
- Logged into Docker Hub: `docker login`

**What it does:**
1. Builds Rust binaries for all architectures
2. Resolves the addon version from `config.yaml` unless `--version` is provided
3. Auto-bumps `config.yaml` when `--version` is omitted
4. Adds `image: dtconcepts/rhythm-os-addon` to `config.yaml`
5. Builds multi-arch Docker image
6. Pushes to Docker Hub with version + `latest` tags

#### Local Development (--local)

```bash
./tools/os/scripts/deploy-addon.sh --local                       # Deploy to HA
./tools/os/scripts/deploy-addon.sh --local --ha-host 192.168.1.50
./tools/os/scripts/deploy-addon.sh --local --ha-arch amd64       # x86 HA
./tools/os/scripts/deploy-addon.sh --local --skip-build          # Just copy
./tools/os/scripts/deploy-addon.sh --local --dry-run             # Preview
```

**Options:**
| Flag | Description | Default |
|------|-------------|---------|
| `--local` | Enable local deployment mode | - |
| `--ha-host <host>` | HA hostname/IP | `homeassistant.local` or `$HA_HOST` |
| `--ha-user <user>` | SSH user | `root` or `$HA_USER` |
| `--ha-arch <arch>` | Target architecture | `aarch64` or `$HA_ARCH` |

**Prerequisites:**
- SSH access to HA instance
- SSH key authentication: `ssh-copy-id root@homeassistant.local`

**What it does:**
1. Builds Rust binary for target arch only
2. Removes `image:` from config.yaml (HA will build locally)
3. Copies addon to HA via SCP
4. HA rebuilds on next install/update

---

## Typical Workflows

### Initial Setup

```bash
# 1. Configure environment
cp .env.example .env
# Edit .env with your HA_HOST, HA_TOKEN, etc.

# 2. Install Rust targets
rustup target add x86_64-unknown-linux-musl
rustup target add aarch64-unknown-linux-musl

# 3. Install tools
cargo install cross

# 4. Test locally
./tools/os/scripts/run-dev.sh
```

### Development Cycle

```bash
# Make changes, then:
./tools/os/scripts/deploy-addon.sh --local

# In HA: Settings → Add-ons → Rhythm OS → Rebuild
```

### Release

```bash
# 1. Update addon changelog if needed

# 2. Deploy addon
# Auto-bumps addon version from install/addon/config.yaml
./tools/os/scripts/deploy-addon.sh

# Or pin the addon explicitly for a coordinated release
./tools/os/scripts/deploy-addon.sh --version 0.4.0

# 3. Commit release changes
git add -A
git commit -m "Release v0.4.0"

# 4. Tag and push the server/appliance release
./tools/os/scripts/release.sh
```

`./tools/os/scripts/release.sh` defaults to the newer of the workspace version
or the next patch tag after the latest `vX.Y.Z`.
Once the GitHub `Release` workflow finishes, the GitHub release page contains
the platform binary tarballs.

---

## Environment Variables

These can be set in `.env` or exported in your shell:

| Variable | Used By | Description |
|----------|---------|-------------|
| `HA_HOST` | run-dev.sh, deploy-addon.sh | Home Assistant hostname |
| `HA_PORT` | run-dev.sh | Home Assistant port |
| `HA_TOKEN` | run-dev.sh | Long-lived access token |
| `HA_USER` | deploy-addon.sh | SSH user for local deploy |
| `HA_ARCH` | deploy-addon.sh | Target architecture |
| `INGRESS_PORT` | run-dev.sh | Local web UI port |
| `RHYTHM_UPDATES_SSH_HOST` | release.sh `--upload` | OTA upload SSH host |
| `RHYTHM_UPDATES_SSH_USER` | release.sh `--upload` | OTA upload SSH user |
| `RHYTHM_UPDATES_BASE_DIR` | release.sh `--upload` | OTA upload destination directory |
| `RHYTHM_UPDATES_SSH_KEY_FILE` | release.sh `--upload` | Path to the SSH private key to use for OTA upload |
| `RHYTHM_UPDATES_SSH_KEY` | release.sh `--upload` | Inline SSH private key, used when no key file path is set |

---

## Troubleshooting

### "declare: -A: invalid option"
Your bash is too old (macOS ships with bash 3.2). The scripts have been updated to work with bash 3.2, but if you see this error, pull the latest version.

### SSH connection failed
```bash
# Test connection
ssh root@homeassistant.local

# Set up key auth
ssh-copy-id root@homeassistant.local

# Check HA SSH addon is installed and running
```

### Docker buildx not available
```bash
# Install Docker Desktop (includes buildx)
# Or enable manually:
docker buildx create --use
```
