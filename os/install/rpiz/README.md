# Raspberry Pi Zero

This target is for a Pi Zero / Zero W without Raspberry Pi OS. The Rust appliance crate is `rhythm-linux-embedded`; the `rpiz` target cross-compiles it for the Pi Zero's ARMv6 userspace, and the Buildroot external tree in `install/rpiz/buildroot` wraps the packaged appliance binary in a minimal image.

## What the target does

- Cross-builds `rhythm-server` for `arm-unknown-linux-musleabihf`
- Boots a minimal Buildroot image instead of Raspberry Pi OS
- Starts Rhythm automatically at boot under BusyBox `init` with `RHYTHM_PLATFORM_TYPE=embedded` and `RHYTHM_PLATFORM_CONTEXT=rpiz`
- Uses BusyBox `init` `respawn`, not `systemd`, so the appliance always brings `rhythm-server` back if it exits
- Brings up `usb0` at `192.168.7.2/24` for first-boot API testing over the Pi Zero OTG port
- Optionally embeds Wi-Fi credentials for Pi Zero W / Zero 2 W images

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

That produces:

```bash
dist/bin/rpiz/rhythm-linux-embedded
dist/bin/rpiz/rhythm-server
```

## Build the SD-card image

If `./buildroot` does not exist, the helper script now clones Buildroot there automatically.

```bash
./scripts/build-rpiz-image.sh --release
```

The image lands at:

```bash
out/rpiz/images/sdcard.img
```

### Docker-backed image build

If your host is macOS, or you just want the Linux image build isolated, use Docker:

```bash
./scripts/build-rpiz-image.sh --release --docker
```

That flow still builds the `rhythm-linux-embedded` appliance on the host, writes the compatibility binary to `dist/bin/rpiz/rhythm-server`, then runs the Buildroot image step in a Debian-based Docker container defined by `install/rpiz/docker/Dockerfile`.
By default, the Docker flow writes images to `out/rpiz-docker` so it does not reuse macOS-generated Buildroot host artifacts from `out/rpiz`.

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

If you want to use a non-default Buildroot checkout, pass `--buildroot-dir /path/to/buildroot`. The script only auto-clones the default `./buildroot` path.

## Flash the SD card

Example on macOS:

```bash
diskutil list
diskutil unmountDisk /dev/diskN
sudo dd if=out/rpiz/images/sdcard.img of=/dev/rdiskN bs=4m conv=sync
diskutil eject /dev/diskN
```

Use the Pi Zero's USB OTG/data port, not the power-only port.

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
