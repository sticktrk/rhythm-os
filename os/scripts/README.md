# Build & Deploy Scripts

This directory contains all build and deployment scripts for Rhythm OS.

## Quick Reference

```bash
# Development
./scripts/run-dev.sh                    # Build and run addon locally

# Build individual components
./scripts/build-rust.sh                 # Build Rust addon

# Deploy
./scripts/deploy-addon.sh               # Push addon to Docker Hub
./scripts/deploy-addon.sh --local       # Deploy to local HA for testing
```

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

---

## Deployment Scripts

### deploy-addon.sh

Deploy the addon to Docker Hub or directly to a Home Assistant instance.

#### Docker Hub (Production)

```bash
./scripts/deploy-addon.sh                   # Build all + push to Docker Hub
./scripts/deploy-addon.sh --version 1.0.0   # Specific version tag
./scripts/deploy-addon.sh --skip-build      # Use existing builds
./scripts/deploy-addon.sh --dry-run         # Show what would happen
```

**Prerequisites:**
- Docker with buildx support
- Logged into Docker Hub: `docker login`

**What it does:**
1. Builds Rust binaries for all architectures
2. Adds `image: dtconcepts/rhythm-os-addon` to `config.yaml`
4. Builds multi-arch Docker image
5. Pushes to Docker Hub with version + `latest` tags

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
4. Copies addon to HA via SCP
5. HA rebuilds on next install/update

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
# 1. Update version in addon/config.yaml

# 2. Update changelog in addon/CHANGELOG.md

# 3. Deploy addon to Docker Hub
./scripts/deploy-addon.sh

# 4. Commit and tag
git add -A
git commit -m "Release v1.0.0"
git tag v1.0.0
git push && git push --tags
```

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
