# Build & Deploy Scripts

This directory contains all build and deployment scripts for Rhythm OS.

## Quick Reference

```bash
# Development
./scripts/run-dev.sh                    # Build and run addon locally

# Triage
./scripts/triage.sh [issue-number]      # Download and summarize app bug-report debug bundle

# Build individual components
./scripts/build-server.sh               # Build the server
./scripts/build-rust.sh                 # Build Rust addon
./scripts/build-rpiz-image.sh           # Build Pi Zero SD image

# Refresh the rpiz CI/local builder image (content-addressed; no-op if unchanged)
./scripts/build/refresh-builder-image.sh --push

# Deploy
./scripts/deploy-addon.sh               # Push addon to Docker Hub
./scripts/deploy-addon.sh --local       # Deploy to local HA for testing
./scripts/release.sh                    # Tag and push the next GitHub release
./scripts/release.sh --upload           # Build + upload the rpiz OTA feed locally
```

Anything under `scripts/build/` deals with producing the prebuilt Docker
*builder* image (`dtconcepts/rhythm-rpiz-builder`) that the rpiz flow and its
CI workflow share. See [`scripts/build/README.md`](build/README.md).

---

## Development

### run-dev.sh

Run the addon locally for development and testing.

```bash
./scripts/run-dev.sh                    # Full build + run
./scripts/run-dev.sh --skip-build       # Skip builds, just run
./scripts/run-dev.sh --debug            # Debug build instead of release
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
./scripts/build-rust.sh                         # Debug build, native arch
./scripts/build-rust.sh --release               # Release build, native arch
./scripts/build-rust.sh --release --target amd64      # Specific arch
./scripts/build-rust.sh --release --target all        # All architectures
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
./scripts/build-server.sh                              # Debug build, native target
./scripts/build-server.sh --release                    # Release build, native target
./scripts/build-server.sh --release --target rpiz      # Raspberry Pi Zero / Zero W
./scripts/build-server.sh --release --target all-linux # All Linux targets
RHYTHM_CHIP_OUT_DIR=/path/to/connectedhomeip/out/rpiz ./scripts/build-server.sh --release --target rpiz
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
For bring-up only, `RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1` makes `rhythm-chipd` skip Matter DAC/PAA verification. Leave it unset for normal builds and any image you intend to ship.

### build-rpiz-image.sh

Build a Raspberry Pi Zero SD-card image using the Buildroot external tree in `install/rpiz/buildroot`.

```bash
./scripts/build-rpiz-image.sh --release
./scripts/build-rpiz-image.sh --release --prod
./scripts/build-rpiz-image.sh --skip-server-build
./scripts/build-rpiz-image.sh --release --docker
./scripts/build-rpiz-image.sh --release --wifi-ssid "MyNet" --wifi-psk "secretpass"
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
By default, rpiz image builds include the bring-up extras: Dropbear SSH, root password `rhythm`, and Matter device attestation bypass. Use `--prod` or `RHYTHM_DEV_MODE=0` to turn those off.
Production images also bake `/etc/default/rhythm` with Matter attestation
enforced, test PAA roots disabled, and
`RHYTHM_MATTER_PAA_TRUST_STORE_PATH=/data/matter/paa-root-certs`. Override that
path at build time with `RHYTHM_PROD_PAA_TRUST_STORE_PATH` if your provisioning
flow uses a different trust-store location.

`--docker` pulls `dtconcepts/rhythm-rpiz-builder:latest` from Docker Hub and
runs the full cross-compile + image packaging inside it. Override the image
ref with `--docker-image <ref>`; skip the pull with `--no-pull`. To refresh
the image when its baked inputs change, see
[`scripts/build/`](build/README.md).

### build/refresh-builder-image.sh

Hashes the slow-path inputs (CHIP prebuilts, ARMv6 musl toolchain, Buildroot
checkout + external tree, Dockerfile, packaging script) into a content tag
like `v1-aa91843f7d73`, checks Docker Hub for it, and only builds+pushes when
that tag is missing. Updates `install/rpiz/builder-image.lock`, which is what
both your local `--docker` runs and `.github/workflows/rpiz-sd-image.yml` pull.

```bash
./scripts/build/refresh-builder-image.sh            # dry-check: report if push needed
./scripts/build/refresh-builder-image.sh --push     # build + push iff the tag is missing
./scripts/build/refresh-builder-image.sh --force    # force a rebuild even when tag exists
```

`./scripts/release.sh` runs this with `--push` before tagging so normal release
cycles are a no-op unless CHIP/Buildroot/defconfig actually changed.

### build/build-rpiz-builder-image.sh

Low-level packaging script invoked by `refresh-builder-image.sh`. Stages the
current host's chip source (with pruning), `out/rpiz-arm-musl/` prebuilts,
`~/x-tools/arm-unknown-linux-musleabihf` toolchain, `buildroot/` checkout, and
full `out/rpiz/` Buildroot output into a clean Docker build context, then runs
`docker build` (+ optional `docker push`). Must run on Linux. Useful to drive
by hand when experimenting with the Dockerfile:

```bash
./scripts/build/build-rpiz-builder-image.sh --image-tag dtconcepts/rhythm-rpiz-builder:dev
```

See [`scripts/build/README.md`](build/README.md) for the full flow, the hashed
inputs, and the `third_party/` pruning knobs used to keep image size down.

---

## Deployment Scripts

### release.sh

Create a Git release tag. By default this pushes the release so the GitHub
workflows can build and publish the assets.

There are two release modes:

- **Binary release (default)** — the rpiz OTA tarball via the tag-driven
  `rpiz Binary` job in `ci.yml`. Fast, ~5 min of CI. This is the normal
  cadence for appliance code changes.
- **Full image release (`--with-image`)** — everything above *plus* dispatches
  `rpiz-sd-image.yml`, which re-runs Buildroot end-to-end and attaches
  `sdcard.img` + `rootfs.ext2.gz` to the release. Use this when you bumped
  CHIP, Buildroot, or the defconfig.

Use `--upload` to keep the release local, build the `rpiz` artifact, and
upload the OTA feed directly instead of going through GitHub Actions.

```bash
./scripts/release.sh                    # Tags and pushes the next patch release (binary-only)
./scripts/release.sh --minor            # Tags and pushes the next minor release (binary-only)
./scripts/release.sh --version 0.4.1    # Tags and pushes an explicit version
./scripts/release.sh --with-image       # Binary release + dispatch rpiz-sd-image.yml for full SD card
./scripts/release.sh --upload           # Builds and uploads only the rpiz OTA feed locally
./scripts/release.sh --version 0.4.101  # Explicit high patch version is valid semver
./scripts/release.sh --dry-run          # Preview without creating the tag
```

**Options:**
| Flag | Description |
|------|-------------|
| `--version <semver>` | Explicit version, accepts `X.Y.Z` or `vX.Y.Z` |
| `--major` | Bump the latest release tag to the next major version |
| `--minor` | Bump the latest release tag to the next minor version |
| `--patch` | Bump the latest release tag to the next patch version (default) |
| `--with-image` | After pushing, dispatch `rpiz-sd-image.yml` to rebuild the SD-card image and attach it to the release |
| `--skip-builder-refresh` | Skip the builder-image hash check / refresh step (non-Linux hosts, or when you know the lock is right) |
| `--upload` | Build/package/upload the `rpiz` OTA feed locally; implies `--no-push` |
| `--message <text>` | Custom annotated tag message |
| `--remote <name>` | Git remote to push to, default `origin` |
| `--no-push` | Create the local tag without pushing |
| `--dry-run` | Show the planned actions without changing git state |

**Behavior:**
- Requires a clean tracked worktree before tagging.
- Updates the workspace version in `Cargo.toml` before tagging. It does not rewrite `Cargo.lock`, which avoids unrelated `rhythm-chipd` lockfile churn on macOS release hosts.
- Creates the release commit automatically when those version files change.
- Pushes the current branch and the new tag to `origin` by default.
- The GitHub Actions CI workflow turns that tag into the GitHub release with the rpiz OTA tarball.
- After a CDN publish, the server repo keeps only the latest five `v*` release directories per release root (`install/`, `rpiz/`, and the desktop target roots).
- `--upload` is local-only for now: it loads `.env`, builds only `rpiz`, packages only the `rpiz` OTA feed, uploads it over SSH, prunes old server releases, and leaves the branch/tag unpushed.
- `--upload` accepts either `RHYTHM_UPDATES_SSH_KEY_FILE` or `RHYTHM_UPDATES_SSH_KEY` for the SSH key material.

### Versioning

Rhythm OS now uses two versioning tracks:

- Workspace Rust binaries derive their build version from Git tags.
- `rhythm-addon` remains separate and uses `install/addon/config.yaml`.

For workspace/server/appliance builds:

- Tagged release builds resolve to the exact tag version only when the tracked worktree is clean, for example `v0.4.0` -> `0.4.0`.
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
./scripts/deploy-addon.sh                   # Build all + push to Docker Hub
./scripts/deploy-addon.sh --version 0.4.0   # Explicit addon version
./scripts/deploy-addon.sh --skip-build      # Use existing builds
./scripts/deploy-addon.sh --dry-run         # Show what would happen
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
./scripts/deploy-addon.sh --local                       # Deploy to HA
./scripts/deploy-addon.sh --local --ha-host 192.168.1.50
./scripts/deploy-addon.sh --local --ha-arch amd64       # x86 HA
./scripts/deploy-addon.sh --local --skip-build          # Just copy
./scripts/deploy-addon.sh --local --dry-run             # Preview
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
./scripts/run-dev.sh
```

### Development Cycle

```bash
# Make changes, then:
./scripts/deploy-addon.sh --local

# In HA: Settings → Add-ons → Rhythm OS → Rebuild
```

### Release

```bash
# 1. Update addon changelog if needed

# 2. Deploy addon
# Auto-bumps addon version from install/addon/config.yaml
./scripts/deploy-addon.sh

# Or pin the addon explicitly for a coordinated release
./scripts/deploy-addon.sh --version 0.4.0

# 3. Commit release changes
git add -A
git commit -m "Release v0.4.0"

# 4. Tag and push the server/appliance release
./scripts/release.sh
```

`./scripts/release.sh` defaults to the next patch tag after the latest `vX.Y.Z`.
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
