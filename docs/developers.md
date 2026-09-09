# Developing Rhythm OS

Everything a contributor or integrator needs, in one place. The [root README](../README.md) is the product showcase; this page is the workshop.

## Build on it

Rhythm is a layered set of Rust crates with a clean contract at every seam. Bring your own frontend, your own curve, or your own lighting product.

- **REST + SSE.** Every platform exposes the same API. Full state snapshots, selective reads, and a real-time event stream for room state, motion, hub status, dispatch failures, and pairing progress. See the [API reference](../os/CLAUDE.md#api-endpoints).
- **Pluggable curve engine.** Implement `LightProfileModule` and the whole system runs your shape. The Gaussian solar default is just the one that ships.
- **Add a lighting product in one crate.** Implement `LightController`, `HubRegistry`, `HubProvider`, and event translation. No changes to the core. Contract and template in [os/INTEGRATIONS.md](../os/INTEGRATIONS.md).
- **Dart SDK.** Typed models, SSE with polling fallback, pairing, OTA, diagnostics. Powers the Flutter app and available as [`rhythm_sdk`](../sdk/).
- **Assistant contract.** A machine-readable [contract](architecture/light-assistant-contract.md) and topology endpoint so LLM and automation clients can drive lights without overstepping the user's authority.
- **Flutter app.** iOS, macOS, Android, and web. Runs against a real server or a simulated home. See [app/](../app/).

```
┌─────────────────────────────────────────────────────────┐
│  Binaries        rhythm-server · rhythm-linux-appliance  │
│                  rhythm-addon                             │
├─────────────────────────────────────────────────────────┤
│  OS layer        rhythm-os  (rooms, events, commands)    │
├─────────────────────────────────────────────────────────┤
│  Integrations    rhythm-hue · rhythm-ha · rhythm-matter  │
│  Protocols       rhythm-ble · (rhythm-zigbee, planned)   │
├─────────────────────────────────────────────────────────┤
│  Core            rhythm-core  (solar, curves, color)     │
│                  rhythm-profile · rhythm-devices          │
└─────────────────────────────────────────────────────────┘
```

Read the [server guide](../os/README.md) for the crate-by-crate tour.


## Local development

Start with the [local development quickstart](development.md) to run the server without cloud credentials or hardware. Then read the [server guide](../os/README.md), [app build helpers](../tools/app/scripts/README.md) and [Supabase guide](supabase.md) for platform prerequisites.

The Rust workspace is rooted at the repository root:

```bash
cargo build -p rhythm-server
```

Flutter dependencies resolve to the SDK in this checkout:

```bash
cd app/flutter/rhythm_app
flutter pub get
flutter analyze
```

Configure your own Supabase project using the provided `.env.example` files and keep live `.env` files outside version control. Apple signing identities are explicit local build settings. For web builds, run `./tools/app/scripts/build-wasm.sh` to generate the excluded WebAssembly package, or use the full Flutter web build helper.

README screenshots are captured from the Virtual Experience by an integration test, `app/flutter/rhythm_app/integration_test/readme_screenshots_test.dart`, driven through `test_driver/integration_test.dart`.

## Repository map

| Directory | Purpose |
| --- | --- |
| `os/` | Server, device integrations, and appliance packaging |
| `app/` | Flutter mobile, desktop, and web app |
| `sdk/` | Dart client SDK |
| `runtime/` | Rust runtime API and modules |
| `protocol/` | Shared protocol documentation and contracts |
| `tools/app/supabase/` | Optional product database schema and Edge Functions |
| `admin-api/`, `admin-ui/` | Optional staff administration source; requires your own authorized backend |
| `lab/` | Hardware test tooling and synthetic fixtures |
| `docs/` | Architecture and backend documentation |

This repository preserves a privacy-filtered development history. Private marketing systems, customer investigation material, production environment files, and internal operational automation are excluded. See [public history](public-history.md).

Run the portable repository checks from the root:

```bash
./tools/check-repo-invariants.sh
```


## Contributing and security

- [Discord](https://discord.gg/8DXG3WjA) for help, setups, and feature discussion.
- [GitHub Issues](https://github.com/sticktrk/rhythm-os/issues) for bugs and requests. Please keep raw customer logs, support bundles, credentials, and customer identifiers out of public issues.
- [CONTRIBUTING.md](../CONTRIBUTING.md) for how to submit changes. [SECURITY.md](../SECURITY.md) for private vulnerability reporting.


## License

First-party code is licensed under [Apache-2.0](../LICENSE). Third-party components retain their own licenses and attribution notices. See [NOTICE](../NOTICE).

---

<p align="center">Light is language. Rhythm makes it fluent.</p>
