# CLAUDE.md — rhythm-app (Flutter)

## Overview

This is the Apache-2.0 Flutter app for Rhythm Lighting inside the public Rhythm OS monorepo.

## Project Structure

```
app/
└── flutter/
    ├── rhythm_app/            # Main Flutter app
    │   └── rust/              # FFI crate (Cargokit + FRB bindings)
    └── rhythm_core/           # Shared Dart package (FRB bindings)

tools/app/
├── scripts/                   # Build scripts
├── supabase/                  # Backend config and migrations
└── rhythm_harness.py          # Harness utility
```

## Development Commands

```bash
# Flutter
cd app/flutter/rhythm_app
flutter pub get && flutter build web --release

# WASM (for web builds)
./tools/app/scripts/build-wasm.sh

# Flutter web (includes WASM)
./tools/app/scripts/build-flutter-web.sh

# Mobile
./tools/app/scripts/build-mobile.sh --ios          # iOS
./tools/app/scripts/build-mobile.sh --android      # Android
./tools/app/scripts/build-mobile.sh --codegen --clean  # Regenerate FRB bindings
```

## Flutter Architecture

### rhythm_core (shared package)

- FRB-generated Dart bindings to Rust (`lib/src/rust/`)
- `NativeBrain` — platform-aware FFI wrapper (native_brain_io.dart / native_brain_web.dart)
- `RhythmApi` — abstract interface (getConfigState, saveConfig, getCurveData, getStepSequences)
- Models: `Home`, `Hub` (HubType: homeAssistant/hue/server), `AppSettings` — all Hive-persisted
- Light providers: `HaProvider`, `HaWebSocketProvider`, `HueProvider`, `HubDiscovery`
- `RhythmRunner` + `ProviderManager` for standalone adaptive lighting
- Hue: `HueSseSource`, `HueDeviceRegistry`

### rhythm_app (main app)

**State management**: Provider-based (`MultiProvider` at root)

| Provider | Role |
|---|---|
| `ConfigModel` | Global curve config for designer |
| `HomeProvider` | Home + Hub CRUD, cloud sync, location resolution |
| `HubConnectionProvider` | HA WebSocket + Hue connection health (proxied from HomeProvider) |
| `RoomProvider` | Room list, Rust runner state (all mutations via FRB: `runnerAddRoom`, `runnerHandleAction`, `runnerTick`) |
| `ServerSyncProvider` | Bridges the SDK's RhythmConnection <-> RoomProvider <-> HomeProvider, server-driven room sync |

**Key classes**:
- `HybridApiClient` — composes `AddonApiClient` (remote REST) + `NativeBrain` (local Rust), falls back to local-only
- `RhythmConnection` (from `rhythm_sdk`, in `sdk/`) — dual-mode realtime client (SSE `/api/events` + polling fallback); action dispatches return immediate state, applied via `ServerSyncProvider._onRhythmState`
- `PlatformCapabilities` — feature availability per platform (accounts, hub pairing, location, cloud backend)
- `HomeRepository` — unified offline-first API for Home/Hub CRUD
- `SettingsService` — singleton, Hive-backed, stores AppSettings + RunnerStateDto + curve config
- `SyncService` — offline-first sync between LocalDataSource (Hive) and RemoteDataSource (Supabase)
- `HueServiceLocator` — singleton returning Real or Demo HueBridgeService

**Backend**: Supabase (auth + database + realtime), optional — app works fully offline. Demo mode with `demo@example.invalid`.

**Key screens**: `AppShell` (main), `AllRoomsScreen` (room grid), `DesignerScreen` (curve tuner), `SettingsScreen`, `HueConfiguratorScreen`, `HaConfiguratorScreen`, `BleProvisioningScreen` (BLE Wi-Fi provisioning for Rhythm bridges), `RhythmServerSettingsScreen`, onboarding flow.

### FRB / WASM integration

- Config: `flutter/rhythm_app/flutter_rust_bridge.yaml` — rust_input from `flutter/rhythm_app/rust/`, dart_output to `rhythm_core/lib/src/rust/`
- FFI crate: `flutter/rhythm_app/rust/` (named `rhythm_core_ffi`, standalone workspace for Cargokit)
- Generated Dart stem: `rust_lib_rhythm_app`
- **Do NOT run** `flutter_rust_bridge_codegen integrate` — overwrites main.dart
- After Rust API changes: `./tools/app/scripts/build-mobile.sh --codegen --clean`
- Troubleshooting: "Content hash mismatch" -> `--codegen --clean`; "Failed to load dynamic library" -> check stem; "believes it's in a workspace" -> both Cargo.toml need `[workspace]`

## Mobile Development

### First-Time iOS Setup
```bash
brew install cocoapods
./tools/app/scripts/build-mobile.sh --ios
# On iPhone: Settings -> General -> VPN & Device Management -> Trust developer
```

### After Changing Rust API
```bash
./tools/app/scripts/build-mobile.sh --codegen --clean  # Regenerate FRB bindings
./tools/app/scripts/build-wasm.sh                       # For web builds
```

## Submodule Workflow

```bash
# After Rust API changes
./tools/app/scripts/build-mobile.sh --codegen --clean
./tools/app/scripts/build-wasm.sh
```
