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

During development, run only the narrowest test that covers the change: one
test, one file, one crate, or one package. Do not run the full Rust workspace,
Flutter, or Dart suites while iterating. Run that corpus once, as a final
check, before opening a pull request. CI runs it on every push and pull
request.

Rust, from the repository root. `<filter>` is a test or module name:

```text
cargo test -p <crate> <filter>
```

```bash
cargo test -p rhythm-core solar
```

The hub's scenario tests share one integration-test binary. To add a scenario,
place its file in `os/rust/core/rhythm-os/tests/scenarios/` and declare the
module in that directory's `main.rs`. To run a single module, pass its name
followed by `::` as the test filter.

```bash
cargo test -p rhythm-os --test scenarios scenario_backup_nodes::
```

Flutter and Dart, one file. `--name <test>` selects a single test inside it:

```text
cd app/flutter/rhythm_app && flutter test test/<file>_test.dart
cd sdk && dart test test/<file>_test.dart
cd admin-api && dart test test/<file>_test.dart
```

`./tools/check-repo-invariants.sh` checks source boundaries, licenses, and
deployment fixtures. The command requires Python 3.11+, Bash, Git, ripgrep
(`rg`), jq and Ruby, plus standard Unix utilities. Public CI installs its own
dependencies. It includes the BlueZ ownership, canonical Supabase root,
analytics anchor and shared commissioning guards under `tools/ci/`; those
commands are also useful individually while editing. The BlueZ exceptions are
maintained in `tools/ci/references/bluez-owner-exceptions.tsv` alongside their
contract tests.

On ARM64 GNU/Linux with 8 GiB of RAM, GNU ld.bfd can run out of memory when a
final workspace run links many test binaries at once. Install `lld` and set
the linker flag below for that final check. A single-crate filter links one
test target.

```bash
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUSTFLAGS='-C link-arg=-fuse-ld=lld'
cargo test --workspace --all-targets --locked
```

`--all-features` on that same command links still more test binaries. Use it
only when the final check needs feature-gated targets. The portable CI Rust
job runs the command above without the linker override.

Push your branch to your fork and open a public PR. Include the intended
behavior, the focused tests you ran, and any outstanding hardware verification.
See [CONTRIBUTING.md](../CONTRIBUTING.md).

## Optional surfaces

- Flutter: use Flutter **3.44.6**, then run `flutter pub get` and
  `flutter analyze` from `app/flutter/rhythm_app`. Test one file with
  `flutter test test/<file>_test.dart`. `flutter test` with no file argument
  is the app-suite final check. The SDK dependency resolves within this
  checkout. See [app build helpers](../tools/app/scripts/README.md) for the
  additional native and WASM toolchains used to run each platform.
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
