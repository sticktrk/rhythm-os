# Rhythm OS

Rhythm OS is an adaptive lighting system with a Rust server, Flutter app,
Dart SDK and an optional Supabase backend. This repository preserves a privacy-filtered development
history. Private marketing systems, customer investigation material,
production environment files and internal operational automation are excluded.

## Source layout

| Directory | Purpose |
| --- | --- |
| `os/` | Server, device integrations and appliance packaging |
| `app/` | Flutter mobile, desktop and web app |
| `sdk/` | Dart client SDK |
| `runtime/` | Rust runtime API and modules |
| `protocol/` | Shared protocol documentation and contracts |
| `tools/app/supabase/` | Product database schema and Edge Functions |
| `admin-api/`, `admin-ui/` | Optional staff administration source; requires your own authorized backend |
| `lab/` | Hardware test tooling and synthetic fixtures |
| `docs/` | Architecture and backend documentation |

## Development

Read the [server guide](os/README.md), [app build helpers](tools/app/scripts/README.md)
and [Supabase guide](docs/supabase.md) for platform prerequisites and setup.
The Rust workspace is rooted here:

```bash
cargo build -p rhythm-server
```

Flutter dependencies resolve to the SDK in this checkout:

```bash
cd app/flutter/rhythm_app
flutter pub get
flutter analyze
```

Configure your own Supabase project using the provided `.env.example` files.
Keep live `.env` files outside version control. The simulated app demo uses
`demo@example.invalid` / `demo`; it creates local demo state, not a cloud login.
Apple signing identities are explicit local build settings.
For web builds, run `./tools/app/scripts/build-wasm.sh` to generate the excluded
WebAssembly package, or use the full Flutter web build helper.
See [public history](docs/public-history.md) for filtering and version-tag details.

Run the portable repository checks from the root:

```bash
./tools/check-repo-invariants.sh
```

## Contributions and security

See [CONTRIBUTING.md](CONTRIBUTING.md). Public issues must not contain raw
customer logs, private support bundles, credentials or customer identifiers.
Use private vulnerability reporting for sensitive security findings.

## License

First-party code is licensed under [Apache-2.0](LICENSE). Third-party components
retain their own licenses and attribution notices. See [NOTICE](NOTICE).
