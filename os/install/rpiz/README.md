# Raspberry Pi Zero

This target is for a Pi Zero / Zero W without Raspberry Pi OS. The Rust appliance crate is `rhythm-linux-appliance`; the `rpiz` target cross-compiles it for the Pi Zero's ARMv6 userspace, and the Buildroot external tree in `install/rpiz/buildroot` wraps the packaged appliance binary in a minimal image.

## What the target does

- Cross-builds `rhythm-server` for `arm-unknown-linux-musleabihf`
- Boots a minimal Buildroot image instead of Raspberry Pi OS
- Lays out the SD card as `boot + rootfs_a + rootfs_b + data` so future OTA can write the inactive rootfs slot instead of patching live files
- Starts Rhythm automatically at boot under BusyBox `init` with `RHYTHM_PLATFORM_TYPE=appliance` and `RHYTHM_PLATFORM_CONTEXT=rpiz`
- Uses BusyBox `init` `respawn`, not `systemd`, so the appliance always brings `rhythm-server` back if it exits
- Mounts `/boot` from the FAT partition and `/data` from the dedicated persistent partition
- Writes the main appliance log to `/data/log/rhythm-server.log` and raw Matter `rhythm-chipd` output to `/data/log/rhythm-matter.log`
- Prunes appliance logs in place on a background loop so the long-running append-only file descriptors stay bounded
- Brings up `usb0` at `192.168.7.2/24` for first-boot API testing over the Pi Zero OTG port
- Optionally embeds Wi-Fi credentials for Pi Zero W / Zero 2 W images
- Bundles `cloudflared` and the BusyBox init service used by app-controlled remote access tunnels

## Build the server binary

The easiest cross-build path is `cross`:

```bash
cargo install cross
./scripts/build-server.sh --release --target rpiz
```

To include the direct native Matter bridge in `rhythm-chipd`, point the build at
target-specific `connectedhomeip` artifacts first:

```bash
RHYTHM_CHIP_OUT_DIR=/path/to/connectedhomeip/out/<rpiz-target> \
./scripts/build-server.sh --release --target rpiz
```

For cross-target builds, `RHYTHM_CHIP_ROOT` alone is not enough; the bridge
needs a target-matched `libCHIP.a` via `RHYTHM_CHIP_OUT_DIR` or
`RHYTHM_CHIP_LIB_DIR`.

`rpiz` image builds now default to bring-up mode so early commissioning works
out of the box. The lab-only extras are:

- Dropbear SSH
- root password `rhythm`
- `RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=1` for the appliance at boot

That produces:

```bash
dist/bin/rpiz/rhythm-server
```

## Build the SD-card image

If `./buildroot` does not exist, the helper script now clones Buildroot there automatically.

```bash
./scripts/build-rpiz-image.sh --release
./scripts/build-rpiz-image.sh --release --prod
```

Use `--prod` or `RHYTHM_DEV_MODE=0` when you want a production-style image
without Dropbear, without the known root password, and with Matter device
attestation enforced. Production images write `/etc/default/rhythm` with:

```sh
RHYTHM_DEV_MODE=0
RHYTHM_MATTER_PAA_TRUST_STORE_PATH=/data/matter/paa-root-certs
RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION=0
RHYTHM_MATTER_ALLOW_TEST_PAA=0
```

Provision CSA production PAA roots under that trust-store directory before
commissioning production devices. You can override the baked path during the
image build with `RHYTHM_PROD_PAA_TRUST_STORE_PATH`.

The image lands at:

```bash
out/rpiz/images/sdcard.img
```

The build also writes the rootfs OTA artifact that can be published into the OTA feed:

```bash
out/rpiz/images/rootfs.ext2.gz
```

Buildroot still produces a raw `rootfs.ext2` internally while assembling
`sdcard.img`, but the published OTA artifact should normally be
`rootfs.ext2.gz`.

### Docker-backed image build

For a reproducible build (and the same flow CI runs), use `--docker`:

```bash
./scripts/build-rpiz-image.sh --release --docker
```

That pulls the exact builder image tag pinned in `install/rpiz/builder-image.lock` (something like `dtconcepts/rhythm-rpiz-builder:v1-<hash>`) from Docker Hub and runs the full cross-compile + Buildroot image step inside it. The image carries the ARMv6 musl toolchain, the `connectedhomeip` source + `out/rpiz-arm-musl/` prebuilts, a pinned Buildroot checkout, the full `out/rpiz/` Buildroot output so `make` resumes instead of rebuilding, and a Rust toolchain with the `arm-unknown-linux-musleabihf` target preinstalled, so nothing on the host besides Docker is required.

By default, the Docker flow writes images to `out/rpiz-docker` so it does not reuse non-Docker Buildroot host artifacts from `out/rpiz`.

Flags relevant to the Docker flow:

- `--docker-image <ref>` — override the image ref (default `dtconcepts/rhythm-rpiz-builder:latest`).
- `--no-pull` — skip `docker pull` and reuse the locally cached image.

To embed Wi-Fi credentials for a Pi Zero W / Zero 2 W image:

```bash
./scripts/build-rpiz-image.sh \
  --release \
  --docker \
  --wifi-ssid "YourSSID" \
  --wifi-psk "YourPassword" \
  --wifi-country US
```

The default country is `US`. Credentials are written into `/etc/wpa_supplicant.conf` during the image build and are not committed back into the repo.

If you want to use a non-default Buildroot checkout in the non-Docker flow, pass `--buildroot-dir /path/to/buildroot`. (Under `--docker`, Buildroot is provided by the image at `/opt/buildroot` and this flag is ignored.) The script only auto-clones the default `./buildroot` path.

### Release modes

The rpiz release flow has two modes that map to two workflows:

| Mode | How you trigger it | What you get | CI time |
|------|--------------------|--------------|---------|
| **Binary release** (default) | `./scripts/release.sh` (any variant without `--with-image`) | rhythm-server rpiz tarball for OTA (tag-driven `rpiz Binary` job in `ci.yml`) | ~5 min |
| **Full image release** | `./scripts/release.sh --with-image` | Everything above *plus* sdcard.img + rootfs.ext2.gz attached to the release (`rpiz-sd-image.yml` dispatched via `gh`) | ~5 min + one full Buildroot pass |

Use the binary mode for normal appliance code / Rust-level changes — your Pi Zeros update via OTA against the tarball without needing a new SD card. Use `--with-image` when you've bumped CHIP, Buildroot, the defconfig, or the kernel config (anything that forces a new rootfs). You can also dispatch `rpiz-sd-image.yml` manually at any time:

```bash
gh workflow run rpiz-sd-image.yml -f tag=v0.5.0   # attaches to an existing release
gh workflow run rpiz-sd-image.yml                 # artifact-only rebuild from current branch
```

`rpiz-sd-image.yml` now matches the tag flavor when it builds the rootfs: `*-beta`
tags keep the bring-up image defaults (`RHYTHM_DEV_MODE=1`, Matter device
attestation bypass enabled), while stable tags build the production image. To
force a production-style image from a beta tag for field testing, dispatch with
`image_mode=prod`:

```bash
gh workflow run rpiz-sd-image.yml -f tag=v0.5.0-beta -f image_mode=prod
```

### Refreshing the builder image

The `dtconcepts/rhythm-rpiz-builder` image is content-addressed: its tag is a hash of the baked inputs (CHIP source + prebuilts, ARMv6 musl toolchain, Buildroot checkout + defconfig/fragments/overlays, Dockerfile, packaging script). Most app-code changes don't touch any of those, so most releases don't rebuild the image.

On your Linux host, after a successful local `./scripts/build-rpiz-image.sh --release --prod`:

```bash
# Check whether the hash matches what's already on Docker Hub.
./scripts/build/refresh-builder-image.sh

# Build + push iff the hash has shifted and that tag is missing.
docker login                                      # dtconcepts account
./scripts/build/refresh-builder-image.sh --push
```

`./scripts/release.sh` invokes this script before tagging; the lock file bump is automatically included in the release commit when an image push was needed. Pass `--skip-builder-refresh` to `release.sh` only when you're cutting a release from a non-Linux machine and you know the lock file already points at a published tag.

See `scripts/build/README.md` for the hash inputs and the refresh mechanics.

## Flash the SD card

Example on macOS:

```bash
diskutil list
diskutil unmountDisk /dev/diskN
sudo dd if=out/rpiz/images/sdcard.img of=/dev/rdiskN bs=4m conv=sync
diskutil eject /dev/diskN
```

Use the Pi Zero's USB OTG/data port, not the power-only port.

## Image OTA model

The `rpiz` appliance now treats image OTA as an A/B rootfs switch:

- The running slot is selected by `root=/dev/mmcblk0p2` (`rootfs_a`) or `root=/dev/mmcblk0p3` (`rootfs_b`) in `/boot/cmdline.txt`
- `/boot/rhythm-bootstate.env` records the active slot, pending slot, and last update metadata
- `POST /api/ota/update` on an appliance `rpiz` server prefers a published `rootfs.ext2.gz` or `rootfs.ext2` artifact, writes it to the inactive slot, updates `/boot/cmdline.txt`, and reboots
- Trial boots automatically roll back to the last-good slot if the candidate image reboots or `rhythm-server` exits before startup is marked healthy
- `/data` lives on `mmcblk0p4`, so backups, topology, captures, and update staging survive slot switches

This is still userspace rollback, not a bootloader-managed A/B system. The next slot is selected by rewriting `/boot/cmdline.txt`, and rollback still requires the candidate image to boot far enough to reach init / `rhythm-launch`. The published `sdcard.img` remains the factory/master image for fresh cards and hard recovery.

## USB-first smoke test

On first boot the image loads the USB Ethernet gadget and assigns:

- Host side: configure `192.168.7.1/24`
- Pi side: `192.168.7.2/24`
- Rhythm API: `http://192.168.7.2:54448/api/state`

Minimal smoke test:

```bash
curl http://192.168.7.2:54448/api/state
```

Once that works, you can decide whether to keep the device USB-managed, add Wi-Fi, or extend the image further.

If your board is the original Pi Zero without onboard Wi-Fi, the Wi-Fi flags above will not connect anything unless you attach a supported USB Wi-Fi adapter.

## BLE provisioning

The rpiz image now includes a BlueZ-based BLE provisioning sidecar that reuses
the shared Rhythm provisioning GATT contract.

- On boot, BLE provisioning starts automatically when the Pi does not have an active Wi-Fi IP
- `GET /api/wifi` returns the current Wi-Fi/provisioning status
- `DELETE /api/wifi` clears `/etc/wpa_supplicant.conf`, restarts Wi-Fi, and re-enables BLE provisioning

For test sessions on a Pi that is already connected to Wi-Fi, force the
provisioning sidecar on with:

```bash
RHYTHM_BLE_PROVISION_ALWAYS=1 /usr/bin/rhythm-server --data-dir /data --log-level info
```

## Remote access tunnel

The rpiz image includes the pinned Cloudflare ARMv6 `cloudflared` release
asset, installed at `/usr/bin/cloudflared`, plus
`/etc/init.d/rhythm-cloudflared`.

The service is inert until the app enables remote access. Enabling remote
access writes `/data/cloudflared/connector_token` through the authenticated
local API, then restarts `rhythm-cloudflared`. On later boots,
`rhythm-server` starts the connector after the appliance HTTP API is already
listening, so tunnel startup cannot block the core device boot path.

Defaults are tuned for the original Pi Zero W:

```sh
RHYTHM_CLOUDFLARED_PROTOCOL=http2
RHYTHM_CLOUDFLARED_LOGLEVEL=warn
RHYTHM_CLOUDFLARED_HA_CONNECTIONS=1
RHYTHM_CLOUDFLARED_MAX_RESTARTS=5
RHYTHM_CLOUDFLARED_VMEM_LIMIT_KB=262144
RHYTHM_CLOUDFLARED_NICE=10
```

The service caps the connector process and stops retrying after repeated
failures so a tunnel problem cannot turn into an appliance-wide crash loop.

Override those in `/etc/default/rhythm` or `/etc/default/rhythm-dev` if a
field test needs Cloudflare's default `auto` transport or more HA connections.

## Appliance log pruning

`rpiz` keeps appliance-owned logs under `/data/log` and rotates them in place
every 5 minutes by default. The active files are truncated after the tail is
copied into numbered rotations, which keeps the logs bounded without requiring
`rhythm-server` or `rhythm-chipd` to reopen their file descriptors.

Default limits:

- `rhythm-server.log`: 4 MiB, keep 2 rotations
- `rhythm-matter.log`: 4 MiB, keep 3 rotations
- `wifi.log`: 256 KiB, keep 2 rotations
- `bluetooth.log`: 256 KiB, keep 2 rotations

Override the log directory, prune interval, or size limits in
`/etc/default/rhythm-dev` with:

```sh
RHYTHM_LOG_DIR=/data/log
RHYTHM_LOG_PRUNE_INTERVAL_SECS=300
RHYTHM_SERVER_LOG_MAX_BYTES=4194304
RHYTHM_SERVER_LOG_KEEP=2
RHYTHM_MATTER_LOG_MAX_BYTES=4194304
RHYTHM_MATTER_LOG_KEEP=3
RHYTHM_WIFI_LOG_MAX_BYTES=262144
RHYTHM_WIFI_LOG_KEEP=2
RHYTHM_BLUETOOTH_LOG_MAX_BYTES=262144
RHYTHM_BLUETOOTH_LOG_KEEP=2
```

`rpiz` sets `RHYTHM_MATTER_LOGFILE=/data/log/rhythm-matter.log` by default so
raw CHIP/Matter daemon output (`[DMG]`, `[EM]`, `[CSM]`, `[DIS]`, etc.) stays
out of `/data/log/rhythm-server.log`. Override that variable, or set it to an
empty string in `/etc/default/rhythm-dev`, if you want `rhythm-chipd` to use a
different file or inherit the main appliance log sink again.

## Debug bundle

`rpiz` also exposes an on-demand debug export:

```http
POST /api/diag/debug-bundle
```

The response body is a `tar.gz` attachment. It is meant for support/debugging
workflows where the app or a local client wants one bounded snapshot of the
appliance state without SSH access.

Example:

```bash
curl -X POST \
  http://192.168.7.2:54448/api/diag/debug-bundle \
  --output rhythm-debug-bundle.tar.gz
```

The archive currently includes:

- `manifest.json` with bundle metadata and the exact log files captured
- `state.json` from the normal redacted state snapshot
- `profile_bundle.json` from the normal profile-bundle export
- matching files from `/data/log`, including rotated variants for:
- `rhythm-server.log`, `rhythm-server.log.1`, ...
- `rhythm-matter.log`, `rhythm-matter.log.1`, ...
- `wifi.log`, `wifi.log.1`, ...
- `bluetooth.log`, `bluetooth.log.1`, ...

The endpoint only bundles file-backed appliance logs. It does not include hub
credentials, Wi-Fi passwords, or a secrets-included backup bundle.
