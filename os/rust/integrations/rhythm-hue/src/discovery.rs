//! Hue V2 REST discovery implementation.
//!
//! Implements `HubDiscovery` by querying the Hue bridge V2 API for rooms
//! and typed devices (buttons + motion sensors).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{debug, info, warn};

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::DeviceRegistry;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
use rhythm_os::registry::HubDeviceRegistry;

use crate::transport::HueTransport;

/// Hue V2 discovery using the bridge REST API.
///
/// Caches the device→room mapping built during `discover_rooms()` so that
/// `discover_devices()` can skip re-fetching the rooms JSON.
/// On ESP32 this avoids ~30-50KB of peak heap from parsing rooms twice.
pub struct HueDiscovery<H: HueTransport> {
    transport: Arc<H>,
    username: String,
    /// Cached device_id → room_id mapping, populated by `discover_rooms()`.
    device_to_room_cache: Mutex<Option<HashMap<String, String>>>,
    /// Cached room_id → room_name mapping, populated by `discover_rooms()`.
    room_name_cache: Mutex<Option<HashMap<String, String>>>,
}

impl<H: HueTransport> HueDiscovery<H> {
    pub fn new(transport: Arc<H>, username: String) -> Self {
        Self {
            transport,
            username,
            device_to_room_cache: Mutex::new(None),
            room_name_cache: Mutex::new(None),
        }
    }

    /// Build device_id → room_id mapping from the room JSON array.
    fn build_device_to_room(rooms_data: &[serde_json::Value]) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for room_json in rooms_data {
            let room_id = match room_json.pointer("/id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            if let Some(children) = room_json.get("children").and_then(|v| v.as_array()) {
                for child in children {
                    if child.get("rtype").and_then(|v| v.as_str()) == Some("device") {
                        if let Some(rid) = child.get("rid").and_then(|v| v.as_str()) {
                            map.insert(rid.to_string(), room_id.clone());
                        }
                    }
                }
            }
        }
        map
    }

    /// Extract typed devices (buttons + motion sensors) from a device JSON array
    /// in a single pass over the data.
    fn extract_devices(
        device_data: &[serde_json::Value],
        device_to_room: &HashMap<String, String>,
    ) -> Vec<DiscoveredDevice> {
        let mut devices = Vec::new();

        for dev_json in device_data {
            let device_id = match dev_json.pointer("/id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let room_id = match device_to_room.get(&device_id) {
                Some(id) => id.clone(),
                None => continue,
            };

            let services = dev_json.get("services").and_then(|v| v.as_array());

            if let Some(svcs) = services {
                let mut btns = Vec::new();
                let mut control_id: u8 = 1;
                let mut motion_id: Option<String> = None;

                for svc in svcs {
                    let rtype = svc.get("rtype").and_then(|v| v.as_str()).unwrap_or("");
                    match rtype {
                        "button" => {
                            if let Some(rid) = svc.get("rid").and_then(|v| v.as_str()) {
                                btns.push((rid.to_string(), control_id));
                                control_id += 1;
                            }
                        }
                        "motion" => {
                            motion_id = svc.get("rid").and_then(|v| v.as_str()).map(String::from);
                        }
                        _ => {}
                    }
                }

                if !btns.is_empty() {
                    devices.push(DiscoveredDevice {
                        device_id: device_id.clone(),
                        room_id: room_id.clone(),
                        buttons: btns,
                        device_type: DeviceType::Button,
                    });
                }

                if let Some(mid) = motion_id {
                    devices.push(DiscoveredDevice {
                        device_id: mid,
                        room_id,
                        buttons: vec![],
                        device_type: DeviceType::Motion,
                    });
                }
            }
        }

        devices
    }

    /// Extract device identities with hardware IDs from a device JSON array.
    ///
    /// Processes ALL devices (lights, buttons, motion) — not just typed devices.
    fn extract_identities(
        device_data: &[serde_json::Value],
        device_to_room: &HashMap<String, String>,
        room_names: &HashMap<String, String>,
        zigbee_mac_map: &HashMap<String, String>,
    ) -> Vec<DiscoveredIdentity> {
        let mut identities = Vec::new();

        for dev_json in device_data {
            let device_id = match dev_json.pointer("/id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let room_id = match device_to_room.get(&device_id) {
                Some(id) => id.clone(),
                None => continue,
            };

            let room_name = room_names.get(&room_id).cloned().unwrap_or_default();

            let name = dev_json
                .pointer("/metadata/name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();

            let manufacturer = dev_json
                .pointer("/product_data/manufacturer_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let model = dev_json
                .pointer("/product_data/model_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let services = dev_json.get("services").and_then(|v| v.as_array());
            let device_type = match Self::infer_device_type(services) {
                Some(dt) => dt,
                None => continue, // No recognizable service (bridge, etc.)
            };

            let hardware_ids = zigbee_mac_map
                .get(&device_id)
                .map(|mac| vec![HardwareId::mac(mac)])
                .unwrap_or_default();

            // Skip virtual group lights (Hue "Room" devices) — physical Hue
            // lights always have zigbee connectivity and therefore a MAC.
            if device_type == DeviceType::Light && hardware_ids.is_empty() {
                continue;
            }

            identities.push(DiscoveredIdentity {
                native_id: device_id,
                room_id,
                room_name,
                name,
                device_type,
                hardware_ids,
                manufacturer,
                model,
            });
        }

        identities
    }

    /// Infer device type from Hue V2 service list.
    fn infer_device_type(services: Option<&Vec<serde_json::Value>>) -> Option<DeviceType> {
        let services = services?;
        let mut has_light = false;
        let mut has_button = false;
        let mut has_motion = false;

        for svc in services {
            match svc.get("rtype").and_then(|v| v.as_str()) {
                Some("light") => has_light = true,
                Some("button") => has_button = true,
                Some("motion") => has_motion = true,
                _ => {}
            }
        }

        if has_light {
            Some(DeviceType::Light)
        } else if has_button {
            Some(DeviceType::Button)
        } else if has_motion {
            Some(DeviceType::Motion)
        } else {
            None
        }
    }

    /// Build device_id → mac_address mapping from zigbee_connectivity resources.
    fn build_zigbee_mac_map(zigbee_data: &[serde_json::Value]) -> HashMap<String, String> {
        let mut map = HashMap::new();
        for entry in zigbee_data {
            let device_id = entry.pointer("/owner/rid").and_then(|v| v.as_str());
            let mac = entry.get("mac_address").and_then(|v| v.as_str());
            if let (Some(did), Some(mac)) = (device_id, mac) {
                map.insert(did.to_string(), mac.to_string());
            }
        }
        map
    }
}

impl<H: HueTransport> HueDiscovery<H> {
    /// Shared implementation: fetch devices and extract typed devices.
    ///
    /// Uses the device→room cache if populated by a prior `discover_rooms()` call,
    /// avoiding a second rooms JSON fetch (~30-50KB saved on ESP32). Falls back
    /// to fetching rooms if the cache is empty.
    fn fetch_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        // Try to use the cached device→room mapping from discover_rooms()
        let device_to_room = {
            let cache = self
                .device_to_room_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock device_to_room cache"))?;
            cache.clone()
        };

        let device_to_room = match device_to_room {
            Some(cached) => {
                info!(target: "hue_discovery", "Using cached device→room mapping ({} entries)", cached.len());
                cached
            }
            None => {
                // Fallback: fetch rooms if cache wasn't populated
                let rooms_resp = self.transport.get_resources(&self.username, "room")?;
                let empty = Vec::new();
                let rooms_data = rooms_resp
                    .get("data")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty);
                Self::build_device_to_room(rooms_data)
                // rooms_resp dropped here
            }
        };

        let dev_resp = self.transport.get_resources(&self.username, "device")?;
        let empty = Vec::new();
        let dev_data = dev_resp
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        Ok(Self::extract_devices(dev_data, &device_to_room))
    }

    /// Extract the device reference from a Hue V2 behavior_instance JSON.
    ///
    /// The behavior_instance JSON structure varies by behavior script type.
    /// We try multiple known paths to find the device reference:
    /// 1. `configuration.where[N].group.rid` (common for switch-configured behaviors)
    /// 2. `configuration.device.rid` (direct device reference)
    /// 3. `group.rid` (top-level group reference)
    ///
    /// Returns the device/group resource ID if found.
    fn extract_device_from_behavior(bi: &serde_json::Value) -> Option<String> {
        // Path 1: configuration.where[].group.rid
        if let Some(wheres) = bi
            .pointer("/configuration/where")
            .and_then(|v| v.as_array())
        {
            for w in wheres {
                if let Some(rid) = w.pointer("/group/rid").and_then(|v| v.as_str()) {
                    return Some(rid.to_string());
                }
            }
        }

        // Path 2: configuration.device.rid
        if let Some(rid) = bi
            .pointer("/configuration/device/rid")
            .and_then(|v| v.as_str())
        {
            return Some(rid.to_string());
        }

        // Path 3: group.rid (top-level)
        if let Some(rid) = bi.pointer("/group/rid").and_then(|v| v.as_str()) {
            return Some(rid.to_string());
        }

        // Path 4: Walk "configuration" looking for any object with "rid" + "rtype": "device"
        if let Some(config) = bi.get("configuration") {
            if let Some(rid) = Self::find_device_rid(config) {
                return Some(rid);
            }
        }

        None
    }

    /// Recursively search a JSON value for an object with `rtype: "device"` and extract its `rid`.
    fn find_device_rid(value: &serde_json::Value) -> Option<String> {
        match value {
            serde_json::Value::Object(map) => {
                if map.get("rtype").and_then(|v| v.as_str()) == Some("device") {
                    if let Some(rid) = map.get("rid").and_then(|v| v.as_str()) {
                        return Some(rid.to_string());
                    }
                }
                for v in map.values() {
                    if let Some(rid) = Self::find_device_rid(v) {
                        return Some(rid);
                    }
                }
                None
            }
            serde_json::Value::Array(arr) => {
                for v in arr {
                    if let Some(rid) = Self::find_device_rid(v) {
                        return Some(rid);
                    }
                }
                None
            }
            _ => None,
        }
    }
}

impl<H: HueTransport + 'static> HubDiscovery for HueDiscovery<H> {
    fn release_resources(&self) {
        self.transport.release_connection();
    }

    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        let resp = self.transport.get_resources(&self.username, "room")?;
        let data = resp
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("No data array in room response"))?;

        // Build device→room cache while iterating (avoids re-fetching rooms later)
        let mut device_to_room = HashMap::new();
        let mut rooms = Vec::new();

        for room_json in data {
            let id = match room_json.pointer("/id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            let name = room_json
                .pointer("/metadata/name")
                .and_then(|v| v.as_str())
                .unwrap_or("Unknown")
                .to_string();

            // Find grouped_light service
            let grouped_light_id = room_json
                .get("services")
                .and_then(|v| v.as_array())
                .and_then(|services| {
                    services.iter().find_map(|svc| {
                        let rtype = svc.get("rtype").and_then(|v| v.as_str())?;
                        if rtype == "grouped_light" {
                            svc.get("rid").and_then(|v| v.as_str()).map(String::from)
                        } else {
                            None
                        }
                    })
                })
                .unwrap_or_default();

            if grouped_light_id.is_empty() {
                continue; // Skip rooms without a grouped_light (can't control them)
            }

            // Collect child device IDs and populate device→room cache
            let device_ids: Vec<String> = room_json
                .get("children")
                .and_then(|v| v.as_array())
                .map(|children| {
                    children
                        .iter()
                        .filter_map(|child| {
                            let rtype = child.get("rtype").and_then(|v| v.as_str())?;
                            if rtype == "device" {
                                child.get("rid").and_then(|v| v.as_str()).map(String::from)
                            } else {
                                None
                            }
                        })
                        .collect()
                })
                .unwrap_or_default();

            for dev_id in &device_ids {
                device_to_room.insert(dev_id.clone(), id.clone());
            }

            rooms.push(DiscoveredRoom {
                id,
                name,
                grouped_light_id,
                device_ids,
            });
        }

        // Cache the mapping for fetch_devices()
        if let Ok(mut cache) = self.device_to_room_cache.lock() {
            *cache = Some(device_to_room);
        }

        // Cache room names for discover_identities()
        let mut room_names = HashMap::new();
        for room in &rooms {
            room_names.insert(room.id.clone(), room.name.clone());
        }
        if let Ok(mut cache) = self.room_name_cache.lock() {
            *cache = Some(room_names);
        }

        info!(target: "hue_discovery", "Discovered {} rooms from Hue bridge", rooms.len());
        Ok(rooms)
    }

    /// Discover all typed devices (buttons + motion sensors) from the hub.
    ///
    /// Makes only 2 HTTP calls (rooms + devices if cache not populated,
    /// or just devices if cache populated by prior discover_rooms() call).
    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        let devices = self.fetch_devices()?;
        let button_count = devices
            .iter()
            .filter(|d| d.device_type == DeviceType::Button)
            .count();
        let motion_count = devices
            .iter()
            .filter(|d| d.device_type == DeviceType::Motion)
            .count();
        info!(target: "hue_discovery", "Discovered {} button devices and {} motion sensors from Hue bridge",
            button_count, motion_count);
        Ok(devices)
    }

    /// Discover all devices with full hardware identity information.
    ///
    /// Returns identities for ALL device types (lights, buttons, motion) with
    /// names, manufacturer/model, and MAC addresses from zigbee_connectivity.
    /// Used by the canonical device registry for cross-hub deduplication.
    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        // Get device→room cache (populated by discover_rooms or fetched on demand)
        let device_to_room = {
            let cache = self
                .device_to_room_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock device_to_room cache"))?;
            cache.clone()
        };

        let device_to_room = match device_to_room {
            Some(cached) => cached,
            None => {
                let rooms_resp = self.transport.get_resources(&self.username, "room")?;
                let empty = Vec::new();
                let rooms_data = rooms_resp
                    .get("data")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty);
                Self::build_device_to_room(rooms_data)
            }
        };

        // Get room names cache
        let room_names = {
            let cache = self
                .room_name_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock room_name cache"))?;
            cache.clone().unwrap_or_default()
        };

        // Fetch all devices
        let dev_resp = self.transport.get_resources(&self.username, "device")?;
        let empty = Vec::new();
        let dev_data = dev_resp
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        // Fetch zigbee_connectivity for MAC addresses
        let zigbee_mac_map = match self
            .transport
            .get_resources(&self.username, "zigbee_connectivity")
        {
            Ok(zigbee_resp) => {
                let empty_z = Vec::new();
                let zigbee_data = zigbee_resp
                    .get("data")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty_z);
                Self::build_zigbee_mac_map(zigbee_data)
            }
            Err(e) => {
                warn!(target: "hue_discovery", "Failed to fetch zigbee_connectivity: {}", e);
                HashMap::new()
            }
        };

        let identities =
            Self::extract_identities(dev_data, &device_to_room, &room_names, &zigbee_mac_map);
        let light_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Light)
            .count();
        let button_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Button)
            .count();
        let motion_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Motion)
            .count();
        info!(target: "hue_discovery",
            "Discovered {} device identities ({} lights, {} buttons, {} motion) from Hue bridge",
            identities.len(), light_count, button_count, motion_count);
        Ok(identities)
    }

    fn discover_configured_devices(&self) -> Result<Vec<(String, String)>> {
        let resp = self
            .transport
            .get_resources(&self.username, "behavior_instance")?;
        let empty = Vec::new();
        let data = resp
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        debug!(target: "hue_discovery",
            "behavior_instance response: {} entries, raw: {}",
            data.len(),
            serde_json::to_string_pretty(&resp).unwrap_or_default());

        let mut mappings = Vec::new();
        for bi in data {
            let behavior_id = match bi.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };

            // Try multiple paths to find the device reference.
            // Hue V2 behavior_instance JSON varies by behavior script type.
            let device_id = Self::extract_device_from_behavior(bi);

            match device_id {
                Some(dev_id) => {
                    debug!(target: "hue_discovery",
                        "behavior_instance {} -> device {}", behavior_id, dev_id);
                    mappings.push((behavior_id, dev_id));
                }
                None => {
                    debug!(target: "hue_discovery",
                        "behavior_instance {} — could not extract device reference",
                        behavior_id);
                }
            }
        }

        info!(target: "hue_discovery",
            "Discovered {} behavior_instances across {} devices from Hue bridge",
            mappings.len(),
            mappings.iter().map(|(_, d)| d.as_str()).collect::<std::collections::HashSet<_>>().len());
        Ok(mappings)
    }
}

/// Discover and register a single device on-demand from an SSE event.
///
/// When an unknown button_id or motion_id arrives via SSE, this function
/// fetches the individual resource and device resources from the Hue bridge
/// (tiny responses, ~200-500 bytes each) instead of the full device list (~50KB+).
///
/// Returns `Ok(true)` if the device was registered, `Ok(false)` if the device
/// is not in any known room (not an error — just a device we don't care about).
pub fn discover_device<H: HueTransport>(
    transport: &H,
    username: &str,
    resource_id: &str,
    resource_type: &str,
    registry: &Arc<Mutex<HubDeviceRegistry>>,
) -> Result<bool> {
    // Step 1: Fetch the single resource to get its owner device
    let resp = transport.get_resources(username, &format!("{}/{}", resource_type, resource_id))?;
    let data = resp
        .pointer("/data/0")
        .ok_or_else(|| anyhow::anyhow!("No data in {} response", resource_type))?;

    let device_id = data
        .pointer("/owner/rid")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("{} has no owner device", resource_type))?;

    // Step 2: Check if this device is in a known room
    let room_id = {
        let reg = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Registry lock failed"))?;
        reg.get_room_for_device(device_id)
    };

    let room_id = match room_id {
        Some(id) => id,
        None => {
            info!(target: "hue_discovery", "{} {} belongs to device {} which is not in any known room",
                resource_type, resource_id, device_id);
            return Ok(false);
        }
    };

    // Step 3: Fetch the full device resource to get all services
    let device_resp = transport.get_resources(username, &format!("device/{}", device_id))?;
    let device_data = device_resp
        .pointer("/data/0")
        .ok_or_else(|| anyhow::anyhow!("No data in device response"))?;

    let services = device_data
        .get("services")
        .and_then(|v| v.as_array())
        .ok_or_else(|| anyhow::anyhow!("Device has no services"))?;

    let mut buttons = Vec::new();
    let mut control_id: u8 = 1;
    let mut found_target = false;

    for svc in services {
        let rtype = svc.get("rtype").and_then(|v| v.as_str()).unwrap_or("");
        match rtype {
            "button" => {
                if let Some(rid) = svc.get("rid").and_then(|v| v.as_str()) {
                    buttons.push((rid.to_string(), control_id));
                    control_id += 1;
                    if rid == resource_id {
                        found_target = true;
                    }
                }
            }
            "motion" => {
                if let Some(rid) = svc.get("rid").and_then(|v| v.as_str()) {
                    // Register each motion service as a Motion device
                    let mut reg = registry
                        .lock()
                        .map_err(|_| anyhow::anyhow!("Registry lock failed"))?;
                    reg.upsert_device(rid, &room_id, &[], DeviceType::Motion);
                    if rid == resource_id {
                        found_target = true;
                    }
                }
            }
            _ => {}
        }
    }

    // For button resources, the target must have been found in button services.
    // For motion resources, the target must have been found in motion services.
    if !found_target {
        warn!(target: "hue_discovery", "Device {} has no {} service matching {}",
            device_id, resource_type, resource_id);
        return Ok(false);
    }

    // Register buttons if present on the same device
    if !buttons.is_empty() {
        let mut reg = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Registry lock failed"))?;
        reg.upsert_device(device_id, &room_id, &buttons, DeviceType::Button);
    }

    info!(target: "hue_discovery", "Discovered {} {} on device {} for room {}",
        resource_type, resource_id, device_id, room_id);
    Ok(true)
}
