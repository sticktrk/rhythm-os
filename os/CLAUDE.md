# CLAUDE.md

## Overview

Rhythm OS is adaptive lighting that follows the sun. This is the open-source Rust backend — a headless server with a complete REST API.

1. **HA Add-on** (`rust/bins/rhythm-addon/`) — Rust binary connecting to Home Assistant via WebSocket
2. **Server** (`rust/bins/rhythm-server/`) — macOS/Linux CLI server with REST API + mDNS
3. **Linux Appliance** (`rust/bins/rhythm-linux-appliance/`) — `rpiz` appliance runtime layered on the native server stack

## Development Commands

```bash
# Rust workspace
cargo build -p rhythm-addon              # Build addon
cargo test                               # Test all workspace crates
cargo test -p rhythm-core solar          # Filter by crate + test name
cargo test -p rhythm-core -- --nocapture # Show println output
cargo clippy --all-targets               # Lint
RUST_LOG=debug cargo run -p rhythm-addon # Run addon locally (needs .env)

# Build scripts
./tools/os/scripts/build-rust.sh --release        # Addon binary (debug without --release)
./tools/os/scripts/build-server.sh --release       # Server binary (macOS/Linux)
./tools/os/scripts/build-server.sh --release --target rpiz # Appliance binary for Pi Zero / Zero W
./tools/os/scripts/build-addon.sh                 # Docker image
./tools/os/scripts/run-dev.sh                     # Build + run addon locally
```

## Rust Crate Architecture

### Directory layout

```
rust/
  core/           # Foundation libraries (algorithms, traits, business logic)
    rhythm-core/
    rhythm-profile/
    rhythm-devices/
    rhythm-os/
  integrations/   # Light hub/platform integrations
    rhythm-hue/
    rhythm-ha/
    rhythm-matter/
  bins/           # Deployable binaries
    rhythm-addon/
    rhythm-server/
    rhythm-chipd/
    rhythm-linux-appliance/
```

### Workspace members

**rhythm-core** — Pure algorithms + runtime orchestration (no I/O)
- `RhythmEngine<C: LightController>`, `RhythmRuntime<C,T,S,R>`, `RuntimeHandle` (type-erased)
- Traits: `LightController`, `LightProfileModule`, `PersistenceProvider`, `HubRegistry`, `TimeProvider`, `Scheduler`, `DeviceRegistry`, `RoomStateStore`, `GroupController`
- `ButtonAction` enum (OnPress, OffPress, Toggle, UpPress, DownPress, UpHold, DownHold, Stop, RhythmOn, RhythmOff, LightsOff, SleepOn, SleepOff, Reset), `InputEvent`, `RoomSnapshot`
- `GroupController` trait, `LightGroup`, `NoOpGroupController` — group-based light control (ZHA groups, ZigBee group addressing)
- `CommonCurveConfig`, `SolarContext`, `LightingValues`, `LightingCommand`, `RoomManager`
- Solar calculations (`SolarTime`, `SunTimes`, `TwilightTimes`), color conversions (Kelvin/RGB/XY/mireds)
- Feature flags: `serde` (default), `test-support`

**rhythm-profile** — Pluggable lighting profile contract (no rhythm-* deps)
- `LightProfileModule` trait, `LightProfileConfig`, `TimerSetting`, `CurveContext`
- `LightCurveShape`, `LightingValues`, color/render/solar helpers — re-exported through rhythm-core

**rhythm-devices** — Device capability database
- `DeviceDatabase` (`builtin_db()`) — known light models with capabilities (color modes, kelvin range, gamut) and protocol quirks (Zigbee endpoints, Hue API workarounds, Matter data)
- `adapt_command()` — adapts a `LightingCommand` to a device's capabilities

**rhythm-hue** — Platform-agnostic Hue V2 integration (reference integration, see [INTEGRATIONS.md](INTEGRATIONS.md))
- `HueLightController<H: HueTransport>` implements `LightController`
- `HueTransport` trait — abstract HTTP/TLS transport (platform provides impl)
- `HueDeviceRegistry` — type alias for `rhythm_os::registry::HubDeviceRegistry`
- `HueBehaviorTracker`, SSE event parsing
- Button mapping re-exported from `rhythm_os::hue_buttons`
- Reqwest-based lifecycle and transport modules are built in unconditionally for the active server/add-on/appliance targets

**rhythm-os** — Platform-agnostic business logic
- `state` — `AppState` / `SharedState` — shared application state with type-erased hub
- `storage` — `Storage` trait + `FileStorage` — persistence for rooms, config, location, settings, hub credentials, registry
- `hub` — `HubType` (string newtype), `HubCredentials`, `ActiveHub` (with `dyn RuntimeHandle` + `dyn HubRegistry`), `HubProvider` trait, `HubEvent` enum
- `commands` — business logic for all mutations (hub-agnostic)
- `handlers` — framework-agnostic HTTP handler functions (`ApiResponse` struct), `routes` — route → handler glue
- `registry` — `HubDeviceRegistry` (button/device/room mapping, implements `HubRegistry` + `DeviceRegistry`)
- `canonical` — canonical device model (cross-hub device identity)
- `topology` — topology rooms/nodes graph, `scenes` — scene storage + preview/apply
- `auth` — API auth (claim/status/settings, `require_api_auth_middleware`), `remote_access` — remote-access config
- `bundle` — profile bundle import/export, `factory_default_config` — factory defaults
- `pairing` — device pairing/unpairing (Matter commissioning, Zigbee permit join), `provisioning` — Wi-Fi/BLE provisioning contract
- `button_resolve` — shared button resolution (lookup → discover → map → emit)
- `hue_buttons` — Hue V2 button → `ButtonAction` mapping (shared by rhythm-hue and rhythm-ha)
- `discovery` — `HubDiscovery` trait, server-side device discovery
- `room_sync` — hub discovery + diff against current state
- `lifecycle` — generic hub connect, runtime creation, event translation
- `event_loop` — hub event → engine action dispatch loop
- `periodic` — periodic tick + room state broadcast
- `mdns` — mDNS constants and service registration, `logging` — tracing setup + HTTP observability
- `axum_router` — shared Axum routes: `api_routes()`, SSE endpoint
- `server_event` — SSE event types: NodeState, MotionTimer, InputEvent, HubStatus, DispatchFailure, SettingsChanged, LightBreakerChanged, ModeChanged, ConfigChanged, NodesChanged, TriageChanged, PairingProgress, OtaUpdateProgress

**rhythm-ha** — Platform-agnostic Home Assistant integration (like rhythm-hue)
- `HaLightController<H: HaTransport>` implements `LightController`
- `HaTransport` trait — abstract HTTP transport (platform provides impl)
- `HaDeviceRegistry` — type alias for `rhythm_os::registry::HubDeviceRegistry`
- `HaConnectionConfig`, WebSocket event parsing, service/ZHA event translation
- Reqwest + WebSocket runtime modules are built in unconditionally for the active server/add-on/appliance targets

**rhythm-matter** — Matter integration (protocol + integration hybrid)
- Controls Matter-over-WiFi/Thread lights; the server acts as Matter commissioner — the Matter fabric IS the "hub"
- One crate for all brands (Matter standardizes clusters); brand identity comes from `CanonicalDevice.manufacturer` + the rhythm-devices database
- `controller`, `commissioning`, `clusters`, `groups`, `fabric`, `discovery`, `bulb_test`, `capture` modules
- Talks to the native `rhythm-chipd` daemon via `chip_rpc`/`chip_transport`

**rhythm-addon** — HA add-on binary (like rhythm-server)
- Uses rhythm-os (AppState, SharedState, commands, event_loop, periodic)
- Uses rhythm-ha, rhythm-hue, and rhythm-matter integrations
- Axum HTTP server
- `FileStorage` — JSON persistence to `/data/`
- Auto-configures from HA Supervisor (token, location)

**rhythm-server** — macOS/Linux CLI server
- Modules: `http_server`, `hub`, `self_update`, `auto_update`, `bootstate`, `debug_bundle`, `liveness`
- Shared `api_routes()` from rhythm-os + server-specific endpoints (`/api/discover`, `/api/ota/*`, `/api/diag/*`, `/api/restart`)
- `clap`-based CLI args (port, data-dir, log-level)
- Axum HTTP server + mDNS advertisement + mDNS discovery
- `FileStorage` — JSON persistence to `~/.rhythm/`
- Uses rhythm-hue, rhythm-ha, and rhythm-matter integrations
- Also builds the `rhythm-cli` secondary binary (`src/cli.rs`)

**rhythm-chipd** — Native Matter controller daemon
- Standalone binary wrapping the CHIP/Matter stack; rhythm-matter connects to it over RPC

**rhythm-linux-appliance** — Linux appliance runtime
- Builds on top of `rhythm-server` plus appliance-only Wi-Fi/BLE provisioning pieces
- Uses the same server-class stack (`rhythm-os`, reqwest-based integrations)
- Ships as the `rpiz` target and the Buildroot appliance image

### Dependency graph

```
rhythm-profile, rhythm-devices (base, no rhythm-* deps)
    |
rhythm-core ──► rhythm-profile
rhythm-os   ──► rhythm-core + rhythm-devices
    |
rhythm-hue    ──► rhythm-core + rhythm-os + rhythm-devices
rhythm-ha     ──► rhythm-core + rhythm-os + rhythm-devices
rhythm-matter ──► rhythm-core + rhythm-os + rhythm-devices
    |
rhythm-addon  ──► rhythm-os + rhythm-ha + rhythm-hue + rhythm-matter
rhythm-server ──► rhythm-os + rhythm-hue + rhythm-ha + rhythm-matter
rhythm-linux-appliance ──► rhythm-server + rhythm-os
rhythm-chipd (standalone Matter daemon, used by rhythm-matter at runtime)
```

For the integration system architecture (adding new lighting integrations), see [INTEGRATIONS.md](INTEGRATIONS.md).

## API Endpoints

### Shared (from `rhythm_os::axum_router`)

#### State & Events
- `GET /health` — health check
- `GET /api/state` — full snapshot (nodes, topology, devices, settings, triage)
- `GET /api/nodes/state` — lightweight node state for polling
- `GET /api/events` — SSE stream (`ServerEvent` variants, see rhythm-os `server_event`)

#### Auth & Remote Access
- `GET /api/auth/status` — auth/claim status
- `POST /api/auth/claim` — claim the device
- `PUT /api/auth/settings` — update auth settings
- `GET /api/remote-access/status` — remote-access status
- `PUT /api/remote-access/config`, `DELETE /api/remote-access/config` — set/clear remote-access config

#### Nodes
- `PUT /api/nodes/action` — dispatch node action(s) (node_id + action)
- `PUT /api/nodes/brightness` — set node brightness
- `PUT /api/nodes/curve` — set node curve
- `PUT /api/nodes/color` — set node color
- `PUT /api/nodes/offset` — set node time offset
- `PUT /api/nodes/preferences` — update node preferences (rhythm_enabled, disabled, soft_off)
- `PUT /api/nodes/profile-overrides` — per-node light profile overrides

#### Devices, Topology & Triage
- `DELETE /api/devices?id=` — remove device
- `POST /api/devices/pair`, `POST /api/devices/unpair` — device pairing/unpairing (Matter commissioning, Zigbee permit join)
- `GET /api/devices/canonical` — list canonical devices
- `GET /api/devices/canonical/:id` — fetch canonical device details
- `PUT /api/devices/canonical/:id/room` — assign or unassign canonical device room
- `PUT /api/devices/canonical/:id/parent` — attach a canonical device to a topology parent
- `PUT /api/devices/canonical/:id/preferred` — choose preferred endpoint for a canonical device
- `POST /api/devices/canonical/:id/flash` — flash a device to identify it
- `GET /api/triage`, `GET /api/triage/count` — list/count pending room/device merge decisions
- `PUT /api/triage/:id/merge|new|dismiss|room|bind` — resolve triage entries
- `GET /api/topology/rooms`, `POST /api/topology/rooms` — list/create topology rooms
- `GET /api/topology/nodes` — list topology nodes
- `PUT /api/topology/rooms/:id`, `DELETE /api/topology/rooms/:id` — rename/delete topology room
- `PUT /api/topology/rooms/:id/merge` — merge topology rooms
- `PUT /api/topology/rooms/:id/devices/move` — move devices between topology rooms
- `PUT /api/topology/nodes/:id/controls/:kind` — set node control binding

#### Config, Settings & Profiles
- `GET /api/config`, `PUT /api/config` — read/push curve config
- `POST /api/config/absorb-offset` — fold time offset into config
- `POST /api/config/reset` — reset curve config
- `PUT /api/location` — set location (lat, lon, utc_offset)
- `GET /api/settings`, `PUT /api/settings` — runtime settings
- `GET /api/light-breaker`, `PUT /api/light-breaker` — global autonomous light-breaker switch
- `GET /api/mode`, `PUT /api/mode` — rhythm mode (day/sleep)
- `GET /api/transitions`, `PUT /api/transitions` — mode transition config
- `POST /api/transitions/:id/trigger` — trigger a mode transition
- `GET /api/profiles` — list light profiles
- `PUT /api/light-profile` — select active light profile
- `GET /api/curve`, `POST /api/curve` — curve visualization / preview
- `GET /api/curve/now`, `GET /api/curve/solar` — current curve values / solar context
- `GET /api/profile-bundle`, `PUT /api/profile-bundle` — export/import profile bundle (aliased as `/api/share-bundle`)
- `GET /api/profile-bundle/factory-default`, `POST /api/profile-bundle/reset` — factory-default bundle / reset
- `GET /api/backup`, `PUT /api/backup` — full backup export/restore
- `POST /api/factory-reset` — factory reset

#### Input Bindings & Scenes
- `GET /api/input-bindings`, `POST /api/input-bindings` — list/create input bindings
- `PUT /api/input-bindings/:id`, `DELETE /api/input-bindings/:id` — update/delete input binding
- `GET /api/scenes`, `POST /api/scenes` — list/create scenes
- `PUT /api/scenes/:id`, `DELETE /api/scenes/:id` — update/delete scene
- `POST /api/scenes/:id/apply`, `POST /api/scenes/:id/preview`, `POST /api/scenes/preview` — apply/preview scenes
- `POST /api/scenes/:id/apply-home` — apply one stored scene to the whole home (server-owned enumeration, palette continuity, paced dispatch); `target_mode: devices` (default: every light individually, one paced lane per hub) or `rooms`
- `POST /api/scene-previews/:id/commit`, `POST /api/scene-previews/:id/cancel` — resolve scene previews

#### Hub & Matter
- `PUT /api/hub/credentials`, `DELETE /api/hub/credentials` — push/clear hub credentials (hub_type, address, credentials)
- `POST /api/hub/retry` — retry hub connection
- `POST /api/sync` — trigger hub re-discovery and room sync
- `GET /api/matter/captures`, `GET /api/matter/captures/:id` — Matter traffic captures
- `POST /api/matter/audition/run`, `POST /api/matter/audition/report` — Bulb Audition scenarios and typed control-profile report (`/api/matter/bulb-test/*` remain one-release aliases)

### Server-only (rhythm-server)
- `GET /api/discover` — scan local network for rhythm devices via mDNS (~3s)
- `GET /api/ota/capabilities` — describe OTA strategy and payload types
- `GET /api/ota/status` — current OTA state and last check/apply result
- `GET /api/ota/check` — check for self-update
- `POST /api/ota/update` — download and apply self-update (restarts process)
- `POST /api/restart` — restart the device (rpiz appliance reboots; desktop server exits and is restarted by systemd/launchd)
- `POST /api/diag/debug-bundle` — collect a diagnostic debug bundle
- `POST /api/diag/reset-matter-fabric` — reset the Matter fabric

### Appliance-only
- `GET /api/wifi` — current appliance Wi-Fi/provisioning status
- `PUT /api/wifi` — set Wi-Fi credentials (applies after short delay, reconnects)
- `DELETE /api/wifi` — clear Wi-Fi credentials, restart networking, and re-enable BLE provisioning

## Key Architectural Patterns

- **Generic trait-based core**: `RhythmEngine<C: LightController>`, `RhythmRuntime<C,T,S,R>` — same algorithms, different backends
- **Type erasure**: `RuntimeHandle` blanket impl + `ActiveHub` with `dyn HubRegistry` for hub-agnostic command handling
- **Config-driven light profiles**: `LightProfileModule` trait, `LightProfileRegistry`, default profiles: `rhythm` (Day), `sleep`, `day_idle`, `sleep_idle`
- **Integration system**: Each lighting product (Hue, Home Assistant, Matter, etc.) is a crate implementing `LightController` + `HubRegistry` + `HubProvider`. See [INTEGRATIONS.md](INTEGRATIONS.md)
- **Hub abstraction**: `HueTransport` trait for platform-decoupled Hue communication, `Storage` trait for persistence, `HubProvider` trait for plug-and-play hub configuration
- **Deferred runtime creation**: `ensure_runtime_fn` callback — runtime created when first room arrives, not at startup
- **Active platform focus**: shared Rust core primarily targets the HA addon, standalone server, and Linux appliance runtime
- **Shared button resolution**: `button_resolve` in rhythm-os standardizes lookup → discover → map → emit, used by both rhythm-hue (SSE) and rhythm-ha (`hue_event`)
- **Non-blocking hub dispatch**: `CompositeController` never blocks callers — `turn_on`/`turn_off` enqueue into per-hub mailboxes with latest-wins coalescing per target; async workers on a dedicated dispatch runtime deliver with per-hub pacing (Hue token bucket), supervised timeouts, per-target cooldowns, and `HubDispatchOutcome` events (failures broadcast as `ServerEvent::DispatchFailure`)
- **SSE real-time updates**: `ServerEvent` broadcast via `GET /api/events`
- **Unified device registry**: `HubDeviceRegistry` in rhythm-os; `HueDeviceRegistry` and `HaDeviceRegistry` are type aliases

## Git Workflow & Deployment

**Version files**: `install/addon/config.yaml`
**Changelog**: `install/addon/CHANGELOG.md`

```bash
# Build and test before committing
./tools/os/scripts/build-rust.sh --release && cargo test

# Deploy
./tools/os/scripts/deploy-addon.sh --push             # Docker Hub + addon repo (auto-bumps version)
./tools/os/scripts/deploy-addon.sh --local --ha-host homeassistant.local
```

**Repos**: Main — `sticktrk/rhythm-os` (this public monorepo), Addon — `sticktrk/rhythm-os-addon`, Docker Hub — `dtconcepts/rhythm-os-addon`

**OTA releases**: see [`docs/ota.md`](../docs/ota.md) — channels (beta/stable), binary vs full-image updates, the rootfs fingerprint gate, and runbooks.
