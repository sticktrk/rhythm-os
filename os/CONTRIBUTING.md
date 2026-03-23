# Contributing to Rhythm OS

Thanks for your interest in contributing! This guide covers what you need to get started.

## Prerequisites

- [Rust](https://rustup.rs/) stable toolchain (edition 2021)
- For ESP32 development: ESP-IDF v5.5.2 + ESP-RS toolchain (see [CLAUDE.md](CLAUDE.md#esp32-development))

## Building

```bash
cargo build -p rhythm-server          # Build the macOS/Linux server
cargo build -p rhythm-addon           # Build the HA add-on binary
./scripts/build-esp32.sh --flash      # Build + flash ESP32 firmware
```

## Testing

```bash
cargo test                            # Run all workspace tests
cargo test -p rhythm-core solar       # Filter by crate + test name
```

## Code style

All PRs must pass these checks:

```bash
cargo fmt --check                     # Formatting
cargo clippy --all-targets            # Linting (warnings are errors in CI)
```

Run `cargo fmt` to auto-fix formatting before committing.

## Making changes

1. Fork the repository and create a branch from `main`
2. Make your changes
3. Add tests if applicable
4. Ensure `cargo test`, `cargo clippy --all-targets`, and `cargo fmt --check` all pass
5. Submit a pull request

### Commit messages

- Use the imperative mood ("Add feature" not "Added feature")
- Keep the first line under 72 characters
- Reference issues with `#123` when applicable

## Project structure

The codebase is a Cargo workspace with layered crates:

- **`rust/core/`** — Foundation libraries (algorithms, traits, business logic)
- **`rust/integrations/`** — Lighting platform integrations (Hue, Home Assistant)
- **`rust/bins/`** — Deployable binaries (server, addon, ESP32)

See [CLAUDE.md](CLAUDE.md) for detailed architecture documentation.

## Adding a new integration

To add support for a new lighting platform (LIFX, Nanoleaf, etc.), create a new crate implementing the integration contract: `LightController`, `HubRegistry`, `HubProvider`, and event translation.

See [INTEGRATIONS.md](INTEGRATIONS.md) for the full specification and a crate template.

## Reporting bugs

Use [GitHub Issues](https://github.com/sticktrk/rhythm-os/issues) and include:

- Which platform you're using (server, addon, ESP32)
- Version number
- Steps to reproduce
- Relevant logs (`RUST_LOG=debug` for verbose output)

## License

By contributing, you agree that your contributions will be licensed under the [Apache License 2.0](LICENSE.md).
