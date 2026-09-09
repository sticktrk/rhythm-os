# Local development

Clone the public repository and work here using ordinary branches and pull
requests. All source, local product checks and synthetic fixtures are included.

## Run the server without cloud credentials

Install Git, a C/C++ toolchain and the stable Rust toolchain from
[rustup](https://rustup.rs/). On Ubuntu/Debian, install the native dependencies:

```bash
sudo apt-get update
sudo apt-get install -y build-essential pkg-config libdbus-1-dev libudev-dev libssl-dev
```

On macOS, install the Xcode Command Line Tools with `xcode-select --install`.

```bash
git clone https://github.com/sticktrk/rhythm-os.git
cd rhythm-os
git switch -c my-change
cargo build --locked -p rhythm-server
cargo run --locked -p rhythm-server -- --data-dir ./target/dev-state --port 54448
```

The server creates an empty local installation. No Supabase account, signing
identity or lighting hardware is required to start it and read state. In a
second terminal:

```bash
curl --fail http://127.0.0.1:54448/api/nodes/state
```

Stop the foreground server with Ctrl-C. Its development data stays in
`target/dev-state`; your normal `~/.rhythm` installation is separate. The server
listens on the selected port on all interfaces for local network clients.

CI exercises the same startup path using fresh temporary storage and an
environment without cloud credentials:

```bash
python3 tools/ci/smoke-server.py --binary target/debug/rhythm-server
```

## Test and contribute

Start with a focused Rust package test, then run the portable source checks:

```bash
cargo test --locked -p rhythm-core --lib
./tools/check-repo-invariants.sh
```

The source-check command requires Python 3.11+, Bash, Git, ripgrep (`rg`), jq and
Ruby, plus standard Unix utilities. Public CI installs its own dependencies.
It includes the BlueZ ownership, canonical Supabase root, analytics anchor and
shared commissioning guards under `tools/ci/`; those commands are also useful
individually while editing. The BlueZ exceptions are maintained in
`tools/ci/references/bluez-owner-exceptions.tsv` alongside their contract tests.

Push your branch to your fork and open a public PR. Include the intended
behavior, focused tests and any outstanding hardware verification. See
[CONTRIBUTING.md](../CONTRIBUTING.md).

## Optional surfaces

- Flutter: use Flutter **3.44.6**, then run `flutter pub get`, `flutter analyze`
  and `flutter test` from `app/flutter/rhythm_app`. The SDK dependency resolves
  within this checkout. See [app build helpers](../tools/app/scripts/README.md)
  for the additional native and WASM toolchains used to run each platform.
- Cloud backend: follow [the Supabase guide](supabase.md) to start a local
  backend or deploy your own project. Cloud authentication, remote access and
  optional integrations need their documented configuration. Native app account
  onboarding has additional requirements beyond this server quickstart.
- Hardware and Matter: see [server integrations](../os/INTEGRATIONS.md) and
  [packaging tools](../tools/os/scripts/README.md). Native Matter SDK builds and
  real-device compatibility checks are additional to the portable CI suite.
- Signed mobile/store distributions require your own platform accounts,
  signing identities and bundle identifiers.

Product version tags and published downloads are available on the
[Releases page](https://github.com/sticktrk/rhythm-os/releases). Build and upload
helpers accept your own credentials; pushing a source tag alone does not update
an OTA feed without an explicitly configured publisher.
