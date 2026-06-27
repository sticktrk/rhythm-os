//! HA area discovery and room sync.
//!
//! Queries the HA WebSocket API for areas and entities, filters to areas
//! with light entities, and syncs them into the device registry as rooms.

use std::collections::{HashMap, HashSet};
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

/// A contact sensor discovered from HA entity registry or state attributes.
#[derive(Clone, Debug)]
pub struct HaContactSensor {
    /// Entity ID (e.g. "binary_sensor.front_door").
    pub entity_id: String,
    /// Area this sensor belongs to.
    pub area_id: String,
}

/// A HA control source that should become a canonical Button device.
#[derive(Clone, Debug)]
struct HaButtonDevice {
    native_id: String,
    area_id: String,
    buttons: Vec<(String, u8)>,
    name_entity_id: Option<String>,
    device_id: Option<String>,
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
    /// Contact sensors (binary_sensor.* with door/window/opening device class).
    pub contact_sensors: Vec<HaContactSensor>,
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
    /// Contact sensors classified from `get_states`: (entity_id, area_id, is_open).
    prefetched_contact: Vec<(String, String, bool)>,
    /// Button/control devices that can emit HA events.
    button_devices: Vec<HaButtonDevice>,
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
    let mut light_device_ids: HashSet<String> = HashSet::new();

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
            if let Some(device_id) = entity.device_id.as_ref().filter(|d| !d.is_empty()) {
                light_device_ids.insert(device_id.clone());
            }
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
                && is_motion_device_class(e.original_device_class.as_deref())
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

    let contact_sensors: Vec<HaContactSensor> = entities
        .iter()
        .filter(|e| {
            e.entity_id.starts_with("binary_sensor.")
                && is_contact_device_class(e.original_device_class.as_deref())
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
            Some(HaContactSensor {
                entity_id: e.entity_id.clone(),
                area_id,
            })
        })
        .collect();

    let mut motion_device_ids: HashSet<String> = entities
        .iter()
        .filter(|e| {
            e.entity_id.starts_with("binary_sensor.")
                && is_motion_device_class(e.original_device_class.as_deref())
        })
        .filter_map(|e| e.device_id.as_ref().filter(|d| !d.is_empty()).cloned())
        .collect();

    let mut contact_device_ids: HashSet<String> = entities
        .iter()
        .filter(|e| {
            e.entity_id.starts_with("binary_sensor.")
                && is_contact_device_class(e.original_device_class.as_deref())
        })
        .filter_map(|e| e.device_id.as_ref().filter(|d| !d.is_empty()).cloned())
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
    let states = states_json.unwrap_or_default();

    let prefetched_motion: Vec<(String, String, bool)> = states
        .iter()
        .filter_map(|state_obj| {
            let entity_id = state_obj.get("entity_id")?.as_str()?;
            let area_id = binary_sensor_areas.get(entity_id)?;
            let device_class = state_obj
                .get("attributes")
                .and_then(|a| a.get("device_class"))
                .and_then(|v| v.as_str());
            if !is_motion_device_class(device_class) {
                return None;
            }
            let is_active = state_obj.get("state").and_then(|v| v.as_str()) == Some("on");
            Some((entity_id.to_string(), area_id.clone(), is_active))
        })
        .collect();

    let prefetched_contact: Vec<(String, String, bool)> = states
        .iter()
        .filter_map(|state_obj| {
            let entity_id = state_obj.get("entity_id")?.as_str()?;
            let area_id = binary_sensor_areas.get(entity_id)?;
            let device_class = state_obj
                .get("attributes")
                .and_then(|a| a.get("device_class"))
                .and_then(|v| v.as_str());
            if !is_contact_device_class(device_class) {
                return None;
            }
            let is_open = state_obj.get("state").and_then(|v| v.as_str()) == Some("on");
            Some((entity_id.to_string(), area_id.clone(), is_open))
        })
        .collect();

    motion_device_ids.extend(
        prefetched_motion
            .iter()
            .filter_map(|(entity_id, _, _)| entity_device_map.get(entity_id).cloned()),
    );
    contact_device_ids.extend(
        prefetched_contact
            .iter()
            .filter_map(|(entity_id, _, _)| entity_device_map.get(entity_id).cloned()),
    );

    let button_devices = build_button_devices(
        &entities,
        &device_area_map,
        &device_info_map,
        &event_entity_areas,
        &light_device_ids,
        &motion_device_ids,
        &contact_device_ids,
    );

    info!(target: "area_sync",
        "Discovered {} areas with lights, {} device-area mappings, {} motion sensors, {} contact sensors, {} binary_sensor entities, {} event entities, {} prefetched motion states, {} prefetched contact states",
        result.len(), device_area_map.len(), motion_sensors.len(), contact_sensors.len(), binary_sensor_areas.len(), event_entity_areas.len(), prefetched_motion.len(), prefetched_contact.len());
    Ok(FullRegistryData {
        result: AreaDiscoveryResult {
            areas: result,
            device_area_map,
            motion_sensors,
            contact_sensors,
            binary_sensor_areas,
            event_entity_areas,
        },
        area_names: area_map,
        device_info: device_info_map,
        entity_device_map,
        light_entity_areas,
        virtual_entity_ids,
        prefetched_motion,
        prefetched_contact,
        button_devices,
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
        let mut devices = Vec::new();
        let mut seen_motion = HashSet::new();
        let mut seen_contact = HashSet::new();

        for sensor in data.result.motion_sensors {
            if seen_motion.insert(sensor.entity_id.clone()) {
                devices.push(rhythm_os::discovery::DiscoveredDevice {
                    device_id: sensor.entity_id,
                    room_id: Some(sensor.area_id),
                    buttons: vec![],
                    device_type: DeviceType::Motion,
                });
            }
        }

        for (entity_id, area_id, _) in data.prefetched_motion {
            if seen_motion.insert(entity_id.clone()) {
                devices.push(rhythm_os::discovery::DiscoveredDevice {
                    device_id: entity_id,
                    room_id: Some(area_id),
                    buttons: vec![],
                    device_type: DeviceType::Motion,
                });
            }
        }

        for sensor in data.result.contact_sensors {
            if seen_contact.insert(sensor.entity_id.clone()) {
                devices.push(rhythm_os::discovery::DiscoveredDevice {
                    device_id: sensor.entity_id,
                    room_id: Some(sensor.area_id),
                    buttons: vec![],
                    device_type: DeviceType::Contact,
                });
            }
        }

        for (entity_id, area_id, _) in data.prefetched_contact {
            if seen_contact.insert(entity_id.clone()) {
                devices.push(rhythm_os::discovery::DiscoveredDevice {
                    device_id: entity_id,
                    room_id: Some(area_id),
                    buttons: vec![],
                    device_type: DeviceType::Contact,
                });
            }
        }

        devices.extend(data.button_devices.into_iter().map(|button| {
            rhythm_os::discovery::DiscoveredDevice {
                device_id: button.native_id,
                room_id: Some(button.area_id),
                buttons: button.buttons,
                device_type: DeviceType::Button,
            }
        }));

        Ok(devices)
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
            let room_name = data.area_names.get(area_id).cloned();
            let (name, hw_ids, manufacturer, model) = enrich_from_device(&data, entity_id);

            // Physical lights always have hardware IDs (MAC, ZHA IEEE, serial).
            // Entities with no HW IDs are virtual groups (Hue "Room", etc.).
            if hw_ids.is_empty() {
                continue;
            }

            identities.push(DiscoveredIdentity {
                native_id: entity_id.clone(),
                room_id: Some(area_id.clone()),
                room_name,
                name,
                device_type: DeviceType::Light,
                hardware_ids: hw_ids,
                manufacturer,
                model,
            });
        }

        // Motion sensors → DiscoveredIdentity with DeviceType::Motion.
        // Some HA integrations only expose motion/occupancy via state
        // attributes, so include prefetched runtime-classified sensors too.
        let mut seen_motion = HashSet::new();
        for sensor in &data.result.motion_sensors {
            if seen_motion.insert(sensor.entity_id.clone()) {
                push_motion_identity(&data, &mut identities, &sensor.entity_id, &sensor.area_id);
            }
        }
        for (entity_id, area_id, _) in &data.prefetched_motion {
            if seen_motion.insert(entity_id.clone()) {
                push_motion_identity(&data, &mut identities, entity_id, area_id);
            }
        }

        let mut seen_contact = HashSet::new();
        for sensor in &data.result.contact_sensors {
            if seen_contact.insert(sensor.entity_id.clone()) {
                push_contact_identity(&data, &mut identities, &sensor.entity_id, &sensor.area_id);
            }
        }
        for (entity_id, area_id, _) in &data.prefetched_contact {
            if seen_contact.insert(entity_id.clone()) {
                push_contact_identity(&data, &mut identities, entity_id, area_id);
            }
        }

        // Button/control devices → DiscoveredIdentity with DeviceType::Button
        for button in &data.button_devices {
            let room_name = data.area_names.get(&button.area_id).cloned();
            let (name, hw_ids, manufacturer, model) = button_identity_fields(&data, button);

            identities.push(DiscoveredIdentity {
                native_id: button.native_id.clone(),
                room_id: Some(button.area_id.clone()),
                room_name,
                name,
                device_type: DeviceType::Button,
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
        let button_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Button)
            .count();
        let contact_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Contact)
            .count();
        info!(
            target: "area_sync",
            "Discovered {} device identities ({} lights, {} buttons, {} motion, {} contact) from HA",
            identities.len(),
            light_count,
            button_count,
            motion_count,
            contact_count
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

fn build_button_devices(
    entities: &[EntityEntry],
    device_area_map: &HashMap<String, String>,
    device_info: &HashMap<String, DeviceInfo>,
    event_entity_areas: &HashMap<String, String>,
    light_device_ids: &HashSet<String>,
    motion_device_ids: &HashSet<String>,
    contact_device_ids: &HashSet<String>,
) -> Vec<HaButtonDevice> {
    let mut by_native: HashMap<String, HaButtonDevice> = HashMap::new();
    let mut event_parent_device_ids: HashSet<String> = HashSet::new();

    for entity in entities
        .iter()
        .filter(|entity| entity.entity_id.starts_with("event."))
    {
        let Some(area_id) = event_entity_areas.get(&entity.entity_id) else {
            continue;
        };
        let device_id = entity
            .device_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .cloned();
        let native_id = device_id
            .as_deref()
            .map(|id| control_native_id_for_device(id, device_info.get(id)))
            .unwrap_or_else(|| entity.entity_id.clone());

        if let Some(device_id) = &device_id {
            event_parent_device_ids.insert(device_id.clone());
        }

        let entry = by_native
            .entry(native_id.clone())
            .or_insert_with(|| HaButtonDevice {
                native_id,
                area_id: area_id.clone(),
                buttons: Vec::new(),
                name_entity_id: Some(entity.entity_id.clone()),
                device_id: device_id.clone(),
            });
        let control_id = parse_event_button_number(&entity.entity_id);
        if !entry
            .buttons
            .iter()
            .any(|(button_id, _)| button_id == &entity.entity_id)
        {
            entry.buttons.push((entity.entity_id.clone(), control_id));
        }
        if entry.name_entity_id.is_none() {
            entry.name_entity_id = Some(entity.entity_id.clone());
        }
        if entry.device_id.is_none() {
            entry.device_id = device_id;
        }
    }

    for (device_id, area_id) in device_area_map {
        if light_device_ids.contains(device_id)
            || motion_device_ids.contains(device_id)
            || contact_device_ids.contains(device_id)
            || event_parent_device_ids.contains(device_id)
        {
            continue;
        }

        let info = device_info.get(device_id);
        if !looks_like_control_device(info) {
            continue;
        }

        let native_id = control_native_id_for_device(device_id, info);
        by_native
            .entry(native_id.clone())
            .or_insert_with(|| HaButtonDevice {
                native_id,
                area_id: area_id.clone(),
                buttons: Vec::new(),
                name_entity_id: None,
                device_id: Some(device_id.clone()),
            });
    }

    let mut devices: Vec<_> = by_native.into_values().collect();
    for device in &mut devices {
        device
            .buttons
            .sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    }
    devices.sort_by(|left, right| left.native_id.cmp(&right.native_id));
    devices
}

fn parse_event_button_number(entity_id: &str) -> u8 {
    let name = entity_id.strip_prefix("event.").unwrap_or(entity_id);
    if let Some(last_underscore) = name.rfind('_') {
        if let Ok(n) = name[last_underscore + 1..].parse::<u8>() {
            if (1..=8).contains(&n) {
                return n;
            }
        }
    }
    1
}

fn control_native_id_for_device(device_id: &str, info: Option<&DeviceInfo>) -> String {
    info.and_then(zha_identifier)
        .unwrap_or_else(|| device_id.to_string())
}

fn zha_identifier(info: &DeviceInfo) -> Option<String> {
    rhythm_core::device::ieee::extract_from_identifiers(&info.identifiers)
}

fn looks_like_control_device(info: Option<&DeviceInfo>) -> bool {
    let Some(info) = info else {
        return false;
    };

    let mut haystack = info.name.to_lowercase();
    if let Some(manufacturer) = &info.manufacturer {
        haystack.push(' ');
        haystack.push_str(&manufacturer.to_lowercase());
    }
    if let Some(model) = &info.model {
        haystack.push(' ');
        haystack.push_str(&model.to_lowercase());
    }

    [
        "button", "switch", "remote", "dimmer", "dial", "knob", "scene", "shortcut", "tap", "pico",
    ]
    .iter()
    .any(|needle| haystack.contains(needle))
}

fn button_identity_fields(
    data: &FullRegistryData,
    button: &HaButtonDevice,
) -> (String, Vec<HardwareId>, Option<String>, Option<String>) {
    if let Some(device_id) = &button.device_id {
        if let Some(info) = data.device_info.get(device_id) {
            return (
                if info.name.is_empty() {
                    button.native_id.clone()
                } else {
                    info.name.clone()
                },
                extract_hardware_ids(info),
                info.manufacturer.clone(),
                info.model.clone(),
            );
        }
    }

    if let Some(entity_id) = &button.name_entity_id {
        return enrich_from_device(data, entity_id);
    }

    (button.native_id.clone(), Vec::new(), None, None)
}

fn push_motion_identity(
    data: &FullRegistryData,
    identities: &mut Vec<DiscoveredIdentity>,
    entity_id: &str,
    area_id: &str,
) {
    let room_name = data.area_names.get(area_id).cloned();
    let (name, hw_ids, manufacturer, model) = enrich_from_device(data, entity_id);

    identities.push(DiscoveredIdentity {
        native_id: entity_id.to_string(),
        room_id: Some(area_id.to_string()),
        room_name,
        name,
        device_type: DeviceType::Motion,
        hardware_ids: hw_ids,
        manufacturer,
        model,
    });
}

fn push_contact_identity(
    data: &FullRegistryData,
    identities: &mut Vec<DiscoveredIdentity>,
    entity_id: &str,
    area_id: &str,
) {
    let room_name = data.area_names.get(area_id).cloned();
    let (name, hw_ids, manufacturer, model) = enrich_from_device(data, entity_id);

    identities.push(DiscoveredIdentity {
        native_id: entity_id.to_string(),
        room_id: Some(area_id.to_string()),
        room_name,
        name,
        device_type: DeviceType::Contact,
        hardware_ids: hw_ids,
        manufacturer,
        model,
    });
}

fn is_motion_device_class(device_class: Option<&str>) -> bool {
    matches!(device_class, Some("motion") | Some("occupancy"))
}

fn is_contact_device_class(device_class: Option<&str>) -> bool {
    matches!(
        device_class,
        Some("door") | Some("window") | Some("opening") | Some("garage_door")
    )
}

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
    use futures_util::{SinkExt, StreamExt};
    use rhythm_os::discovery::HubDiscovery;
    use tokio::net::TcpListener;

    fn spawn_fake_ha_ws() -> (HaConnectionConfig, std::thread::JoinHandle<()>) {
        let (port_tx, port_rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            let runtime = tokio::runtime::Runtime::new().unwrap();
            runtime.block_on(async move {
                let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
                port_tx.send(listener.local_addr().unwrap().port()).unwrap();
            let (stream, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(
                serde_json::json!({"type": "auth_required"}).to_string(),
            ))
            .await
            .unwrap();

            let auth = ws.next().await.unwrap().unwrap().into_text().unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&auth).unwrap()["access_token"],
                "test-token"
            );
            ws.send(Message::Text(
                serde_json::json!({"type": "auth_ok"}).to_string(),
            ))
            .await
            .unwrap();

            let mut ids = HashMap::<String, u64>::new();
            while ids.len() < 4 {
                let msg = ws.next().await.unwrap().unwrap().into_text().unwrap();
                let json: Value = serde_json::from_str(&msg).unwrap();
                ids.insert(
                    json["type"].as_str().unwrap().to_string(),
                    json["id"].as_u64().unwrap(),
                );
            }

            let response = |request_type: &str, result: Value| {
                serde_json::json!({
                    "id": ids[request_type],
                    "type": "result",
                    "success": true,
                    "result": result,
                })
                .to_string()
            };
            ws.send(Message::Text(response(
                "config/area_registry/list",
                serde_json::json!([
                    {"area_id": "kitchen", "name": "Kitchen"},
                    {"area_id": "office", "name": "Office"}
                ]),
            )))
            .await
            .unwrap();
            ws.send(Message::Text(response(
                "config/entity_registry/list",
                serde_json::json!([
                    {"entity_id": "light.kitchen_ceiling", "device_id": "dev-light"},
                    {"entity_id": "light.kitchen_group", "area_id": "kitchen", "device_id": "dev-virtual", "platform": "group"},
                    {"entity_id": "light.office_virtual", "area_id": "office", "device_id": "dev-nohw"},
                    {"entity_id": "binary_sensor.kitchen_motion", "device_id": "dev-motion", "original_device_class": "motion"},
                    {"entity_id": "binary_sensor.kitchen_occupancy", "area_id": "kitchen", "original_device_class": "occupancy"},
                    {"entity_id": "binary_sensor.kitchen_door", "device_id": "dev-contact", "original_device_class": "door"},
                    {"entity_id": "binary_sensor.kitchen_battery", "area_id": "kitchen"},
                    {"entity_id": "binary_sensor.office_hue_motion", "device_id": "dev-hue-motion"},
                    {"entity_id": "binary_sensor.office_window", "device_id": "dev-window"},
                    {"entity_id": "event.kitchen_remote_button_1", "device_id": "dev-remote", "platform": "zha"},
                    {"entity_id": "event.kitchen_remote_button_2", "device_id": "dev-remote", "platform": "zha"}
                ]),
            )))
            .await
            .unwrap();
            ws.send(Message::Text(response(
                "config/device_registry/list",
                serde_json::json!([
                    {
                        "id": "dev-light",
                        "area_id": "kitchen",
                        "name": "Kitchen Ceiling",
                        "manufacturer": "Signify",
                        "model": "Hue Bulb",
                        "connections": [["mac", "00:17:88:01:02:03:04"]]
                    },
                    {"id": "dev-virtual", "area_id": "kitchen", "name": "Kitchen Group"},
                    {"id": "dev-nohw", "area_id": "office", "name": "Office Virtual Light"},
                    {
                        "id": "dev-motion",
                        "area_id": "kitchen",
                        "name": "Kitchen Motion",
                        "manufacturer": "Aqara",
                        "model": "Motion Sensor",
                        "serial_number": "motion-123"
                    },
                    {
                        "id": "dev-hue-motion",
                        "area_id": "office",
                        "name": "Office Hue Motion",
                        "manufacturer": "Signify",
                        "model": "SML002",
                        "identifiers": [["zha", "00:17:88:01:04:05:06:07"]]
                    },
                    {
                        "id": "dev-contact",
                        "area_id": "kitchen",
                        "name": "Kitchen Door",
                        "manufacturer": "Aqara",
                        "model": "Door Sensor",
                        "serial_number": "door-123"
                    },
                    {
                        "id": "dev-window",
                        "area_id": "office",
                        "name": "Office Window",
                        "manufacturer": "Aqara",
                        "model": "Window Sensor",
                        "serial_number": "window-456"
                    },
                    {
                        "id": "dev-remote",
                        "area_id": "kitchen",
                        "name_by_user": "Kitchen Remote",
                        "manufacturer": "Signify",
                        "model": "RWL022",
                        "identifiers": [["zha", "00:17:88:01:0a:b2:c3:d4"]]
                    },
                    {
                        "id": "dev-scene-switch",
                        "area_id": "kitchen",
                        "name": "Wall Scene Switch",
                        "manufacturer": "Lutron",
                        "model": "Pico Remote",
                        "serial_number": "switch-456"
                    }
                ]),
            )))
            .await
            .unwrap();
            ws.send(Message::Text(response(
                "get_states",
                serde_json::json!([
                    {
                        "entity_id": "binary_sensor.kitchen_motion",
                        "state": "on",
                        "attributes": {"device_class": "motion"}
                    },
                    {
                        "entity_id": "binary_sensor.kitchen_occupancy",
                        "state": "off",
                        "attributes": {"device_class": "occupancy"}
                    },
                    {
                        "entity_id": "binary_sensor.kitchen_battery",
                        "state": "off",
                        "attributes": {"device_class": "battery"}
                    },
                    {
                        "entity_id": "binary_sensor.office_hue_motion",
                        "state": "off",
                        "attributes": {"device_class": "motion"}
                    },
                    {
                        "entity_id": "binary_sensor.office_window",
                        "state": "on",
                        "attributes": {"device_class": "window"}
                    }
                ]),
            )))
            .await
            .unwrap();

            while let Some(Ok(msg)) = ws.next().await {
                if matches!(msg, Message::Close(_)) {
                    break;
                }
            }
            });
        });
        let port = port_rx.recv().unwrap();

        (
            HaConnectionConfig {
                host: "127.0.0.1".to_string(),
                port,
                token: "test-token".to_string(),
                use_ssl: false,
            },
            handle,
        )
    }

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
    fn ha_discovery_maps_full_registry_data_from_websocket() {
        let (config, server) = spawn_fake_ha_ws();
        let discovery = HaDiscovery::new(config);

        let mut rooms = discovery.discover_rooms().unwrap();
        rooms.sort_by(|left, right| left.id.cmp(&right.id));
        assert_eq!(rooms.len(), 2);
        assert_eq!(rooms[0].id, "kitchen");
        assert_eq!(rooms[0].name, "Kitchen");
        assert_eq!(rooms[0].grouped_light_id, "kitchen");
        assert_eq!(
            rooms[0].device_ids,
            vec![
                "light.kitchen_ceiling".to_string(),
                "light.kitchen_group".to_string()
            ]
        );
        assert_eq!(rooms[1].id, "office");

        let devices = discovery.discover_devices().unwrap();
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Motion
                && device.device_id == "binary_sensor.kitchen_motion"
                && device.room_id.as_deref() == Some("kitchen")
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Motion
                && device.device_id == "binary_sensor.office_hue_motion"
                && device.room_id.as_deref() == Some("office")
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Contact
                && device.device_id == "binary_sensor.kitchen_door"
                && device.room_id.as_deref() == Some("kitchen")
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Contact
                && device.device_id == "binary_sensor.office_window"
                && device.room_id.as_deref() == Some("office")
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Button
                && device.device_id == "00:17:88:01:0a:b2:c3:d4"
                && device.buttons
                    == vec![
                        ("event.kitchen_remote_button_1".to_string(), 1),
                        ("event.kitchen_remote_button_2".to_string(), 2),
                    ]
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Button
                && device.device_id == "dev-scene-switch"
                && device.buttons.is_empty()
        }));

        let identities = discovery.discover_identities().unwrap();
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Light
                && identity.native_id == "light.kitchen_ceiling"
                && identity.name == "Kitchen Ceiling"
                && identity.hardware_ids == vec![HardwareId::mac("00:17:88:01:02:03:04")]
        }));
        assert!(!identities
            .iter()
            .any(|identity| identity.native_id == "light.kitchen_group"));
        assert!(!identities
            .iter()
            .any(|identity| identity.native_id == "light.office_virtual"));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Motion
                && identity.native_id == "binary_sensor.kitchen_motion"
                && identity.hardware_ids == vec![HardwareId::serial("motion-123")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Motion
                && identity.native_id == "binary_sensor.office_hue_motion"
                && identity.name == "Office Hue Motion"
                && identity.hardware_ids == vec![HardwareId::mac("00:17:88:01:04:05:06:07")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Contact
                && identity.native_id == "binary_sensor.kitchen_door"
                && identity.name == "Kitchen Door"
                && identity.hardware_ids == vec![HardwareId::serial("door-123")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Contact
                && identity.native_id == "binary_sensor.office_window"
                && identity.name == "Office Window"
                && identity.hardware_ids == vec![HardwareId::serial("window-456")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Button
                && identity.native_id == "00:17:88:01:0a:b2:c3:d4"
                && identity.name == "Kitchen Remote"
        }));

        let mut motion = discovery.discover_motion_state().unwrap();
        motion.sort_by(|left, right| left.sensor_id.cmp(&right.sensor_id));
        assert_eq!(motion.len(), 3);
        assert_eq!(motion[0].sensor_id, "binary_sensor.kitchen_motion");
        assert!(motion[0].is_active);
        assert_eq!(motion[1].sensor_id, "binary_sensor.kitchen_occupancy");
        assert!(!motion[1].is_active);
        assert_eq!(motion[2].sensor_id, "binary_sensor.office_hue_motion");
        assert!(!motion[2].is_active);

        discovery.release_resources();
        server.join().unwrap();
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

    fn named_device_info(name: &str, model: &str) -> DeviceInfo {
        DeviceInfo {
            name: name.to_string(),
            manufacturer: Some("Test".to_string()),
            model: Some(model.to_string()),
            serial_number: None,
            connections: Vec::new(),
            identifiers: vec![vec![
                "zha".to_string(),
                "00:17:88:01:0a:b2:c3:d4".to_string(),
            ]],
        }
    }

    #[test]
    fn test_build_button_devices_includes_event_entities() {
        let entities = vec![EntityEntry {
            entity_id: "event.hue_dimmer_button_2".to_string(),
            area_id: None,
            device_id: Some("device-1".to_string()),
            original_device_class: None,
            platform: Some("zha".to_string()),
        }];
        let device_area_map = HashMap::from([("device-1".to_string(), "kitchen".to_string())]);
        let device_info = HashMap::from([(
            "device-1".to_string(),
            named_device_info("Hue Dimmer", "RWL022"),
        )]);
        let event_entity_areas = HashMap::from([(
            "event.hue_dimmer_button_2".to_string(),
            "kitchen".to_string(),
        )]);

        let devices = build_button_devices(
            &entities,
            &device_area_map,
            &device_info,
            &event_entity_areas,
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].area_id, "kitchen");
        assert_eq!(
            devices[0].buttons,
            vec![("event.hue_dimmer_button_2".to_string(), 2)]
        );
    }

    #[test]
    fn test_build_button_devices_skips_non_control_zha_devices() {
        let device_area_map = HashMap::from([("temp-1".to_string(), "kitchen".to_string())]);
        let device_info = HashMap::from([(
            "temp-1".to_string(),
            named_device_info("Kitchen Temperature", "Temperature Sensor"),
        )]);

        let devices = build_button_devices(
            &[],
            &device_area_map,
            &device_info,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        );

        assert!(devices.is_empty());
    }

    #[test]
    fn test_build_button_devices_includes_control_named_devices_without_event_entity() {
        let device_area_map = HashMap::from([("remote-1".to_string(), "kitchen".to_string())]);
        let device_info = HashMap::from([(
            "remote-1".to_string(),
            named_device_info("Kitchen Remote", "Scene Switch"),
        )]);

        let devices = build_button_devices(
            &[],
            &device_area_map,
            &device_info,
            &HashMap::new(),
            &HashSet::new(),
            &HashSet::new(),
            &HashSet::new(),
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].area_id, "kitchen");
        assert!(devices[0].buttons.is_empty());
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
