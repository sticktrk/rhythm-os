# Installation

Platform-specific instructions for installing and running Rhythm OS.

## Server (macOS / Linux)

### One-line install (macOS / Linux desktop)

```bash
curl -fsSL https://get.rhythm.lighting/install.sh | bash
```

The bootstrap installer detects your platform, downloads the matching release tarball from the Rhythm CDN, verifies its SHA256, and sets up `rhythm-server` as a system service. Pass `--user` for a Linux user-level service, `--port` / `--log-level` to override defaults, `--no-start` to install without starting, or `--uninstall` to remove:

```bash
curl -fsSL https://get.rhythm.lighting/install.sh | bash -s -- --user --port 8080
curl -fsSL https://get.rhythm.lighting/install.sh | bash -s -- --uninstall
```

Set `RHYTHM_NO_TELEMETRY=1` to skip the post-install hit-counter ping.

### Download manually

Pre-built desktop tarballs live at `https://dl.rhythm.lighting/server/install/<tag>/`:

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | `rhythm-server-<version>-macos-arm64.tar.gz` |
| macOS (Intel) | `rhythm-server-<version>-macos-x86_64.tar.gz` |
| Linux (x86_64) | `rhythm-server-<version>-linux-amd64.tar.gz` |
| Linux (ARM64) | `rhythm-server-<version>-linux-aarch64.tar.gz` |

Each tarball contains `bin/rhythm-server`, `bin/rhythm-cli`, and a copy of `install/install.sh` so you can install offline. The current tag is at <https://dl.rhythm.lighting/server/install/latest.txt>.

The Raspberry Pi Zero appliance is not distributed via the desktop installer — it ships as the SD-card image and self-updates via the OTA feed at `https://dl.rhythm.lighting/server/rpiz/manifest.json`.

### Build from source

```bash
./scripts/build-server.sh --release
./scripts/build-server.sh --release --target rpiz
```

### Install as a system service

If you have the repo checked out (or extracted a tarball), the local installer sets up auto-start and auto-restart:

```bash
./install/install.sh                                             # uses dist/bin/<platform>/rhythm-server
./install/install.sh --binary path/to/rhythm-server              # use a specific binary
```

| Platform | Service manager | Binary | Data |
|----------|----------------|--------|------|
| macOS | launchd (LaunchAgent) | `/usr/local/bin/rhythm-server` | `~/.rhythm/` |
| Linux (root) | systemd (system) | `/usr/local/bin/rhythm-server` | `/var/lib/rhythm/` |
| Linux (user) | systemd (user) | `~/.local/bin/rhythm-server` | `~/.rhythm/` |

```bash
# Linux user-level install (no root)
./install/install.sh --user

# Custom port or log level
./install/install.sh --port 8080 --log-level debug

# Install without starting the service
./install/install.sh --no-start            # also via RHYTHM_NO_START=1 or CI=true

# Uninstall
./install/install.sh --uninstall
```

Two install paths to keep straight:

- **`install/install.sh`** — local-repo / extracted-tarball installer. Expects binaries at `dist/bin/<platform>/` or via `--binary <path>`.
- **`install/pages/install.sh`** — bootstrap installer served at `https://get.rhythm.lighting/install.sh`. Downloads the release tarball from the Rhythm CDN, verifies SHA256, and hands off to the bundled `install/install.sh`.

The server runs on port `54448` by default and advertises via mDNS.

## Raspberry Pi Zero appliance

For a non-Raspberry-Pi-OS setup, use the `rpiz` target plus the Buildroot image scaffolding in [install/rpiz/README.md](rpiz/README.md). The Rust appliance crate is `rhythm-linux-appliance`; the image path remains USB-first and boots the packaged appliance binary on a minimal Linux rootfs while bringing up `usb0` at `192.168.7.2` so you can smoke-test the API over USB before worrying about Wi-Fi or LAN.

### Management

```bash
rhythm-cli status
rhythm-cli stop
rhythm-cli start
rhythm-cli update
rhythm-cli uninstall
```

## Home Assistant add-on

Add the [Rhythm OS add-on repository](https://github.com/sticktrk/rhythm-os-addon) to your Home Assistant instance, then install from the add-on store. The add-on auto-configures from HA Supervisor (token, location).

To build the Docker image locally:

```bash
./scripts/build-addon.sh
```
