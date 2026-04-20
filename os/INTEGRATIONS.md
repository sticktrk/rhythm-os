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
|  rhythm-server, rhythm-linux-appliance, rhythm-addon |
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
rhythm-server         ---> rhythm-os + rhythm-hue + ...
rhythm-linux-appliance ---> rhythm-server + rhythm-os + ...
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
| `Storage` | `rhythm-os/src/storage.rs` | Persistence trait (filesystem today, other backends possible) |
| `AppState` | `rhythm-os/src/state.rs` | Shared state with `hubs: HashMap<HubKey, ActiveHub>` and per-hub credentials |

## How rhythm-hue Implements This

rhythm-hue is the reference integration. Its structure demonstrates the pattern:

```
rhythm-hue/
  src/
    lib.rs              -- module structure, re-exports
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
    reqwest_lifecycle.rs   -- Server/add-on/appliance lifecycle
    reqwest_transport.rs   -- reqwest-based HueTransport impl
    reqwest_sse.rs         -- reqwest-based SSE reader
```

The reqwest transport stack is built in unconditionally for the active
server/add-on/appliance targets.

## Wiring an Integration into a Binary

Each active binary crate provides either a dispatch table or a static integration
registry mapping `HubType` to the correct provider/lifecycle:

```rust
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
| rhythm-server | `rhythm-hue/src/reqwest_transport.rs` | `rhythm-hue/src/reqwest_sse.rs` |
| rhythm-linux-appliance | Reuses `rhythm-server`'s reqwest transport stack | Reuses `rhythm-server`'s SSE stack |

The integration crate owns the lifecycle logic; the binary only provides the transport factory.

## Integration Crate Template

For creating a new integration crate:

```
rhythm-{name}/
  Cargo.toml
  src/
    lib.rs              -- module structure, re-exports
    controller.rs       -- LightController impl
    registry.rs         -- HubRegistry + DeviceRegistry impl
    transport.rs        -- communication trait (platform-abstracted)
    provider.rs         -- configure_{name}_hub() shared validation + init
    lifecycle.rs        -- generic lifecycle helpers shared across platforms
    events.rs           -- native events -> HubEvent translation
    buttons.rs          -- native buttons -> ButtonAction mapping (if applicable)
    hub_state.rs        -- integration data stored in ActiveHub::hub_data
    desktop_lifecycle.rs    -- optional first-party server-class wrapper
```

For active targets, reqwest- or daemon-backed runtime wrappers are compiled in
unconditionally. New integrations should follow the same server-class shape
instead of introducing parallel feature-gated runtime stacks.

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
- `ZigbeeCoordinator` trait -- platform provides impl (USB dongle, appliance sidecar, future hardware bridge)
- Device profiles: ZLL light, ZHA switch, IAS motion sensor
- Commission/join logic, event normalization

### rhythm-ble
- `BlePeripheral` trait -- platform provides impl
- GATT characteristic read/write
- Used by integrations controlling lights via BLE (some Govee, some IKEA)

### WiFi/HTTP
No crate needed -- just std networking (`reqwest` on the active server/appliance paths). Already handled directly in the integration crates.

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
| `rust/bins/rhythm-server/src/hub.rs` | Provider dispatch table (server) |
| `rust/bins/rhythm-linux-appliance/src/main.rs` | Appliance runtime wiring on top of the server stack |
