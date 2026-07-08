# OTA Updates — the canonical guide

One tag-driven pipeline, two channels, two payload tiers. Everything else is
detail.

```
                       ┌────────────────────────────────────────────────┐
release.sh ── tag ────▶│ ci.yml: release-plan → binaries (+image?) →    │──▶ dl.rhythm.lighting/server/rpiz/
  --promote-stable ───▶│          publish (one manifest per release)    │──▶ dl.rhythm.lighting/server/rpiz-stable/
                       └────────────────────────────────────────────────┘
                                                                  ▲ polled by appliances
```

## Terminology (three things "dev" used to mean — now separated)

| Term | Values | What it is |
|------|--------|------------|
| **Channel** | `beta` / `stable` | Which OTA feed a device polls. Beta = every tagged release (`rpiz/`). Stable = promoted releases (`rpiz-stable/`), consumed by the fleet's daily auto-update window. |
| **Image posture** | `dev` / `prod` | Security posture *baked into a rootfs image*: dev = bring-up extras (Dropbear, root password `rhythm`, Matter attestation bypass); prod = no Dropbear or known root password, while Matter attestation bypass remains enabled until production PAA provisioning is wired. Flags: `build-rpiz-image.sh --dev/--prod`, workflow input `image_mode`. `auto` maps beta→dev, stable→prod. |
| **Dev loop** | — | `push-rpiz-dev.sh`: cross-compile and scp a binary straight to a bench device. Not OTA at all. |

## Channels

- Tags: `vX.Y.Z-beta` (every release) and `vX.Y.Z-stable` (promotion of the
  same commit). The tag suffix decides channel, feed, and GH prerelease flag.
- Devices choose their feed with the `update_channel` setting
  (`PUT /api/settings {"update_channel": "beta"|"stable"}`; shown in
  `/api/ota/status.channel` and the admin device console System page).
  **Default: stable.** Only test hubs should run beta.
- `auto_update` (default on) means *only* "apply updates automatically in the
  daily 14:00–16:00 window". It works on either channel and no longer implies
  anything about the channel. (Historically `auto_update=false` silently moved
  a device to the beta feed; that coupling is gone.)

## Payload tiers: binary vs image

| | Binary (package) | Full image (rootfs) |
|---|---|---|
| Ships | `rhythm-server` + `rhythm-chipd` tarball (~MB) | `rootfs.ext2.gz` A/B slot image (+`sdcard.img.gz` factory image) |
| When | every release | only when the **rootfs fingerprint** changed (or `--with-image` forces it) |
| Applied by device | staged binary swap + supervisor restart, 3-start probation rollback | dd to inactive slot, binaries overlaid into it, cmdline.txt switch, two-boot probation via S41bootstate |

### The rootfs fingerprint

`tools/os/scripts/compute-rootfs-fingerprint.sh --image-mode dev|prod` hashes
everything that bakes into the rootfs:

- the builder-image content hash from `os/install/rpiz/builder-image.lock`
  (covers CHIP prebuilts, toolchain, Buildroot checkout, builder Dockerfile),
- the Buildroot external tree `os/install/rpiz/buildroot/**`
  (defconfig, overlays, post-build.sh, S41bootstate, packages),
- `tools/os/scripts/build-rpiz-image.sh`,
- the image posture (dev/prod — posture changes rootfs contents).

Rust sources are deliberately **not** hashed: the OTA package overlay
refreshes the binaries on any rootfs, so they don't define the base.

The fingerprint is stamped into the image at `/etc/rhythm-image-fingerprint`
(next to `/etc/rhythm-image-version`) and recorded on each fresh
`images[]` entry in the feed manifest.

### The auto-image gate (ci.yml `release-plan`)

On every tag, CI compares the checkout's fingerprint against the newest
published `rootfs_image` entry in the target feed:

- **match** → binary-only release. The manifest **carries the existing image
  entries forward** (both channels), so a device that is behind on its image
  base still sees the correct rootfs + the new binary in one update.
- **mismatch / missing / `[with-image]` tag marker** → the Buildroot image
  job is chained into the same release and the manifest gets fresh
  fingerprinted image entries.

`release.sh --with-image` just embeds the `[with-image image_mode=…]` marker
in the tag message — it forces the image leg, nothing else.

`release.sh --no-image` embeds the `[no-image]` marker instead: no image
build **and no carry-forward** — the manifest ships with `images: []`, so
devices take the package-only path. Escape hatch for firmware whose image
path is broken (see fleet notes). Because the resulting manifest has no
fingerprinted image, the *next* normal release auto-builds a fresh image and
re-seeds the entries — restoration is automatic.

> First release after introducing fingerprints: the published feeds have no
> fingerprint, so the gate force-builds one image per channel to seed them.
> Expect Buildroot time once; binary-only is the steady state afterwards.

## Feed layout & manifest

```
https://dl.rhythm.lighting/server/
  rpiz/            (beta)              rpiz-stable/        (stable)
    manifest.json                        manifest.json
    v<ver>/rhythm-server-rpiz.tar.gz     v<ver>/...
    v<ver>/rootfs.ext2.gz  (image releases)
    v<ver>/sdcard.img.gz   (image releases; factory flash, never OTA-applied)
    latest/{rootfs.ext2.gz,sdcard.img.gz}
```

`manifest.json` (written only by `tools/os/scripts/package-server-updates.sh`):

```json
{
  "version": "0.7.0-beta", "channel": "beta", "published_at": "…",
  "package": { "kind": "archive_bundle", "url": "v0.7.0-beta/rhythm-server-rpiz.tar.gz",
               "sha256": "…", "size": 123, "install": [ … slots: self|sibling|absolute … ] },
  "images": [ { "kind": "rootfs_image", "url": "v0.6.9-beta/rootfs.ext2.gz",
                "version": "0.6.9-beta", "sha256": "…", "size": 456,
                "fingerprint": "v1-dev-a1b2c3d4e5f6", "compression": "gzip" } ]
}
```

Carried-forward image entries keep their original `v<ver>/` URL; the CDN
prune script never deletes a version directory that any feed manifest still
references.

## Device state files (rpiz appliance)

| Path | Purpose |
|------|---------|
| `/boot/rhythm-bootstate.env` (+`.bak`) | A/B slot state machine (hashed; two-boot probation via `S41bootstate`) |
| `/boot/cmdline.txt` | `root=` selects rootfs slot p2/p3 |
| `/etc/rhythm-image-version`, `/etc/rhythm-image-fingerprint` | identity of the running rootfs base |
| `/data/ota/` | download staging, bootstate mirror, auto-update history (`auto-update-state.json`) |
| `/usr/bin/.rhythm-update-pending.json` etc. | binary-swap probation (3 start attempts), last-rollback, drift-repair budget |
| settings JSON in `/data` | `update_channel`, `auto_update` |

## Runbooks

```bash
# Ship a beta (day-to-day). CI decides binary-only vs +image automatically.
./tools/os/scripts/release.sh                     # next patch; --minor/--major/--version X.Y.Z

# Promote a tested beta to stable (same commit, rebuilt with -stable, prod image if needed).
./tools/os/scripts/release.sh --promote-stable            # latest beta
./tools/os/scripts/release.sh --promote-stable 0.7.0      # explicit

# Force a full image rebuild despite an unchanged fingerprint (rare).
./tools/os/scripts/release.sh --with-image [--image-mode dev|prod]

# Binary-only rescue release: manifest ships with NO image entries, so old
# updaters take the package-only path. Next normal release re-seeds the image.
./tools/os/scripts/release.sh --no-image
./tools/os/scripts/release.sh --promote-stable --no-image

# Escape hatch: build + publish the feed locally when GitHub Actions is down.
./tools/os/scripts/release.sh --upload            # uses .env RHYTHM_UPDATES_* creds

# Factory/manual image build without the tag pipeline:
gh workflow run rpiz-sd-image.yml -f tag=vX.Y.Z-beta -f image_mode=auto \
    -f publish_full_image_ota=false

# Bench dev loop (no OTA, no git):
./tools/os/scripts/push-rpiz-dev.sh --host <device-ip>

# Flip a hub's channel (admin device console → System, or directly):
curl -X PUT http://<hub>/api/settings -d '{"update_channel":"beta"}'
```

## Fleet behavior notes

- A stable release outranks its own beta (`0.7.0-stable > 0.7.0-beta`), so a
  device switched beta→stable at the same core version sees one rebuild
  "update". Downgrades are always refused device-side.
- Devices that had auto-update off were historically on the silent beta feed;
  they now resolve to **stable + manual**. After shipping that change, promote
  a stable release promptly so those devices have something to update to.
- The device detects three update reasons: `version_mismatch` (normal),
  `image_base_drift` (rootfs older than the manifest's image entry → image +
  package applied together), `component_drift` (sibling binary version skew →
  repair bundle).
- Firmware older than 0.6.223 **cannot apply image updates at all**: it
  mounted the freshly written inactive slot with `-t ext2`, but the images
  have always been ext4, so the mount fails (`EINVAL`) and the whole update
  aborts — including the package half. Such devices can only consume
  manifests whose image entries are no newer than their image base; get them
  past 0.6.223 via a `--no-image` release or `push-rpiz-dev.sh` first.
