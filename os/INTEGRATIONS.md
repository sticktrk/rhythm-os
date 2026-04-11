# Integration System Architecture

How Rhythm OS supports multiple lighting product ecosystems (Hue, LIFX, IKEA, WLED, etc.) across different communication protocols and device types.

## Terminology

| Term | Level | Meaning | Example |
|------|-------|---------|---------|
| **Integration** | Product | A crate for a lighting product ecosystem | `rhythm-hue`, `rhythm-lifx` |
| **Protocol** | Transport | A shared crate for a communication method | `rhythm-zigbee`, `rhythm-ble` |
| **Provider** | Trait | rhythm-os interface for configuring an integration | `HubProvider` trait |
| **Transport** | Trait | Communication abstraction within an integration | `HueTransport` trait |
| **Controller** | Trait impl | `LightController` implementation that sends commands | `HueLightController` |
| **Registry** | Trait impl | Device/room mapping state | `HueDeviceRegistry` |

**"Integration"** is the user/contributor-facing term. Every device has a brand -- no generic catch-all integrations.

## Architecture Layers

```
+---------------------------------------------------+
|  Binary targets (platform I/O)                    |
|  rhythm-esp32, rhythm-server, rhythm-addon        |
+---------------------------------------------------+
|  OS / Business logic                              |
|  rhythm-os (state, commands, event loop)          |
+---------------------------------------------------+
|  Integrations (product-specific logic)            |
|  rhythm-hue   rhythm-lifx   rhythm-ikea   ...    |
+---------------------------------------------------+
|  Protocols (shared transport abstraction)          |
|  rhythm-zigbee   rhythm-ble   rhythm-thread       |
|  (WiFi/HTTP = just std networking, no crate)      |
+---------------------------------------------------+
|  Core algorithms                                  |
|  rhythm-core (engine, traits, curves, solar)      |
+---------------------------------------------------+
```

### Dependency Flow

```
rhythm-core (base, no rhythm-* deps)
    |
rhythm-zigbee ---> rhythm-core (ZCL abstractions, coordinator trait)
rhythm-ble    ---> rhythm-core (BLE characteristics, peripheral trait)
    |
rhythm-hue  ---> rhythm-core + rhythm-zigbee[optional]
rhythm-lifx ---> rhythm-core
rhythm-ikea ---> rhythm-core + rhythm-zigbee
    |
rhythm-os ---> rhythm-core (+ integration crates via features)
    |
rhythm-esp32  ---> rhythm-os + rhythm-hue + rhythm-zigbee + ...
rhythm-server ---> rhythm-os + rhythm-hue + ...
```

## Integration Contract

Every integration crate must provide these pieces.

### Required

#### 1. `LightController` impl

Send lighting commands to devices. Room-level interface: `turn_on(room_id, command)`, `turn_off(room_id)`.

- Hub integrations: single API call (Hue `grouped_light`, Z2M group publish)
- Direct-device integrations: controller iterates `devices_for_room()` internally

**Reference:** `rhythm-hue/src/controller.rs` -- `HueLightController<H: HueTransport>`

#### 2. `HubRegistry` impl

Device/room mapping + persistence. Tracks which devices belong to which rooms, which sensors (buttons, motion) are in each room, and serializes state via `snapshot_json()`.

**Reference:** `rhythm-hue/src/registry.rs` -- `HueDeviceRegistry`

#### 3. `HubProvider` impl

Configuration entry point called by rhythm-os commands without knowing integration specifics:

```rust
fn configure(&self, address: &str, credentials_json: &str, state: &SharedState) -> Result<()>;
```

**Reference:** `rhythm-hue/src/provider.rs` -- `configure_hue_hub()`

#### 4. Event translation

Native events -> `HubEvent`:

```
Hue SSE button event  -> rhythm-hue translates  -> HubEvent::Button
Zigbee ZCL button     -> rhythm-zigbee normalizes -> integration translates -> HubEvent::Button
Zigbee IAS Zone       -> rhythm-zigbee normalizes -> integration translates -> HubEvent::Motion
```

The `event_loop.rs` in rhythm-os processes `HubEvent` generically.

**Reference:** `rhythm-hue/src/hue_lifecycle.rs` -- `translate_sse_event()`

### Optional

5. **`HubDiscovery` impl** -- Network discovery + pairing UI support
6. **Multiple transport modes** -- e.g., rhythm-hue has bridge mode (WiFi) + future direct mode (Zigbee)

## Core Traits (rhythm-core)

These traits define the contract that integration crates implement:

| Trait | File | Purpose |
|-------|------|---------|
| `LightController` | `rhythm-core/src/controller.rs` | Send lighting commands |
| `HubRegistry` | `rhythm-core/src/runtime/hub_registry.rs` | Room/device/sensor mapping for commands |
| `DeviceRegistry` | `rhythm-core/src/runtime/registry.rs` | Device-to-room routing for runtime |
| `RuntimeHandle` | `rhythm-core/src/runtime/handle.rs` | Type-erased runtime interface |

## Hub Abstraction (rhythm-os)

The hub layer in rhythm-os provides the glue between integration crates and the platform:

| Type | File | Purpose |
|------|------|---------|
| `HubEvent` | `rhythm-os/src/hub.rs` | Normalized events (Button, Motion, Heartbeat, Disconnected) |
| `HubType` | `rhythm-os/src/hub.rs` | String newtype for dispatch (`"hue"`, `"lifx"`, etc.) |
| `HubCredentials` | `rhythm-os/src/hub.rs` | Generic credential storage (hub_type + address + JSON data) |
| `ActiveHub` | `rhythm-os/src/hub.rs` | Running hub with type-erased runtime + registry |
| `HubProvider` | `rhythm-os/src/hub.rs` | Trait for hub configuration |
| `Storage` | `rhythm-os/src/storage.rs` | Persistence trait (NVS, filesystem, etc.) |
| `AppState` | `rhythm-os/src/state.rs` | Shared state with `hubs: HashMap<HubKey, ActiveHub>` and per-hub credentials |

## How rhythm-hue Implements This

rhythm-hue is the reference integration. Its structure demonstrates the pattern:

```
rhythm-hue/
  src/
    lib.rs              -- module structure, re-exports, feature gates
    controller.rs       -- HueLightController<H: HueTransport> implements LightController
    registry.rs         -- HueDeviceRegistry implements HubRegistry + DeviceRegistry
    transport.rs        -- HueTransport trait (platform-abstracted HTTP/TLS)
    provider.rs         -- configure_hue_hub() shared validation + init
    hue_lifecycle.rs    -- connect/disconnect + ensure_runtime (generic over transport)
    hub_state.rs        -- HueHubData stored in ActiveHub::hub_data
    sse.rs              -- HueSseEvent parsing
    buttons.rs          -- Hue button events -> ButtonAction mapping
    behavior.rs         -- HueBehaviorTracker for multi-button sequences
    api_types.rs        -- Hue V2 API type definitions
    device_types.rs     -- HueRoom, HueButton, HueSwitchDevice
    embedded_lifecycle.rs  -- ESP32-specific lifecycle (feature = "embedded")
    reqwest_lifecycle.rs   -- Desktop/server lifecycle (feature = "desktop")
    reqwest_transport.rs   -- reqwest-based HueTransport impl
    reqwest_sse.rs         -- reqwest-based SSE reader
```

Feature flags:
```toml
[features]
default = []
blocking = ["rhythm-core/blocking", "rhythm-os/blocking"]
embedded = ["blocking"]
desktop = ["blocking", "dep:reqwest", "dep:reqwest-eventsource", "dep:tokio", "dep:futures"]
```

## Wiring an Integration into a Binary

Each binary crate provides a dispatch table mapping `HubType` to `HubProvider`:

```rust
// rhythm-esp32/src/hub.rs
pub fn get_hub_provider(hub_type: HubType) -> &'static dyn HubProvider {
    static HUE: crate::platform::hue::HueHubProvider = crate::platform::hue::HueHubProvider;
    match hub_type.as_str() {
        HubType::HUE => &HUE,
        _ => &HUE,
    }
}

// rhythm-server/src/hub.rs
pub static INTEGRATIONS: &[&dyn ExternalLightHubIntegration] = &[
    &rhythm_hue::reqwest_lifecycle::INTEGRATION,
    &rhythm_ha::reqwest_lifecycle::INTEGRATION,
    &rhythm_matter::desktop_lifecycle::INTEGRATION,
];
```

The integration registry callbacks on `AppState` connect these to the command layer:

```rust
let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
s.register_controller_fn = Some(callbacks.register_controller_fn);
```

## Platform-Specific Transport

Each binary provides the concrete transport implementation:

| Binary | Transport | SSE |
|--------|-----------|-----|
| rhythm-esp32 | `platform/hue/client.rs` (EspTls HTTP) | `platform/hue/sse.rs` (EspTls SSE) |
| rhythm-server | `rhythm-hue/src/reqwest_transport.rs` | `rhythm-hue/src/reqwest_sse.rs` |

The integration crate owns the lifecycle logic; the binary only provides the transport factory.

## Integration Crate Template

For creating a new integration crate:

```
rhythm-{name}/
  Cargo.toml
  src/
    lib.rs              -- module structure, re-exports, feature gates
    controller.rs       -- LightController impl
    registry.rs         -- HubRegistry + DeviceRegistry impl
    transport.rs        -- communication trait (platform-abstracted)
    provider.rs         -- configure_{name}_hub() shared validation + init
    lifecycle.rs        -- generic lifecycle helpers shared across platforms
    events.rs           -- native events -> HubEvent translation
    buttons.rs          -- native buttons -> ButtonAction mapping (if applicable)
    hub_state.rs        -- integration data stored in ActiveHub::hub_data
    embedded_lifecycle.rs   -- optional first-party embedded wrapper (feature = "embedded")
    desktop_lifecycle.rs    -- optional first-party desktop wrapper (feature = "desktop")
```

Feature flag pattern:
```toml
[features]
default = []
blocking = ["rhythm-core/blocking", "rhythm-os/blocking"]
embedded = ["blocking"]
desktop = ["blocking", "dep:reqwest"]
```

The `embedded` feature means the crate remains embeddable and can be linked into an embedded platform crate. Some integrations also provide a first-party `embedded_lifecycle.rs`; others only expose the generic lifecycle plus transport traits until an embedded transport exists.

## Minimal Integration Checklist

Everything a new integration crate must provide, with helpers that eliminate boilerplate.

### Credentials (2 functions)

Define in `provider.rs`. Use `HubCredentials::new()` and `HubCredentials::get_str()` -- rhythm-os stays hub-agnostic.

```rust
use rhythm_os::hub::HubCredentials;

pub fn xx_credentials(address: &str, secret: &str) -> HubCredentials {
    HubCredentials::new("xx", address, serde_json::json!({ "api_key": secret }))
}

pub fn xx_secret(creds: &HubCredentials) -> Option<&str> {
    creds.get_str("api_key")
}
```

### LightController (5 async methods + `name`)

Use `rhythm_os::controller_helpers` to eliminate registry boilerplate:

| Method | Helper |
|--------|--------|
| `get_rooms()` | `rooms_from_registry(&self.registry)` -- one-liner |
| `turn_on(room_id, command)` | `resolve_room_target(&self.registry, room_id)?` for room lookup |
| `turn_off(room_id)` | `resolve_room_target(&self.registry, room_id)?` for room lookup |
| `any_lights_on(room_id)` | `resolve_room_target(&self.registry, room_id)?` or hub-specific |
| `is_connected()` | Delegate to transport -- one-liner |
| `name()` | Return static string -- one-liner |

### Device Registry

Use `rhythm_os::registry::HubDeviceRegistry` directly -- no need to create a `registry.rs` type alias file. Both rhythm-hue and rhythm-ha alias it for historical reasons, but new crates should import it directly.

### Lifecycle (3 thin wrappers)

| Function | Wraps |
|----------|-------|
| `connect_xx(state, ...) -> (ActiveHub, Receiver<HubEvent>)` | `rhythm_os::lifecycle::connect_hub()` |
| `ensure_xx_runtime(state, transport)` | `rhythm_os::lifecycle::ensure_hub_runtime()` |
| `configure_xx_hub(address, creds_json, state, connect_fn)` | `rhythm_os::lifecycle::configure_hub()` |

### Events (1-2 functions)

| Function | Purpose |
|----------|---------|
| `translate_event(raw) -> Vec<HubEvent>` | Convert native events to hub-agnostic events |
| Event stream setup | Start reading from SSE/WebSocket/polling |

### Hub data struct

```rust
pub struct XxHubData {
    pub registry: Arc<Mutex<HubDeviceRegistry>>,
    // ... hub-specific connection state
}
```

### Transport trait

Hub-specific communication abstraction. Transport traits differ fundamentally between hubs -- no unified trait.

### Discovery (optional)

Implement `rhythm_os::discovery::HubDiscovery` for server-driven room sync: `discover_rooms()`, `discover_devices()`, `discover_sensors()`.

## Protocol Crates (Future)

Shared protocol crates abstract radio/transport access for multiple integrations.

### rhythm-zigbee
- ZCL cluster abstractions: On/Off, Level Control, Color Control, IAS Zone
- `ZigbeeCoordinator` trait -- platform provides impl (ESP32 radio, USB dongle)
- Device profiles: ZLL light, ZHA switch, IAS motion sensor
- Commission/join logic, event normalization

### rhythm-ble
- `BlePeripheral` trait -- platform provides impl
- GATT characteristic read/write
- Used by integrations controlling lights via BLE (some Govee, some IKEA)

### WiFi/HTTP
No crate needed -- just std networking (`reqwest` on desktop, `esp-idf-svc` on ESP32). Already handled by feature flags in integration crates.

## Multi-Integration Support

### Current: Multiple Active Hubs
`AppState` already supports multiple active hubs, per-hub credentials, and composite controller registration. That is the current production architecture.

### Future: Richer Integration Orchestration
When a user has Hue bridge for some rooms + IKEA Zigbee for others + local Matter:

```rust
pub struct IntegrationManager {
    active: HashMap<String, ActiveHub>,           // "hue" -> ActiveHub, "ikea" -> ActiveHub
    credentials: HashMap<String, HubCredentials>, // stored per integration
}
```

A `CompositeController` wraps multiple controllers, routing topology room IDs to the correct hub-native targets.

Future work, if needed, is around richer orchestration policy and tooling, not replacing a single-hub state model.

## Sensors in the Architecture

Sensors (switches, motion) flow through the existing `HubEvent` system. The event loop processes them generically.

Future sensor types extend `HubEvent`:
```rust
pub enum HubEvent {
    Button { room_id, action, device_id },
    Motion { room_id, sensor_id, detected },
    LightLevel { room_id, sensor_id, lux: f32 },  // future
    Heartbeat,
    Disconnected(String),
}
```

## Phased Roadmap

### Phase 1: Document & Codify (current)
- This document captures the architecture
- No code changes to traits or existing crates
- rhythm-hue and rhythm-ha serve as the primary reference implementations

### Phase 2: First Protocol Crate
When Zigbee work starts: create `rhythm-zigbee` with coordinator trait + ZCL basics.

### Phase 3: Second Integration
Build next integration (IKEA? LIFX?) using the template. Validates the contract works for non-Hue. This is where Hue-specific names (like `grouped_light_id`) get generalized.

### Phase 4: Multi-Integration Orchestration
Refine the existing multi-hub model only if a future platform needs additional orchestration layers beyond today's `AppState.hubs` + `CompositeController`.

## Key Files Reference

| File | Role |
|------|------|
| `rust/core/rhythm-core/src/controller.rs` | `LightController` trait |
| `rust/core/rhythm-core/src/runtime/hub_registry.rs` | `HubRegistry` trait |
| `rust/core/rhythm-core/src/runtime/registry.rs` | `DeviceRegistry` trait |
| `rust/core/rhythm-core/src/runtime/handle.rs` | `RuntimeHandle` type erasure |
| `rust/core/rhythm-os/src/hub.rs` | `HubProvider`, `ActiveHub`, `HubEvent`, `HubType`, `HubCredentials` |
| `rust/core/rhythm-os/src/controller_helpers.rs` | `rooms_from_registry()`, `resolve_room_target()` |
| `rust/core/rhythm-os/src/state.rs` | `AppState` with `hubs`, per-hub credentials, and platform metadata |
| `rust/core/rhythm-os/src/commands.rs` | Hub-agnostic command handlers |
| `rust/core/rhythm-os/src/event_loop.rs` | Generic `HubEvent` processing |
| `rust/core/rhythm-os/src/storage.rs` | `Storage` persistence trait |
| `rust/integrations/rhythm-hue/` | Reference integration implementation |
| `rust/bins/rhythm-esp32/src/hub.rs` | Provider dispatch table (ESP32) |
| `rust/bins/rhythm-esp32/src/platform/hue/` | Platform-specific transport (ESP32) |
| `rust/bins/rhythm-server/src/hub.rs` | Provider dispatch table (server) |
