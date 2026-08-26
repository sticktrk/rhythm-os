# tools/os/scripts/build

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
      │ ./tools/os/scripts/build-rpiz-image.sh --docker      │
      │ (your Linux box OR CI rpiz-image.yml)       │
      │ → out/rpiz-docker/images/{sdcard,rootfs}    │
      └─────────────────────────────────────────────┘
```

`tools/os/scripts/release.sh` calls `refresh-builder-image.sh --push` before tagging, so
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
  `install/rpiz/builder-image.lock` (image ref, hash, and the `chip_*` pin
  lines). Invoked automatically by `release.sh`.
- `chip-bridge-syntax-check.sh` — `clang++ -fsyntax-only` of the native
  Matter bridge against a connectedhomeip checkout; runs on macOS. Not the
  real build, but resolves every SDK identifier and instantiates templates.

## What triggers an image refresh

Hashed into the tag (a change here flips the hash and forces a rebuild/push):

- `connectedhomeip/` commit SHA + any local diff (e.g. your BLE patch)
- `connectedhomeip/out/rpiz-arm-musl/lib/libCHIP.a` sha + `args.gn`
- `~/x-tools/arm-unknown-linux-musleabihf/bin/arm-unknown-linux-musleabihf-gcc`
  sha (rare — only when the toolchain itself is rebuilt)
- `buildroot/` commit SHA
- `install/rpiz/buildroot/**` (defconfig, fragments, overlays, packages)
- `tools/os/scripts/build/Dockerfile.rpiz-builder`
- `tools/os/scripts/build/build-rpiz-builder-image.sh`

NOT in the hash (code changes here never rebuild the image):

- Rust app code under `rust/**`
- `Cargo.toml` / `Cargo.lock`
- `tools/os/scripts/build-rpiz-image.sh`, `tools/os/scripts/build-server.sh` (run at invoke time)

## Refreshing

The normal path is automatic: `./tools/os/scripts/release.sh` calls the refresh script
and only pushes a new image if the hash changed. You can also drive it by
hand:

```bash
# 0. Sanity check: local build must work first (populates out/rpiz/).
./tools/os/scripts/build-rpiz-image.sh --release --prod

# 1. Check whether a rebuild is needed (no push).
./tools/os/scripts/build/refresh-builder-image.sh

# 2. Build + push iff the tag isn't already on Docker Hub.
docker login                                      # dtconcepts account
./tools/os/scripts/build/refresh-builder-image.sh --push

# 3. (rare) force a rebuild even if the tag exists.
./tools/os/scripts/build/refresh-builder-image.sh --force
```

The lock file at `install/rpiz/builder-image.lock` is updated to the exact
image ref (e.g. `dtconcepts/rhythm-rpiz-builder:v1-aa91843f7d73`). Commit it
alongside whatever input change caused the hash to flip.

The lock also records which `connectedhomeip` checkout the image bakes, so
the SDK a release was built against is readable from the repo without the
release host:

```text
chip_rev=<full commit SHA>
chip_ref=<git describe --tags --always, e.g. v1.5.1.0 or v1.4.2.0-2520-gb468bbbea0>
chip_diff=<sha256 of the uncommitted local diff, or "clean">
```

These lines are informational — the SHA and local diff are already part of
`hash=`, so editing or adding them never forces a rebuild. Print them on any
host with `compute-image-hash.sh --chip-info` (set `RHYTHM_CHIP_SRC_DIR` if
the checkout is not at `os/connectedhomeip`). Cite `chip_ref` in any PR that
changes `os/rust/bins/rhythm-chipd/native/chip_bridge.cc`.

## Bumping connectedhomeip

Only bump for a reason: an SDK security fix, an mDNS / AddressResolve / CASE
bug actually observed in the field, a spec-version requirement, or an API the
bridge needs. Every bump costs a prebuilt rebuild, a builder-image push, and a
Matter soak (commissioning incl. BLE, subscriptions, room fan-out).

Pin to a **release tag** (`v1.5.x`), not a `master` snapshot: same effort,
reproducible, and there is a changelog to read before soaking. `chip_ref`
should then read as a plain version.

On the release host:

```bash
cd os/connectedhomeip
git fetch --tags origin
git checkout v1.5.1.0                             # release tag, not master
# re-apply the local BLE patch if this host carries one (chip_diff != clean)
git submodule update --init --recursive
# rebuild the prebuilts the bridge links against (host + rpiz musl)
gn gen out/host && ninja -C out/host
gn gen out/rpiz-arm-musl --args="$(cat out/rpiz-arm-musl/args.gn)" && ninja -C out/rpiz-arm-musl
cd ../..

# fast header/semantic check of the bridge before a full build (works on macOS too)
RHYTHM_CHIP_ROOT=os/connectedhomeip RHYTHM_CHIP_OUT_DIR=os/connectedhomeip/out/host \
  tools/os/scripts/build/chip-bridge-syntax-check.sh

# full native build + tests, then the image
RHYTHM_CHIP_ROOT=os/connectedhomeip RHYTHM_CHIP_OUT_DIR=os/connectedhomeip/out/host \
  cargo test -p rhythm-chipd --features chip-ffi
./tools/os/scripts/build/refresh-builder-image.sh --push     # new hash, new tag, lock updated
```

The bridge uses semi-internal SDK APIs (`data-model-providers/codegen`,
`ExamplePersistentStorage`, `ExampleOperationalCredentialsIssuer`,
`FileAttestationTrustStore`) that move between releases; expect small bridge
fixes. Land the bump as its own PR with the new `chip_ref` in the title and
soak it on beta before anything else stacks on it.

## Pruning knobs

The packaging script drops known-unused microcontroller vendor SDKs from
`connectedhomeip/third_party/` to keep the image reasonable. If a future chip
bump adds a dependency on a dropped dir, either:

- edit `CHIP_PRUNE_DEFAULT` in `build-rpiz-builder-image.sh` to remove the
  offending entry, or
- set `RHYTHM_CHIP_PRUNE_EXTRA="dir1 dir2"` to add *more* exclusions for a
  one-off build.
