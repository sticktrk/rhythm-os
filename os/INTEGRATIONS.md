# Integration System Architecture

How Rhythm OS supports product ecosystems, direct devices, communication
protocols, and normalized device types without turning every manufacturer into
an architectural boundary.

## Terminology

| Term | Level | Meaning | Example |
|------|-------|---------|---------|
| **Integration** | Control plane | An ecosystem or substantial driver boundary | Hue bridge/API in `rhythm-hue` |
| **Protocol runtime** | Transport | Shared ownership of a communication resource | Linux BlueZ access in `rhythm-ble` |
| **Device profile** | Protocol | Identity, matching, codec, and policy for a device family | A versioned BLE button profile |
| **Device driver** | Protocol/product | Stateful behavior that uses a protocol runtime | Hue BLE bulb control in `rhythm-hue` |
| **Provider** | Trait | rhythm-os interface for configuring an integration | `HubProvider` trait |
| **Transport** | Trait | Communication abstraction within an integration | `HueTransport` trait |
| **Controller** | Trait impl | `LightController` implementation that sends commands | `HueLightController` |
| **Registry** | Trait impl | Device/room mapping state | `HueDeviceRegistry` |

**"Integration"** is the user/contributor-facing term for a distinct control
plane, not a synonym for manufacturer. A bridge API, cloud account, standard
commissioner, or substantial stateful protocol can justify an integration
crate. A device that only contributes another BLE identity or frame grammar
normally adds a profile to the shared local-BLE integration. Manufacturer and
model remain device metadata; they do not have to become a hub type or crate.

This distinction is central to Rhythm's normalization goal: users care that a
button or bulb works, while the implementation keeps only the protocol and
lifecycle boundaries that are operationally real.

## Architecture Layers

```
+---------------------------------------------------+
|  Binary targets (platform I/O)                    |
|  rhythm-server, rhythm-linux-appliance, rhythm-addon |
+---------------------------------------------------+
|  OS / Business logic                              |
|  rhythm-os (state, commands, event loop)          |
+---------------------------------------------------+
|  Integrations and substantial device drivers      |
|  rhythm-hue   rhythm-ha   rhythm-matter   ...    |
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
rhythm-core (base, no rhythm-* deps beyond rhythm-profile)
    |
rhythm-os ---> rhythm-core + rhythm-devices
    |
rhythm-ble    ---> shared Linux BLE adapter/session/scanner runtime
rhythm-hue    ---> rhythm-core + rhythm-os + rhythm-devices + rhythm-ble
rhythm-ha     ---> rhythm-core + rhythm-os + rhythm-devices
rhythm-matter ---> rhythm-core + rhythm-os + rhythm-devices
    |
rhythm-server          ---> rhythm-os + rhythm-hue + rhythm-ha + rhythm-matter
rhythm-addon           ---> rhythm-os + rhythm-hue + rhythm-ha + rhythm-matter
rhythm-linux-appliance ---> rhythm-server + rhythm-os

Protocol crates:
rhythm-zigbee ---> rhythm-core (ZCL abstractions, coordinator trait)
rhythm-ble    ---> one BlueZ resource owner + profiles/driver client API
```

## Local BLE Ownership

Linux exposes one shared Bluetooth adapter, so Rhythm has one process-wide
central-role owner for it: `rhythm-ble`. It owns the BlueZ session and adapter
handles, the Tokio runtime that drives them, discovery and advertisement
multiplexing, connection admission, GATT execution, supervision, and
quiescence. Pairing, background observation, and control operations all enter
through that client API.

No integration or device driver may create a `bluer::Session`, start a BlueZ
discovery stream, or create a private runtime for BlueZ work. Serializing only
the HTTP pairing handler is insufficient: a background button observer and a
bulb pairing scan can still contend for the adapter. Resource ownership must be
shared below every caller.

The appliance's peripheral-role provisioning GATT server is a narrow platform
exception: it advertises Rhythm setup service before normal operation and does
not discover or drive remote devices. The feature-delivery exception manifest
records its exact source, allowed ownership categories, component owner,
reason, and review expiry. A central-role profile or driver cannot use that
exception.

There are two extension shapes:

- **Profile:** the default for a lightweight, event-oriented button, sensor,
  or similar peripheral. It supplies a stable versioned profile ID, candidate
  matching and identity proof, GATT or advertisement codecs, event mapping,
  declared endpoints/actions, opaque replay ordering, and lifecycle policy.
  Profiles register through one profile-host interface, live with the
  local-BLE implementation, and do not create a crate merely for a brand. A
  profile does not start tasks or branch the public hub type; the host supplies
  observations and owns its lifecycle.
- **Driver:** appropriate for a controllable bulb or another substantial
  stateful protocol, including the existing Hue BLE bulb path. New drivers
  normally live as modules behind the local-BLE runtime and public integration.
  A driver may stay in its ecosystem crate when it has meaningful protocol
  code, dependencies, security boundaries, an existing compatibility surface,
  or an independent release surface. It still consumes `rhythm-ble`; the crate
  does not own the adapter, scanner, session, or BLE runtime.

Use this crate decision rule:

| Change | Default home |
|--------|--------------|
| New advertisement identity, QR grammar, replay token, normalized input, or small GATT codec | A profile/module in `rhythm-ble` |
| Stateful bulb/device behavior using the same dependencies and lifecycle | A driver module behind the `rhythm-ble` client API and local-BLE integration |
| Existing ecosystem gains direct BLE support | That ecosystem crate, consuming `rhythm-ble` (Hue BLE) |
| Material dependency, cryptographic/security, protocol-state, or release boundary | A separate driver crate consuming `rhythm-ble`, with the reason documented |

Brand count, file count, or the desire for a marketing label are not crate
boundaries.

The shared runtime must serve both shapes. In particular, a bulb driver needs
ordered GATT reads/writes, notification subscriptions, connection and
reconnection lifecycle, observed-state reconciliation, and per-device failure
isolation. These foreground operations must coexist fairly with low-latency
button advertisements and bounded pairing scans. One stalled or reconnecting
bulb must not starve another bulb, lose button events, or block unrelated room
readiness.

Rich drivers submit ordinary GATT work through lanes keyed by their stable,
protocol-proven device identity, never a rotating BLE address. Equal keys are
ordered across every handle for that driver; different keys can use bounded
adapter capacity independently. Driver-wide pairing, enumeration, and
address-only recovery take an exclusive scope against those lanes. One
admission deadline covers that scope, the keyed lane, and adapter capacity,
and a separate outer deadline covers session recovery plus the complete GATT
future. Timing out releases both the lane and adapter permit. Quiescence is
terminal for the old handle and drains its shared driver scope, while a fresh
post-reset handoff handle may reuse the ordering coordinator without inheriting
the old handle's terminal state.

The local transport is represented once (for example, the stable local-BLE hub
instance), while each device records its `profile_id`, device kind, and
manufacturer/model metadata. Public routing and persistence must not require a
new vendor hub type for each profile. Profile-normalized identities remain
exact private state; public device IDs are randomly allocated per appliance
rather than derived from enumerable BLE identities.

The lightweight profile registry is intentionally input-only and rejects
`Light`. It is not a shortcut around the state/control contract a bulb needs.
A future generic BLE-bulb driver must declare its public onboarding and control
capabilities and implement ordered commands, observed-state reconciliation,
notifications, and lifecycle behavior through the shared runtime. Until that
rich-driver seam exists, preserving an established integration facade such as
`hue_ble@local` is safer than pretending a stateful bulb is an event profile.

Room-readiness policy follows the control plane, not the device noun. The
appliance-local `local_ble@default` store can restore its canonical projections
without waiting for a button or bulb to be reachable, so it does not hold
unrelated Rooms behind startup. A bridge/cloud integration that must discover
authoritative topology may still block readiness. The existing `hue_ble@local`
facade keeps its shipped policy in this compatibility change; that does not
force future local-BLE bulb drivers to become startup gates.

Every profile and driver follows the reproducible contract in
[`docs/architecture/local-ble-conformance.md`](../docs/architecture/local-ble-conformance.md).
The feature-delivery BLE ownership guard enforces the process boundary.

## Integration Contract

Every control-plane integration crate must provide these pieces. A local-BLE
profile instead follows the shared profile/driver conformance contract.

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

**Reference:** `rhythm-hue/src/events.rs` -- `translate_sse_event()`

### Optional

5. **`HubDiscovery` impl** -- Network discovery + pairing UI support
6. **Multiple transport modes** -- e.g., rhythm-hue has bridge mode (WiFi) + direct local-BLE bulb mode

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

## How rhythm-hue Implements an Ecosystem Boundary

rhythm-hue is a reference integration because the Hue bridge/API is a real
ecosystem control plane. Its local-BLE bulb mode is also an example of a rich
driver that remains in an integration crate while using the shared
`rhythm-ble` runtime. Its structure demonstrates the integration pattern:

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
    events.rs           -- SSE events -> HubEvent translation (translate_sse_event)
    behavior.rs         -- HueBehaviorTracker for multi-button sequences
    discovery.rs        -- HubDiscovery impl (room/device/sensor discovery)
    api_types.rs        -- Hue V2 API type definitions
    device_types.rs     -- HueRoom, HueButton, HueSwitchDevice
    ble/                -- stateful direct-bulb driver using the rhythm-ble client API
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
    events.rs           -- native events -> HubEvent translation (incl. button -> ButtonAction mapping)
    hub_state.rs        -- integration data stored in ActiveHub::hub_data
    desktop_lifecycle.rs    -- optional first-party server-class wrapper
```

For active targets, reqwest- or daemon-backed runtime wrappers are compiled in
unconditionally. New control-plane integrations should follow the same
server-class shape instead of introducing parallel feature-gated runtime
stacks. Before creating a new crate, check whether the device is only a new
profile for an existing protocol runtime.

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

Hub-specific communication abstraction. High-level transport traits differ
fundamentally between hubs, so there is no unified hub trait. Radio-backed
transports still use their shared protocol runtime for physical resource
ownership.

### Discovery (optional)

Implement `rhythm_os::discovery::HubDiscovery` for server-driven room sync: `discover_rooms()`, `discover_devices()`, `discover_sensors()`.

## Shared Protocol Crates

Shared protocol crates abstract radio/transport access for multiple integrations.

### rhythm-zigbee
- ZCL cluster abstractions: On/Off, Level Control, Color Control, IAS Zone
- `ZigbeeCoordinator` trait -- platform provides impl (USB dongle, appliance sidecar, future hardware bridge)
- Device profiles: ZLL light, ZHA switch, IAS motion sensor
- Commission/join logic, event normalization

### rhythm-ble
- One supervised Linux BlueZ session, adapter, scanner, and runtime owner
- Multiplexed discovery/advertisement subscriptions and bounded resource leases
- GATT connect/read/write/notify operations for both simple profiles and rich drivers
- Exact profile-owned identity, random public IDs, opaque durable replay state,
  and activation-authority/restart-recovery policy
- Used by event devices and by stateful bulb drivers, including Hue BLE

Adding a BLE vendor normally means adding a tested profile, not a crate. A
separate crate is justified only by substantial dependencies, protocol state,
security isolation, or release ownership, and it must still use the shared
runtime.

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

### Phase 1: Document & Codify
- This document captures the integration and protocol boundaries
- rhythm-hue and rhythm-ha serve as control-plane reference implementations
- the local-BLE conformance contract captures profile and driver obligations

### Phase 2: Shared Radio Runtimes
`rhythm-ble` owns Linux BlueZ access. When Zigbee work starts, create
`rhythm-zigbee` with coordinator ownership and ZCL basics using the same
single-resource-owner principle.

### Phase 3: Third Integration — shipped as `rhythm-matter`
`rhythm-matter` validates the contract for non-Hue products. It is a protocol + integration hybrid: Matter standardizes light control across brands, so one crate covers every Matter light. The server acts as commissioner via the native `rhythm-chipd` daemon. Remaining Hue-specific names (like `grouped_light_id`) still get generalized as future integrations need it.

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
| `rust/integrations/rhythm-ble/` | Shared Linux BLE runtime, profile support, and driver client API |
| `rust/integrations/rhythm-hue/` | Reference integration implementation |
| `../docs/architecture/local-ble-conformance.md` | BLE profile/driver fixture and lifecycle contract |
| `rust/bins/rhythm-server/src/hub.rs` | Provider dispatch table (server) |
| `rust/bins/rhythm-linux-appliance/src/main.rs` | Appliance runtime wiring on top of the server stack |
