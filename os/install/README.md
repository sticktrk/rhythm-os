# Installation

Platform-specific instructions for installing and running Rhythm OS.

## Server (macOS / Linux)

The hosted macOS/Linux binary installer and its legacy
`/server/install/` artifact tree are retired. Desktop/server operators build
from source and use the local installer below. Raspberry Pi Zero appliances
start from the current production SD-card image at
<https://dl.rhythm.lighting/server/sdcard.img.gz> and subsequently self-update
through the stable OTA feed.

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

`install/install.sh` is the supported installer. It expects binaries at
`dist/bin/<platform>/` or via `--binary <path>`. The
`install/pages/install.sh` endpoint exists only to give cached hosted-installer
links an explicit retirement message.

The server runs on port `54448` by default and advertises via mDNS.

## Raspberry Pi Zero appliance

For a non-Raspberry-Pi-OS setup, use the `rpiz` target plus the Buildroot image scaffolding in [install/rpiz/README.md](rpiz/README.md). The Rust appliance crate is `rhythm-linux-appliance`; the image boots the packaged appliance binary on a minimal Linux rootfs and is provisioned over BLE (or with baked-in Wi-Fi credentials). USB gadget access is compiled into the kernel but not yet wired up — see the rpiz README for current status.

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
