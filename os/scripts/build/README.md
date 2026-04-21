# scripts/build

Build-time scaffolding that is *not* part of the day-to-day developer flow.
Everything here exists to produce (or refresh) the builder Docker image that
both your local `--docker` runs and the CI `rpiz-image` workflow pull from.

## The content-addressed flow

The builder image is tagged by a hash of its baked inputs, so CI pulls a
specific tag and avoids image rebuilds for changes that don't affect it.

```
                 ┌────────────────────────┐
                 │ compute-image-hash.sh  │   (runs on Linux host)
                 │ hashes Dockerfile +    │
                 │ install/rpiz/buildroot │
                 │ + chip libCHIP.a +     │
                 │ buildroot rev +        │
                 │ toolchain bin, etc.    │
                 └───────────┬────────────┘
                             │ hash=<12-char-sha>
                             ▼
                 ┌────────────────────────┐
                 │ refresh-builder-       │     tag exists on
                 │ image.sh               │──── Docker Hub? ──── yes ──▶ done
                 └───────────┬────────────┘                              (just
                             │ no                                         update
                             ▼                                            lock)
                 ┌────────────────────────┐
                 │ build-rpiz-builder-    │
                 │ image.sh --push        │
                 │ (10 GB context, ~5min) │
                 └───────────┬────────────┘
                             ▼
                 dtconcepts/rhythm-rpiz-builder:v1-<hash>
                             ▲
                             │ lock file pinned to exact tag:
                             │   install/rpiz/builder-image.lock
                             │
      ┌──────────────────────┴──────────────────────┐
      │ ./scripts/build-rpiz-image.sh --docker      │
      │ (your Linux box OR CI rpiz-image.yml)       │
      │ → out/rpiz-docker/images/{sdcard,rootfs}    │
      └─────────────────────────────────────────────┘
```

`scripts/release.sh` calls `refresh-builder-image.sh --push` before tagging, so
a release commit automatically includes a bump to `builder-image.lock` iff the
image actually needs rebuilding. Ordinary app-code releases never trigger a
push; CHIP/Buildroot/defconfig bumps do.

## Files

- `Dockerfile.rpiz-builder` — multi-stage image definition. Bakes in the ARMv6
  musl cross toolchain, connectedhomeip source + `out/rpiz-arm-musl/` prebuilts,
  a pinned Buildroot checkout, the full Buildroot output (`out/rpiz/`) so the
  container resumes incrementally, and a Rust toolchain with the
  `arm-unknown-linux-musleabihf` target preinstalled.
- `build-rpiz-builder-image.sh` — packages the current host's state (chip
  source, toolchain, buildroot, `out/rpiz/`) into a clean Docker build context
  and runs `docker build` (+ optional `docker push`). Accepts `--image-tag
  <ref>` so the refresh script can tag by content hash.
- `compute-image-hash.sh` — reads the slow-path inputs and prints a 12-char
  sha prefix on stdout. Used as the image tag.
- `refresh-builder-image.sh` — computes the hash, queries Docker Hub to see if
  that tag already exists, pushes only if it doesn't, and updates
  `install/rpiz/builder-image.lock`. Invoked automatically by `release.sh`.

## What triggers an image refresh

Hashed into the tag (a change here flips the hash and forces a rebuild/push):

- `connectedhomeip/` commit SHA + any local diff (e.g. your BLE patch)
- `connectedhomeip/out/rpiz-arm-musl/lib/libCHIP.a` sha + `args.gn`
- `~/x-tools/arm-unknown-linux-musleabihf/bin/arm-unknown-linux-musleabihf-gcc`
  sha (rare — only when the toolchain itself is rebuilt)
- `buildroot/` commit SHA
- `install/rpiz/buildroot/**` (defconfig, fragments, overlays, packages)
- `scripts/build/Dockerfile.rpiz-builder`
- `scripts/build/build-rpiz-builder-image.sh`

NOT in the hash (code changes here never rebuild the image):

- Rust app code under `rust/**`
- `Cargo.toml` / `Cargo.lock`
- `scripts/build-rpiz-image.sh`, `scripts/build-server.sh` (run at invoke time)

## Refreshing

The normal path is automatic: `./scripts/release.sh` calls the refresh script
and only pushes a new image if the hash changed. You can also drive it by
hand:

```bash
# 0. Sanity check: local build must work first (populates out/rpiz/).
./scripts/build-rpiz-image.sh --release --prod

# 1. Check whether a rebuild is needed (no push).
./scripts/build/refresh-builder-image.sh

# 2. Build + push iff the tag isn't already on Docker Hub.
docker login                                      # dtconcepts account
./scripts/build/refresh-builder-image.sh --push

# 3. (rare) force a rebuild even if the tag exists.
./scripts/build/refresh-builder-image.sh --force
```

The lock file at `install/rpiz/builder-image.lock` is updated to the exact
image ref (e.g. `dtconcepts/rhythm-rpiz-builder:v1-aa91843f7d73`). Commit it
alongside whatever input change caused the hash to flip.

## Pruning knobs

The packaging script drops known-unused microcontroller vendor SDKs from
`connectedhomeip/third_party/` to keep the image reasonable. If a future chip
bump adds a dependency on a dropped dir, either:

- edit `CHIP_PRUNE_DEFAULT` in `build-rpiz-builder-image.sh` to remove the
  offending entry, or
- set `RHYTHM_CHIP_PRUNE_EXTRA="dir1 dir2"` to add *more* exclusions for a
  one-off build.
