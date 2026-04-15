# Installation

Platform-specific instructions for installing and running Rhythm OS.

## Server (macOS / Linux)

### Download

Pre-built binaries are available on the [Releases](https://github.com/sticktrk/rhythm-os/releases) page.

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | `rhythm-server-macos-arm64.tar.gz` |
| macOS (Intel) | `rhythm-server-macos-x86_64.tar.gz` |
| Linux (x86_64) | `rhythm-server-linux-amd64.tar.gz` |
| Linux (ARM64) | `rhythm-server-linux-aarch64.tar.gz` |
| Raspberry Pi Zero / Zero W | `rhythm-server-rpiz.tar.gz` |

Each archive contains `rhythm-server` and `rhythm-cli`.

### Build from source

```bash
./scripts/build-server.sh --release
./scripts/build-server.sh --release --target rpiz
```

### Install as a system service

The install script sets up auto-start and auto-restart:

```bash
./install/install.sh
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

# Uninstall
./install/install.sh --uninstall
```

The server runs on port `54448` by default and advertises via mDNS.

## Raspberry Pi Zero appliance

For a non-Raspberry-Pi-OS setup, use the `rpiz` target plus the Buildroot image scaffolding in [install/rpiz/README.md](rpiz/README.md). The Rust appliance crate is now `rhythm-linux-embedded`; the image path remains USB-first and boots the packaged appliance binary on a minimal Linux rootfs while bringing up `usb0` at `192.168.7.2` so you can smoke-test the API over USB before worrying about Wi-Fi or LAN.

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

## ESP32-C6

### Prerequisites

- ESP-IDF v5.5.2
- Rust ESP toolchain (`ldproxy`, `espflash`)

```bash
# One-time setup
cd ~/esp && git clone --recursive https://github.com/espressif/esp-idf.git
cd esp-idf && ./install.sh esp32c6
cargo install ldproxy espflash
```

### Build and flash

```bash
./scripts/build-esp32.sh --flash           # Build + flash + monitor
./scripts/build-esp32.sh --release --flash  # Release build
./scripts/build-esp32.sh --clean --flash    # Clean rebuild
```

### WiFi credentials

```bash
export WIFI_SSID="YourSSID"
export WIFI_PASS="YourPassword"
```
