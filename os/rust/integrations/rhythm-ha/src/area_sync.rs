//! HA area discovery and room sync.
//!
//! Queries the HA WebSocket API for areas and entities, filters to areas
//! with light entities, and syncs them into the device registry as rooms.

use std::collections::HashMap;
use std::sync::Mutex;

use anyhow::{Context, Result};
use futures_util::{SinkExt, StreamExt};
use log::info;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use serde::Deserialize;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message;

use crate::registry::HaDeviceRegistry;
use crate::transport::HaConnectionConfig;

/// An HA area that contains light entities.
#[derive(Clone, Debug)]
pub struct HaArea {
    pub area_id: String,
    pub name: String,
    pub light_entity_ids: Vec<String>,
}

/// A motion sensor discovered from HA entity registry.
#[derive(Clone, Debug)]
pub struct HaMotionSensor {
    /// Entity ID (e.g. "binary_sensor.living_room_motion").
    pub entity_id: String,
    /// Area this sensor belongs to.
    pub area_id: String,
}

/// Result of area discovery including the device→area mapping.
///
/// The `device_area_map` is used to populate the device area cache
/// for on-demand button discovery.
#[derive(Clone, Debug)]
pub struct AreaDiscoveryResult {
    /// Areas that contain light entities.
    pub areas: Vec<HaArea>,
    /// HA device_id → area_id mapping (all devices, not just lights).
    pub device_area_map: HashMap<String, String>,
    /// Motion sensors (binary_sensor.* with motion/occupancy device class).
    pub motion_sensors: Vec<HaMotionSensor>,
    /// entity_id → area_id for ALL binary_sensor entities.
    ///
    /// Used to populate the event cache for on-demand motion sensor discovery.
    /// Broader than `motion_sensors` because some integrations (e.g. Hue via HA)
    /// don't set `original_device_class` in the entity registry — the device_class
    /// only appears in state attributes at event time.
    pub binary_sensor_areas: HashMap<String, String>,
    /// entity_id → area_id for ALL `event.*` entities.
    ///
    /// Used to populate the event cache for on-demand button discovery from
    /// HA event entities (universal button support across all integrations:
    /// Zigbee2MQTT, deCONZ, Matter, native Hue, etc.).
    pub event_entity_areas: HashMap<String, String>,
}

/// Discover areas that contain light entities via HA WebSocket.
///
/// Opens a temporary WS connection, authenticates, sends two registry
/// list commands, and returns areas that have at least one `light.*` entity.
pub fn discover_areas_with_lights(config: &HaConnectionConfig) -> Result<Vec<HaArea>> {
    let result = discover_areas_with_device_map(config)?;
    Ok(result.areas)
}

/// Discover areas with lights and also return the full device→area map.
///
/// Same as `discover_areas_with_lights` but also returns a mapping of
/// HA device_id → area_id for all devices. This map is used to populate
/// the device area cache for on-demand button registration.
pub fn discover_areas_with_device_map(config: &HaConnectionConfig) -> Result<AreaDiscoveryResult> {
    Ok(discover_full(config)?.result)
}

/// Run full discovery over a temporary WS connection.
///
/// Returns `FullRegistryData` which contains both the public
/// `AreaDiscoveryResult` and the private enrichment maps.
fn discover_full(config: &HaConnectionConfig) -> Result<FullRegistryData> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("Failed to build tokio runtime for area discovery")?;
    rt.block_on(discover_full_async(config))
}

/// Enriched device info from HA device registry (private).
#[derive(Clone)]
struct DeviceInfo {
    name: String,
    manufacturer: Option<String>,
    model: Option<String>,
    serial_number: Option<String>,
    connections: Vec<Vec<String>>,
    identifiers: Vec<Vec<String>>,
}

/// Full WS registry data — cached by HaDiscovery (private).
///
/// HA returns all three registries (areas, entities, devices) in one WS
/// session. This struct captures both the public `AreaDiscoveryResult` and
/// the identity-enrichment data needed by `discover_identities()`.
#[derive(Clone)]
struct FullRegistryData {
    /// Public area sync result.
    result: AreaDiscoveryResult,
    /// area_id → area name.
    area_names: HashMap<String, String>,
    /// device_id → enriched device info.
    device_info: HashMap<String, DeviceInfo>,
    /// entity_id → device_id.
    entity_device_map: HashMap<String, String>,
    /// Light entity_id → area_id.
    light_entity_areas: HashMap<String, String>,
    /// Entity IDs created by virtual platforms (group, homeassistant) — not physical devices.
    virtual_entity_ids: std::collections::HashSet<String>,
    /// Motion sensors with current state from `get_states`: (entity_id, area_id, is_active).
    prefetched_motion: Vec<(String, String, bool)>,
}

async fn discover_full_async(config: &HaConnectionConfig) -> Result<FullRegistryData> {
    let ws_url = config.ws_url();
    info!(target: "area_sync", "Connecting to HA for area discovery: {}", ws_url);

    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(&ws_url)
        .header("Authorization", format!("Bearer {}", config.token))
        .header("Host", &config.host)
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header(
            "Sec-WebSocket-Key",
            tokio_tungstenite::tungstenite::handshake::client::generate_key(),
        )
        .body(())
        .context("Failed to build WS request")?;

    let (ws_stream, _) = tokio_tungstenite::connect_async(request)
        .await
        .context("WS connection failed for area discovery")?;

    let (mut write, mut read) = ws_stream.split();

    // Phase 1: Authenticate
    let mut authenticated = false;
    let mut msg_id: u64 = 1;

    while let Some(msg) = read.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(d) => {
                let _ = write.send(Message::Pong(d)).await;
                continue;
            }
            _ => continue,
        };

        let json: Value = serde_json::from_str(&text)?;
        let msg_type = json.get("type").and_then(|v| v.as_str()).unwrap_or("");

        match msg_type {
            "auth_required" => {
                let auth_msg = serde_json::json!({
                    "type": "auth",
                    "access_token": config.token,
                });
                write.send(Message::Text(auth_msg.to_string())).await?;
            }
            "auth_ok" => {
                authenticated = true;
                break;
            }
            "auth_invalid" => {
                anyhow::bail!("HA authentication failed for area discovery");
            }
            _ => {}
        }
    }

    if !authenticated {
        anyhow::bail!("WS closed before authentication completed");
    }

    // Phase 2: Request area registry
    let area_req = serde_json::json!({
        "id": msg_id,
        "type": "config/area_registry/list",
    });
    let area_msg_id = msg_id;
    msg_id += 1;
    write.send(Message::Text(area_req.to_string())).await?;

    // Phase 3: Request entity registry
    let entity_req = serde_json::json!({
        "id": msg_id,
        "type": "config/entity_registry/list",
    });
    let entity_msg_id = msg_id;
    msg_id += 1;
    write.send(Message::Text(entity_req.to_string())).await?;

    // Phase 4: Request device registry (entities inherit area from device)
    let device_req = serde_json::json!({
        "id": msg_id,
        "type": "config/device_registry/list",
    });
    let device_msg_id = msg_id;
    msg_id += 1;
    write.send(Message::Text(device_req.to_string())).await?;

    // Phase 4b: Request current entity states (for motion sensor prefetch)
    let states_req = serde_json::json!({
        "id": msg_id,
        "type": "get_states",
    });
    let states_msg_id = msg_id;
    write.send(Message::Text(states_req.to_string())).await?;

    // Phase 5: Collect responses
    let mut areas_json: Option<Vec<AreaEntry>> = None;
    let mut entities_json: Option<Vec<EntityEntry>> = None;
    let mut devices_json: Option<Vec<DeviceEntry>> = None;
    let mut states_json: Option<Vec<Value>> = None;

    while let Some(msg) = read.next().await {
        let text = match msg? {
            Message::Text(t) => t,
            Message::Ping(d) => {
                let _ = write.send(Message::Pong(d)).await;
                continue;
            }
            _ => continue,
        };

        let json: Value = serde_json::from_str(&text)?;
        let resp_id = json.get("id").and_then(|v| v.as_u64()).unwrap_or(0);

        if resp_id == area_msg_id {
            if let Some(result) = json.get("result") {
                areas_json = serde_json::from_value(result.clone()).ok();
            }
        } else if resp_id == entity_msg_id {
            if let Some(result) = json.get("result") {
                entities_json = serde_json::from_value(result.clone()).ok();
            }
        } else if resp_id == device_msg_id {
            if let Some(result) = json.get("result") {
                devices_json = serde_json::from_value(result.clone()).ok();
            }
        } else if resp_id == states_msg_id {
            if let Some(result) = json.get("result") {
                states_json = serde_json::from_value(result.clone()).ok();
            }
        }

        if areas_json.is_some()
            && entities_json.is_some()
            && devices_json.is_some()
            && states_json.is_some()
        {
            break;
        }
    }

    // Close WS
    let _ = write.send(Message::Close(None)).await;

    let areas = areas_json.unwrap_or_default();
    let entities = entities_json.unwrap_or_default();
    let devices = devices_json.unwrap_or_default();

    // Build area map: area_id -> name
    let area_map: std::collections::HashMap<String, String> =
        areas.into_iter().map(|a| (a.area_id, a.name)).collect();

    // Build device→area map and device info map from devices.
    // Iterate by reference so we can build both maps in one pass.
    let mut device_area_map: HashMap<String, String> = HashMap::new();
    let mut device_info_map: HashMap<String, DeviceInfo> = HashMap::new();

    for d in &devices {
        if let Some(area) = d.area_id.as_ref().filter(|a| !a.is_empty()) {
            device_area_map.insert(d.id.clone(), area.clone());
        }
        device_info_map.insert(
            d.id.clone(),
            DeviceInfo {
                name: d.display_name().to_string(),
                manufacturer: d.manufacturer.clone(),
                model: d.model.clone(),
                serial_number: d.serial_number.clone(),
                connections: d.connections.clone(),
                identifiers: d.identifiers.clone(),
            },
        );
    }

    // Build entity_id → device_id and group light entities by area.
    // Resolution order: entity.area_id → device.area_id (most lights use the latter)
    let mut area_lights: HashMap<String, Vec<String>> = HashMap::new();
    let mut entity_device_map: HashMap<String, String> = HashMap::new();
    let mut light_entity_areas: HashMap<String, String> = HashMap::new();
    let mut virtual_entity_ids = std::collections::HashSet::new();

    for entity in &entities {
        // Track entity→device mapping for all entities (used by discover_identities)
        if let Some(device_id) = entity.device_id.as_ref().filter(|d| !d.is_empty()) {
            entity_device_map.insert(entity.entity_id.clone(), device_id.clone());
        }

        // Track virtual/group entities — these are HA-generated aggregates,
        // not physical devices. Common platforms: "group" (legacy groups),
        // "homeassistant" (area groups created by the light integration).
        if let Some(platform) = entity.platform.as_deref() {
            if matches!(platform, "group" | "homeassistant") {
                virtual_entity_ids.insert(entity.entity_id.clone());
            }
        }

        if !entity.entity_id.starts_with("light.") {
            continue;
        }
        // Resolve area: direct assignment takes priority, then device inheritance
        let area_id = entity
            .area_id
            .as_ref()
            .filter(|a| !a.is_empty())
            .cloned()
            .or_else(|| {
                entity
                    .device_id
                    .as_ref()
                    .and_then(|did| device_area_map.get(did).cloned())
            });

        if let Some(area_id) = area_id {
            light_entity_areas.insert(entity.entity_id.clone(), area_id.clone());
            area_lights
                .entry(area_id)
                .or_default()
                .push(entity.entity_id.clone());
        }
    }

    // Build result: only areas that have lights
    let result: Vec<HaArea> = area_lights
        .into_iter()
        .filter_map(|(area_id, light_ids)| {
            let name = area_map.get(&area_id)?.clone();
            Some(HaArea {
                area_id,
                name,
                light_entity_ids: light_ids,
            })
        })
        .collect();

    // Collect motion sensors: binary_sensor.* with motion or occupancy device class
    let motion_sensors: Vec<HaMotionSensor> = entities
        .iter()
        .filter(|e| {
            e.entity_id.starts_with("binary_sensor.")
                && matches!(
                    e.original_device_class.as_deref(),
                    Some("motion") | Some("occupancy")
                )
        })
        .filter_map(|e| {
            let area_id = e
                .area_id
                .as_ref()
                .filter(|a| !a.is_empty())
                .cloned()
                .or_else(|| {
                    e.device_id
                        .as_ref()
                        .and_then(|did| device_area_map.get(did).cloned())
                })?;
            Some(HaMotionSensor {
                entity_id: e.entity_id.clone(),
                area_id,
            })
        })
        .collect();

    // Build entity_id → area_id for ALL binary_sensor entities.
    // Broader than motion_sensors: some integrations (Hue via HA) don't set
    // original_device_class in the entity registry, so we rely on event-level
    // device_class filtering instead.
    let binary_sensor_areas: HashMap<String, String> = entities
        .iter()
        .filter(|e| e.entity_id.starts_with("binary_sensor."))
        .filter_map(|e| {
            let area_id = e
                .area_id
                .as_ref()
                .filter(|a| !a.is_empty())
                .cloned()
                .or_else(|| {
                    e.device_id
                        .as_ref()
                        .and_then(|did| device_area_map.get(did).cloned())
                })?;
            Some((e.entity_id.clone(), area_id))
        })
        .collect();

    // Build entity_id → area_id for ALL event.* entities (button events).
    let event_entity_areas: HashMap<String, String> = entities
        .iter()
        .filter(|e| e.entity_id.starts_with("event."))
        .filter_map(|e| {
            let area_id = e
                .area_id
                .as_ref()
                .filter(|a| !a.is_empty())
                .cloned()
                .or_else(|| {
                    e.device_id
                        .as_ref()
                        .and_then(|did| device_area_map.get(did).cloned())
                })?;
            Some((e.entity_id.clone(), area_id))
        })
        .collect();

    // Prefetch motion sensor states from get_states response.
    // Filters states_json for binary_sensors that are in binary_sensor_areas
    // and whose runtime attributes.device_class is "motion" or "occupancy".
    let prefetched_motion: Vec<(String, String, bool)> = states_json
        .unwrap_or_default()
        .iter()
        .filter_map(|state_obj| {
            let entity_id = state_obj.get("entity_id")?.as_str()?;
            let area_id = binary_sensor_areas.get(entity_id)?;
            let device_class = state_obj
                .get("attributes")
                .and_then(|a| a.get("device_class"))
                .and_then(|v| v.as_str());
            if !matches!(device_class, Some("motion") | Some("occupancy")) {
                return None;
            }
            let is_active = state_obj.get("state").and_then(|v| v.as_str()) == Some("on");
            Some((entity_id.to_string(), area_id.clone(), is_active))
        })
        .collect();

    info!(target: "area_sync",
        "Discovered {} areas with lights, {} device-area mappings, {} motion sensors, {} binary_sensor entities, {} event entities, {} prefetched motion states",
        result.len(), device_area_map.len(), motion_sensors.len(), binary_sensor_areas.len(), event_entity_areas.len(), prefetched_motion.len());
    Ok(FullRegistryData {
        result: AreaDiscoveryResult {
            areas: result,
            device_area_map,
            motion_sensors,
            binary_sensor_areas,
            event_entity_areas,
        },
        area_names: area_map,
        device_info: device_info_map,
        entity_device_map,
        light_entity_areas,
        virtual_entity_ids,
        prefetched_motion,
    })
}

/// Sync discovered areas into the device registry as rooms.
///
/// For each area with lights:
/// - Upserts a room (area_id as room_id, area_id as grouped_light_id)
/// - Sets the light entity list for the area
///
/// Returns the number of newly created rooms.
pub fn sync_areas_to_registry(areas: &[HaArea], registry: &mut HaDeviceRegistry) -> usize {
    let mut created = 0;

    for area in areas {
        let is_new = !registry.room_matches(
            &area.area_id,
            &area.name,
            &area.area_id,
            &area.light_entity_ids,
        );

        registry.upsert_room(
            &area.area_id,
            &area.name,
            &area.area_id,
            &area.light_entity_ids,
        );

        if is_new {
            created += 1;
            info!(target: "area_sync", "Created room '{}' ({}) with {} lights",
                area.name, area.area_id, area.light_entity_ids.len());
        }
    }

    info!(target: "area_sync", "Area sync complete: {} new rooms, {} total",
        created, areas.len());
    created
}

// ---------------------------------------------------------------------------
// HubDiscovery implementation
// ---------------------------------------------------------------------------

/// HA discovery wrapping the existing area sync logic.
///
/// Caches `FullRegistryData` from the first WS query so that subsequent
/// calls to `discover_devices()` and `discover_identities()` don't open
/// additional connections. Adapted from Hue's per-mapping caches — HA uses
/// one combined cache because all registry data arrives in a single WS session.
pub struct HaDiscovery {
    config: HaConnectionConfig,
    cache: Mutex<Option<FullRegistryData>>,
}

impl HaDiscovery {
    pub fn new(config: HaConnectionConfig) -> Self {
        Self {
            config,
            cache: Mutex::new(None),
        }
    }

    /// Return cached data or fetch once via WS.
    fn get_or_fetch(&self) -> Result<FullRegistryData> {
        let cached = self
            .cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock HaDiscovery cache"))?
            .clone();
        if let Some(data) = cached {
            return Ok(data);
        }
        let data = discover_full(&self.config)?;
        if let Ok(mut guard) = self.cache.lock() {
            *guard = Some(data.clone());
        }
        Ok(data)
    }
}

impl rhythm_os::discovery::HubDiscovery for HaDiscovery {
    fn discover_rooms(&self) -> Result<Vec<rhythm_os::discovery::DiscoveredRoom>> {
        let data = self.get_or_fetch()?;
        Ok(data
            .result
            .areas
            .into_iter()
            .map(|a| rhythm_os::discovery::DiscoveredRoom {
                id: a.area_id.clone(),
                name: a.name,
                grouped_light_id: a.area_id, // HA uses area_id as grouped_light
                device_ids: a.light_entity_ids,
            })
            .collect())
    }

    fn discover_devices(&self) -> Result<Vec<rhythm_os::discovery::DiscoveredDevice>> {
        let data = self.get_or_fetch()?;
        Ok(data
            .result
            .motion_sensors
            .into_iter()
            .map(|s| rhythm_os::discovery::DiscoveredDevice {
                device_id: s.entity_id,
                room_id: s.area_id,
                buttons: vec![],
                device_type: DeviceType::Motion,
            })
            .collect())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        let data = self.get_or_fetch()?;

        let mut identities = Vec::new();

        // Light entities → DiscoveredIdentity with DeviceType::Light
        // Skip:
        // 1. Virtual entities (HA "group"/"homeassistant" platform — area groups)
        // 2. Entities whose backing device has no hardware IDs (e.g. Hue "Room"
        //    group devices that HA imports with manufacturer but no MAC/serial)
        for (entity_id, area_id) in &data.light_entity_areas {
            if data.virtual_entity_ids.contains(entity_id) {
                continue;
            }
            let room_name = data.area_names.get(area_id).cloned().unwrap_or_default();
            let (name, hw_ids, manufacturer, model) = enrich_from_device(&data, entity_id);

            // Physical lights always have hardware IDs (MAC, ZHA IEEE, serial).
            // Entities with no HW IDs are virtual groups (Hue "Room", etc.).
            if hw_ids.is_empty() {
                continue;
            }

            identities.push(DiscoveredIdentity {
                native_id: entity_id.clone(),
                room_id: area_id.clone(),
                room_name,
                name,
                device_type: DeviceType::Light,
                hardware_ids: hw_ids,
                manufacturer,
                model,
            });
        }

        // Motion sensors → DiscoveredIdentity with DeviceType::Motion
        for sensor in &data.result.motion_sensors {
            let room_name = data
                .area_names
                .get(&sensor.area_id)
                .cloned()
                .unwrap_or_default();
            let (name, hw_ids, manufacturer, model) = enrich_from_device(&data, &sensor.entity_id);

            identities.push(DiscoveredIdentity {
                native_id: sensor.entity_id.clone(),
                room_id: sensor.area_id.clone(),
                room_name,
                name,
                device_type: DeviceType::Motion,
                hardware_ids: hw_ids,
                manufacturer,
                model,
            });
        }

        let light_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Light)
            .count();
        let motion_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Motion)
            .count();
        info!(
            target: "area_sync",
            "Discovered {} device identities ({} lights, {} motion) from HA",
            identities.len(),
            light_count,
            motion_count
        );

        Ok(identities)
    }

    fn discover_motion_state(&self) -> Result<Vec<rhythm_os::discovery::DiscoveredMotionState>> {
        let data = self.get_or_fetch()?;
        Ok(data
            .prefetched_motion
            .iter()
            .map(
                |(sensor_id, room_id, is_active)| rhythm_os::discovery::DiscoveredMotionState {
                    sensor_id: sensor_id.clone(),
                    room_id: room_id.clone(),
                    is_active: *is_active,
                },
            )
            .collect())
    }

    fn release_resources(&self) {
        if let Ok(mut guard) = self.cache.lock() {
            *guard = None;
        }
    }
}

// ---------------------------------------------------------------------------
// Identity enrichment helpers
// ---------------------------------------------------------------------------

/// Look up the parent device for an entity and return enriched fields.
///
/// Returns `(name, hardware_ids, manufacturer, model)`. Entities without
/// a known parent device get the entity_id as name and empty hardware IDs.
fn enrich_from_device(
    data: &FullRegistryData,
    entity_id: &str,
) -> (String, Vec<HardwareId>, Option<String>, Option<String>) {
    let device_id = data.entity_device_map.get(entity_id);
    let info = device_id.and_then(|did| data.device_info.get(did));

    match info {
        Some(info) => (
            info.name.clone(),
            extract_hardware_ids(info),
            info.manufacturer.clone(),
            info.model.clone(),
        ),
        None => (entity_id.to_string(), vec![], None, None),
    }
}

/// Extract hardware IDs from a device's connections, identifiers, and serial number.
///
/// - `connections` with type `"mac"` → `HardwareId::mac()` (normalized)
/// - ZHA identifiers → `HardwareId::mac()` via `ieee::extract_from_identifiers()`
/// - `serial_number` → `HardwareId::serial()`
/// - Dedup: skip ZHA IEEE if a matching MAC was already added from connections
fn extract_hardware_ids(info: &DeviceInfo) -> Vec<HardwareId> {
    use std::collections::HashSet;

    let mut ids = Vec::new();
    let mut seen_macs: HashSet<String> = HashSet::new();

    // MAC addresses from connections: [["mac", "aa:bb:cc:dd:ee:ff"], ...]
    for conn in &info.connections {
        if conn.len() >= 2 && conn[0] == "mac" {
            let normalized = rhythm_core::device::ieee::normalize(&conn[1]);
            if seen_macs.insert(normalized.clone()) {
                ids.push(HardwareId::mac(&conn[1]));
            }
        }
    }

    // ZHA IEEE from identifiers: [["zha", "00:17:88:01:0a:b2:c3:d4"], ...]
    if let Some(ieee) = rhythm_core::device::ieee::extract_from_identifiers(&info.identifiers) {
        let normalized = rhythm_core::device::ieee::normalize(&ieee);
        if seen_macs.insert(normalized) {
            ids.push(HardwareId::mac(&ieee));
        }
    }

    // Serial number
    if let Some(serial) = &info.serial_number {
        if !serial.is_empty() {
            ids.push(HardwareId::serial(serial));
        }
    }

    ids
}

// ---------------------------------------------------------------------------
// Deserialization helpers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct AreaEntry {
    area_id: String,
    name: String,
}

#[derive(Deserialize)]
struct EntityEntry {
    entity_id: String,
    area_id: Option<String>,
    device_id: Option<String>,
    /// Device class (e.g. "motion", "occupancy", "light").
    #[serde(default)]
    original_device_class: Option<String>,
    /// Integration platform that created this entity (e.g. "hue", "zha", "group").
    #[serde(default)]
    platform: Option<String>,
}

#[derive(Deserialize)]
struct DeviceEntry {
    id: String,
    area_id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    name_by_user: Option<String>,
    #[serde(default)]
    manufacturer: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    serial_number: Option<String>,
    /// `[["mac", "aa:bb:cc:dd:ee:ff"], ...]`
    #[serde(default)]
    connections: Vec<Vec<String>>,
    /// `[["zha", "00:17:88:01:0a:b2:c3:d4"], ...]`
    #[serde(default)]
    identifiers: Vec<Vec<String>>,
}

impl DeviceEntry {
    /// User-assigned name takes priority, then HA name, then empty.
    fn display_name(&self) -> &str {
        self.name_by_user
            .as_deref()
            .or(self.name.as_deref())
            .unwrap_or("")
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sync_areas_to_registry_creates_rooms() {
        let mut registry = HaDeviceRegistry::with_options(true);
        let areas = vec![
            HaArea {
                area_id: "living_room".to_string(),
                name: "Living Room".to_string(),
                light_entity_ids: vec![
                    "light.living_room_ceiling".to_string(),
                    "light.living_room_lamp".to_string(),
                ],
            },
            HaArea {
                area_id: "bedroom".to_string(),
                name: "Bedroom".to_string(),
                light_entity_ids: vec!["light.bedroom_main".to_string()],
            },
        ];

        let created = sync_areas_to_registry(&areas, &mut registry);
        assert_eq!(created, 2);
        assert!(registry.has_rooms());
        assert_eq!(registry.rooms().len(), 2);

        // Check light entities
        assert_eq!(registry.get_light_entities("living_room").len(), 2);
        assert_eq!(registry.get_light_entities("bedroom").len(), 1);
    }

    #[test]
    fn test_sync_areas_idempotent() {
        let mut registry = HaDeviceRegistry::with_options(true);
        let areas = vec![HaArea {
            area_id: "kitchen".to_string(),
            name: "Kitchen".to_string(),
            light_entity_ids: vec!["light.kitchen".to_string()],
        }];

        let first = sync_areas_to_registry(&areas, &mut registry);
        assert_eq!(first, 1);

        // Second sync with same data should create 0 new rooms
        let second = sync_areas_to_registry(&areas, &mut registry);
        assert_eq!(second, 0);
        assert_eq!(registry.rooms().len(), 1);
    }

    #[test]
    fn test_sync_areas_updates_name() {
        let mut registry = HaDeviceRegistry::with_options(true);

        // First sync
        let areas_v1 = vec![HaArea {
            area_id: "office".to_string(),
            name: "Office".to_string(),
            light_entity_ids: vec!["light.office_desk".to_string()],
        }];
        sync_areas_to_registry(&areas_v1, &mut registry);

        // Second sync with new name — should be an update (counted as new since name differs)
        let areas_v2 = vec![HaArea {
            area_id: "office".to_string(),
            name: "Home Office".to_string(),
            light_entity_ids: vec!["light.office_desk".to_string()],
        }];
        let created = sync_areas_to_registry(&areas_v2, &mut registry);
        assert_eq!(created, 1); // name changed, so room_matches returns false
        assert_eq!(registry.rooms().len(), 1);
    }

    // -----------------------------------------------------------------------
    // extract_hardware_ids tests
    // -----------------------------------------------------------------------

    fn make_device_info(
        connections: Vec<Vec<String>>,
        identifiers: Vec<Vec<String>>,
        serial: Option<&str>,
    ) -> DeviceInfo {
        DeviceInfo {
            name: "Test".to_string(),
            manufacturer: None,
            model: None,
            serial_number: serial.map(String::from),
            connections,
            identifiers,
        }
    }

    #[test]
    fn test_extract_hardware_ids_mac_from_connections() {
        let info = make_device_info(
            vec![vec!["mac".into(), "AA:BB:CC:DD:EE:FF".into()]],
            vec![],
            None,
        );
        let ids = extract_hardware_ids(&info);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], HardwareId::mac("AA:BB:CC:DD:EE:FF"));
    }

    #[test]
    fn test_extract_hardware_ids_ieee_from_identifiers() {
        let info = make_device_info(
            vec![],
            vec![vec!["zha".into(), "00:17:88:01:0a:b2:c3:d4".into()]],
            None,
        );
        let ids = extract_hardware_ids(&info);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], HardwareId::mac("00:17:88:01:0a:b2:c3:d4"));
    }

    #[test]
    fn test_extract_hardware_ids_serial() {
        let info = make_device_info(vec![], vec![], Some("SN12345"));
        let ids = extract_hardware_ids(&info);
        assert_eq!(ids.len(), 1);
        assert_eq!(ids[0], HardwareId::serial("SN12345"));
    }

    #[test]
    fn test_extract_hardware_ids_dedup() {
        // Same MAC in both connections and ZHA identifiers — should produce one entry
        let info = make_device_info(
            vec![vec!["mac".into(), "00:17:88:01:0a:b2:c3:d4".into()]],
            vec![vec!["zha".into(), "00:17:88:01:0a:b2:c3:d4".into()]],
            None,
        );
        let ids = extract_hardware_ids(&info);
        assert_eq!(ids.len(), 1);
    }

    #[test]
    fn test_extract_hardware_ids_empty() {
        let info = make_device_info(vec![], vec![], None);
        let ids = extract_hardware_ids(&info);
        assert!(ids.is_empty());
    }

    // -----------------------------------------------------------------------
    // DeviceEntry tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_device_entry_display_name_priority() {
        // name_by_user takes priority
        let entry: DeviceEntry = serde_json::from_str(
            r#"{
            "id": "d1",
            "name": "HA Name",
            "name_by_user": "My Custom Name"
        }"#,
        )
        .unwrap();
        assert_eq!(entry.display_name(), "My Custom Name");

        // Falls back to name
        let entry: DeviceEntry = serde_json::from_str(
            r#"{
            "id": "d2",
            "name": "HA Name"
        }"#,
        )
        .unwrap();
        assert_eq!(entry.display_name(), "HA Name");

        // Falls back to empty
        let entry: DeviceEntry = serde_json::from_str(
            r#"{
            "id": "d3"
        }"#,
        )
        .unwrap();
        assert_eq!(entry.display_name(), "");
    }

    #[test]
    fn test_device_entry_deserialize_enriched_fields() {
        let json = r#"{
            "id": "abc123",
            "area_id": "living_room",
            "name": "Hue Go",
            "manufacturer": "Signify Netherlands B.V.",
            "model": "LLC020",
            "serial_number": "001788FFFE12AB34",
            "connections": [["mac", "00:17:88:01:0a:b2:c3:d4"]],
            "identifiers": [["zha", "00:17:88:01:0a:b2:c3:d4"]]
        }"#;

        let entry: DeviceEntry = serde_json::from_str(json).unwrap();
        assert_eq!(entry.id, "abc123");
        assert_eq!(entry.area_id.as_deref(), Some("living_room"));
        assert_eq!(entry.name.as_deref(), Some("Hue Go"));
        assert_eq!(
            entry.manufacturer.as_deref(),
            Some("Signify Netherlands B.V.")
        );
        assert_eq!(entry.model.as_deref(), Some("LLC020"));
        assert_eq!(entry.serial_number.as_deref(), Some("001788FFFE12AB34"));
        assert_eq!(entry.connections.len(), 1);
        assert_eq!(entry.identifiers.len(), 1);
    }
}
