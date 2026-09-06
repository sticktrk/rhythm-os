//! Hue V2 REST discovery implementation.
//!
//! Implements `HubDiscovery` by querying the Hue bridge V2 API for rooms
//! and typed devices (buttons + motion sensors).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use log::{debug, info, warn};

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::DeviceRegistry;
use rhythm_os::api_types::{LightCapabilitiesDto, LightColorTemperatureCapabilitiesDto};
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::discovery::{
    DiscoveredDevice, DiscoveredMotionState, DiscoveredRoom, HubDiscovery, ManagedSceneProjection,
};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::scenes::{
    native_scene_id, LightSceneColor, LightSceneLayer, LightSceneOutput, LightScenePower,
    SceneDefinition, SceneSource,
};

use crate::transport::HueTransport;

struct HueDiscoveryOwnershipSource {
    storage: Arc<dyn rhythm_os::storage::Storage>,
    bridge_id: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum HueDiscoveryMode {
    /// Import the bridge's own rooms, automations, and scenes during the
    /// pre-takeover discovery used to build the immutable baseline.
    #[default]
    BridgeNative,
    /// Expose only rooms whose explicit IDs are in the ownership manifest.
    /// Native Hue scenes attached to those rooms remain discoverable, while
    /// native automations remain hidden while Rhythm owns the bridge.
    RhythmAuthoritative { managed_room_ids: BTreeSet<String> },
}

/// Hue V2 discovery using the bridge REST API.
///
/// Caches the device→room mapping built during `discover_rooms()` so that
/// `discover_devices()` can skip re-fetching the rooms JSON.
/// On constrained blocking runtimes this avoids ~30-50KB of peak heap from
/// parsing rooms twice.
pub struct HueDiscovery<H: HueTransport> {
    transport: Arc<H>,
    username: String,
    /// Cached device_id → room_id mapping, populated by `discover_rooms()`.
    device_to_room_cache: Mutex<Option<HashMap<String, String>>>,
    /// Cached room_id → room_name mapping, populated by `discover_rooms()`.
    room_name_cache: Mutex<Option<HashMap<String, String>>>,
    /// Cached native device ID → normalized endpoint capability metadata,
    /// refreshed with each complete identity snapshot.
    endpoint_capabilities_cache: Mutex<HashMap<String, serde_json::Value>>,
    mode: Mutex<HueDiscoveryMode>,
    ownership_source: Option<HueDiscoveryOwnershipSource>,
    /// Bridge inventories shared by the managed scene projections of one
    /// batch (a whole-home apply). `None` outside a batch.
    managed_scene_batch: Mutex<Option<crate::managed_scenes::ManagedSceneBridgeCache>>,
}

impl<H: HueTransport> HueDiscovery<H> {
    pub fn new(transport: Arc<H>, username: String) -> Self {
        Self::new_with_mode(transport, username, HueDiscoveryMode::BridgeNative)
    }

    pub fn new_with_mode(transport: Arc<H>, username: String, mode: HueDiscoveryMode) -> Self {
        Self {
            transport,
            username,
            device_to_room_cache: Mutex::new(None),
            room_name_cache: Mutex::new(None),
            endpoint_capabilities_cache: Mutex::new(HashMap::new()),
            mode: Mutex::new(mode),
            ownership_source: None,
            managed_scene_batch: Mutex::new(None),
        }
    }

    /// Build discovery whose authoritative room allowlist is reloaded from the
    /// durable bridge-identity manifest on every synchronization. This prevents
    /// newly created or moved managed rooms from being hidden by a stale cache.
    pub fn new_with_ownership_storage(
        transport: Arc<H>,
        username: String,
        storage: Arc<dyn rhythm_os::storage::Storage>,
    ) -> Result<Self> {
        let bridge_id = crate::ownership::connected_hue_bridge_id(transport.as_ref(), &username)?;
        Ok(Self {
            transport,
            username,
            device_to_room_cache: Mutex::new(None),
            room_name_cache: Mutex::new(None),
            endpoint_capabilities_cache: Mutex::new(HashMap::new()),
            mode: Mutex::new(HueDiscoveryMode::BridgeNative),
            ownership_source: Some(HueDiscoveryOwnershipSource { storage, bridge_id }),
            managed_scene_batch: Mutex::new(None),
        })
    }

    pub fn new_authoritative(
        transport: Arc<H>,
        username: String,
        managed_room_ids: BTreeSet<String>,
    ) -> Self {
        Self::new_with_mode(
            transport,
            username,
            HueDiscoveryMode::RhythmAuthoritative { managed_room_ids },
        )
    }

    fn authoritative_room_ids(&self) -> Result<Option<BTreeSet<String>>> {
        if let Some(source) = &self.ownership_source {
            return Ok(crate::ownership::load_controller_ownership(
                source.storage.as_ref(),
                &source.bridge_id,
            )?
            .filter(|state| state.phase.is_managed())
            .map(|state| state.managed_room_ids()));
        }
        let mode = self
            .mode
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue discovery mode"))?;
        Ok(match &*mode {
            HueDiscoveryMode::BridgeNative => None,
            HueDiscoveryMode::RhythmAuthoritative { managed_room_ids } => {
                Some(managed_room_ids.clone())
            }
        })
    }

    fn managed_scene_ids(&self) -> Result<BTreeSet<String>> {
        let Some(source) = &self.ownership_source else {
            return Ok(BTreeSet::new());
        };
        Ok(
            crate::ownership::load_controller_ownership(
                source.storage.as_ref(),
                &source.bridge_id,
            )?
            .filter(|state| state.phase.is_managed())
            .map(|state| {
                state
                    .managed_scenes()
                    .values()
                    .map(|scene| scene.hue_scene_id.clone())
                    .collect()
            })
            .unwrap_or_default(),
        )
    }

    fn is_authoritative(&self) -> Result<bool> {
        Ok(self.authoritative_room_ids()?.is_some())
    }

    fn room_is_visible(managed_room_ids: Option<&BTreeSet<String>>, room_id: &str) -> bool {
        managed_room_ids.is_none_or(|managed_room_ids| managed_room_ids.contains(room_id))
    }

    /// Switch discovery scope after takeover/release and invalidate room caches.
    pub fn set_mode(&self, mode: HueDiscoveryMode) -> Result<()> {
        *self
            .mode
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue discovery mode"))? = mode;
        *self
            .device_to_room_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock device_to_room cache"))? = None;
        *self
            .room_name_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock room_name cache"))? = None;
        self.endpoint_capabilities_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock endpoint capability cache"))?
            .clear();
        Ok(())
    }

    /// Build device_id → room_id mapping from the room JSON array.
    fn build_device_to_room<'a>(
        rooms_data: impl IntoIterator<Item = &'a serde_json::Value>,
    ) -> HashMap<String, String> {
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

            // Roomless devices (not children of any Hue room) flow through with
            // `room_id: None` so the canonical pipeline can surface them for
            // user assignment.
            let room_id = device_to_room.get(&device_id).cloned();

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

    /// Extract current motion states from Hue V2 motion resources.
    fn extract_motion_states(
        motion_data: &[serde_json::Value],
        device_to_room: &HashMap<String, String>,
    ) -> Vec<DiscoveredMotionState> {
        let mut states = Vec::new();

        for motion_json in motion_data {
            let sensor_id = match motion_json.get("id").and_then(|v| v.as_str()) {
                Some(id) => id.to_string(),
                None => continue,
            };
            let owner = match motion_json.get("owner") {
                Some(owner) => owner,
                None => continue,
            };
            if owner.get("rtype").and_then(|v| v.as_str()) != Some("device") {
                continue;
            }
            let device_id = match owner.get("rid").and_then(|v| v.as_str()) {
                Some(id) => id,
                None => continue,
            };
            let room_id = device_to_room.get(device_id).cloned();
            let is_active = match motion_json
                .pointer("/motion/motion")
                .and_then(|v| v.as_bool())
            {
                Some(is_active) => is_active,
                None => continue,
            };

            states.push(DiscoveredMotionState {
                sensor_id,
                room_id,
                is_active,
            });
        }

        states
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

            // Roomless devices flow through with `room_id: None` so they
            // surface in the canonical registry for user assignment.
            let room_id = device_to_room.get(&device_id).cloned();
            let room_name = room_id
                .as_deref()
                .and_then(|rid| room_names.get(rid).cloned());

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

            // Motion sensors: align native_id on the motion *service* rid (the
            // id Hue's SSE protocol emits) so canonical lookup matches at event
            // time. The parent device rid is still used above for MAC lookup.
            let native_id = if device_type == DeviceType::Motion {
                services
                    .and_then(|svcs| {
                        svcs.iter()
                            .find(|s| s.get("rtype").and_then(|v| v.as_str()) == Some("motion"))
                            .and_then(|s| s.get("rid").and_then(|v| v.as_str()))
                            .map(String::from)
                    })
                    .unwrap_or_else(|| device_id.clone())
            } else {
                device_id
            };

            identities.push(DiscoveredIdentity {
                native_id,
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

    fn kelvin_from_mirek(mirek: u64) -> Option<u16> {
        if mirek == 0 {
            return None;
        }
        let kelvin = (1_000_000.0_f64 / mirek as f64).round() as u64;
        u16::try_from(kelvin)
            .ok()
            .filter(|kelvin| (500..=25_000).contains(kelvin))
    }

    /// Normalize Hue V2 light-resource capability metadata by owning device.
    ///
    /// A missing light resource or malformed mirek schema is "unknown" and is
    /// deliberately omitted. A complete light resource without a
    /// `color_temperature` member is an explicit brightness/color-only light
    /// and publishes an empty normalized CCT capability. Multi-light devices
    /// (for example Centris fixtures) expose the safe intersection across all
    /// of their light services.
    fn extract_endpoint_capabilities(
        device_data: &[serde_json::Value],
        light_data: &[serde_json::Value],
    ) -> HashMap<String, serde_json::Value> {
        let light_resources = light_data
            .iter()
            .filter_map(|light| {
                light
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .map(|id| (id, light))
            })
            .collect::<HashMap<_, _>>();
        let mut normalized = HashMap::new();

        for device in device_data {
            let Some(device_id) = device.get("id").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let light_service_ids = device
                .get("services")
                .and_then(serde_json::Value::as_array)
                .into_iter()
                .flatten()
                .filter(|service| {
                    service.get("rtype").and_then(serde_json::Value::as_str) == Some("light")
                })
                .filter_map(|service| service.get("rid").and_then(serde_json::Value::as_str))
                .collect::<Vec<_>>();
            if light_service_ids.is_empty() {
                continue;
            }

            let mut complete = true;
            let mut all_support_color_temperature = true;
            let mut supports_xy_color = false;
            let mut intersection: Option<LightColorTemperatureCapabilitiesDto> = None;
            for service_id in light_service_ids {
                let Some(light) = light_resources.get(service_id) else {
                    complete = false;
                    break;
                };
                supports_xy_color |= light.get("color").is_some();
                let Some(color_temperature) = light.get("color_temperature") else {
                    all_support_color_temperature = false;
                    intersection = None;
                    continue;
                };
                let Some(minimum_mirek) = color_temperature
                    .pointer("/mirek_schema/mirek_minimum")
                    .and_then(serde_json::Value::as_u64)
                else {
                    complete = false;
                    break;
                };
                let Some(maximum_mirek) = color_temperature
                    .pointer("/mirek_schema/mirek_maximum")
                    .and_then(serde_json::Value::as_u64)
                else {
                    complete = false;
                    break;
                };
                if minimum_mirek > maximum_mirek {
                    complete = false;
                    break;
                }
                let Some(min_kelvin) = Self::kelvin_from_mirek(maximum_mirek) else {
                    complete = false;
                    break;
                };
                let Some(max_kelvin) = Self::kelvin_from_mirek(minimum_mirek) else {
                    complete = false;
                    break;
                };
                if min_kelvin > max_kelvin {
                    complete = false;
                    break;
                }
                if all_support_color_temperature {
                    intersection = Some(match intersection {
                        Some(current) => LightColorTemperatureCapabilitiesDto {
                            min_kelvin: current.min_kelvin.max(min_kelvin),
                            max_kelvin: current.max_kelvin.min(max_kelvin),
                        },
                        None => LightColorTemperatureCapabilitiesDto {
                            min_kelvin,
                            max_kelvin,
                        },
                    });
                }
            }

            if !complete {
                continue;
            }
            let color_temperature = if all_support_color_temperature {
                intersection.filter(|range| range.min_kelvin <= range.max_kelvin)
            } else {
                None
            };
            if all_support_color_temperature && color_temperature.is_none() {
                continue;
            }
            normalized.insert(
                device_id.to_string(),
                serde_json::json!({
                    "light_capabilities": LightCapabilitiesDto {
                        color_temperature,
                        individual_profile_overrides: None,
                    },
                    "automatic_naming": {
                        "color_kind": if supports_xy_color { "color" } else { "white" }
                    }
                }),
            );
        }

        normalized
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

    /// Refuse to publish an identity snapshot that cannot account for the
    /// bridge topology used by room synchronization. Hue resource collection
    /// responses are complete snapshots, so a room child missing from the
    /// device response (or a light missing its durable Zigbee identity) is a
    /// partial read rather than evidence that the device disappeared.
    fn validate_identity_completeness(
        device_data: &[serde_json::Value],
        device_to_room: &HashMap<String, String>,
        zigbee_mac_map: &HashMap<String, String>,
        identities: &[DiscoveredIdentity],
    ) -> Result<()> {
        let device_ids = device_data
            .iter()
            .filter_map(|device| device.get("id").and_then(serde_json::Value::as_str))
            .collect::<BTreeSet<_>>();

        if device_to_room
            .keys()
            .any(|device_id| !device_ids.contains(device_id.as_str()))
        {
            anyhow::bail!(
                "Hue identity discovery is incomplete: a room child is absent from the device response"
            );
        }

        if zigbee_mac_map
            .keys()
            .any(|device_id| !device_ids.contains(device_id.as_str()))
        {
            anyhow::bail!(
                "Hue identity discovery is incomplete: a Zigbee device is absent from the device response"
            );
        }

        let discovered_light_ids = identities
            .iter()
            .filter(|identity| identity.device_type == DeviceType::Light)
            .map(|identity| identity.native_id.as_str())
            .collect::<BTreeSet<_>>();
        for device in device_data {
            if Self::infer_device_type(device.get("services").and_then(|value| value.as_array()))
                != Some(DeviceType::Light)
            {
                continue;
            }
            let device_id = device
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "Hue identity discovery is incomplete: a light has no device identity"
                    )
                })?;
            if !discovered_light_ids.contains(device_id) {
                anyhow::bail!(
                    "Hue identity discovery is incomplete: a light is missing durable Zigbee identity"
                );
            }
        }

        Ok(())
    }

    fn scene_output(action: &serde_json::Value) -> Option<LightSceneOutput> {
        let power = match action.pointer("/on/on").and_then(|value| value.as_bool()) {
            Some(false) => LightScenePower::Off,
            _ => LightScenePower::On,
        };
        let brightness = action
            .pointer("/dimming/brightness")
            .and_then(|value| value.as_f64())
            .unwrap_or(100.0)
            .round()
            .clamp(1.0, 100.0) as u8;
        let color = action
            .pointer("/color/xy")
            .and_then(|xy| Some((xy.get("x")?.as_f64()? as f32, xy.get("y")?.as_f64()? as f32)))
            .map(|(x, y)| LightSceneColor::Xy {
                xy: rhythm_core::XyColor::new(x, y),
            })
            .or_else(|| {
                action
                    .pointer("/color_temperature/mirek")
                    .and_then(|value| value.as_u64())
                    .filter(|mirek| *mirek > 0)
                    .map(|mirek| LightSceneColor::Kelvin {
                        kelvin: ((1_000_000.0 / mirek as f64).round() as u16).clamp(500, 25_000),
                    })
            });

        if power == LightScenePower::On && color.is_none() {
            return None;
        }

        Some(LightSceneOutput {
            power,
            brightness,
            color,
            transition_ms: None,
        })
    }

    fn scene_palette_outputs(scene: &serde_json::Value) -> Vec<LightSceneOutput> {
        ["color", "color_temperature"]
            .into_iter()
            .flat_map(|kind| {
                scene
                    .pointer(&format!("/palette/{kind}"))
                    .and_then(|value| value.as_array())
                    .into_iter()
                    .flatten()
            })
            .filter_map(Self::scene_output)
            .collect()
    }

    fn scene_action_outputs(scene: &serde_json::Value) -> Vec<LightSceneOutput> {
        scene
            .get("actions")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter_map(|entry| entry.get("action").and_then(Self::scene_output))
            .collect()
    }

    fn scenes_for_room(
        response: &serde_json::Value,
        room_id: &str,
        excluded_scene_ids: &BTreeSet<String>,
    ) -> Vec<SceneDefinition> {
        let mut scenes: Vec<_> = response
            .get("data")
            .and_then(|value| value.as_array())
            .into_iter()
            .flatten()
            .filter(|scene| {
                scene
                    .pointer("/group/rtype")
                    .and_then(|value| value.as_str())
                    == Some("room")
                    && scene.pointer("/group/rid").and_then(|value| value.as_str()) == Some(room_id)
            })
            .filter_map(|scene| {
                let external_id = scene.get("id").and_then(|value| value.as_str())?;
                if excluded_scene_ids.contains(external_id) {
                    return None;
                }
                let name = scene
                    .pointer("/metadata/name")
                    .and_then(|value| value.as_str())
                    .unwrap_or("Hue scene")
                    .trim();
                let native_palette = Self::scene_palette_outputs(scene);
                let is_palette_scene = !native_palette.is_empty();
                let palette = if is_palette_scene {
                    native_palette
                } else {
                    Self::scene_action_outputs(scene)
                };
                if palette
                    .iter()
                    .all(|output| output.power == LightScenePower::Off)
                {
                    return None;
                }

                let mut extensions = [(
                    "hue_room_id".to_string(),
                    serde_json::Value::String(room_id.to_string()),
                )]
                .into_iter()
                .collect::<std::collections::BTreeMap<_, _>>();
                if is_palette_scene {
                    extensions.insert(
                        "hue_palette_scene".to_string(),
                        serde_json::Value::Bool(true),
                    );
                }

                Some(SceneDefinition {
                    id: native_scene_id("hue", external_id),
                    name: if name.is_empty() {
                        "Hue scene".to_string()
                    } else {
                        name.to_string()
                    },
                    description: Some("From Philips Hue".to_string()),
                    source: SceneSource::Imported {
                        provider: "hue".to_string(),
                        external_id: Some(external_id.to_string()),
                    },
                    light: Some(LightSceneLayer {
                        default_transition_ms: None,
                        default_output: palette
                            .iter()
                            .find(|output| output.power == LightScenePower::On)
                            .cloned(),
                        palette,
                        entries: Vec::new(),
                    }),
                    extensions,
                })
            })
            .collect();
        scenes.sort_by(|left, right| {
            left.name
                .to_lowercase()
                .cmp(&right.name.to_lowercase())
                .then_with(|| left.id.cmp(&right.id))
        });
        let palette_count = scenes
            .iter()
            .filter(|scene| {
                scene.extensions.get("hue_palette_scene") == Some(&serde_json::Value::Bool(true))
            })
            .count();
        debug!(
            "Hue scene catalog classified: palette={}, other={}",
            palette_count,
            scenes.len().saturating_sub(palette_count)
        );
        scenes
    }
}

impl<H: HueTransport> HueDiscovery<H> {
    /// Return cached device_id -> room_id mapping, fetching rooms on demand.
    fn cached_or_fetch_device_to_room(&self) -> Result<HashMap<String, String>> {
        let managed_room_ids = self.authoritative_room_ids()?;
        let device_to_room = if self.ownership_source.is_some() {
            None
        } else {
            let cache = self
                .device_to_room_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock device_to_room cache"))?;
            cache.clone()
        };

        match device_to_room {
            Some(cached) => {
                info!(target: "hue_discovery", "Using cached device→room mapping ({} entries)", cached.len());
                Ok(cached)
            }
            None => {
                let rooms_resp = self.transport.get_resources(&self.username, "room")?;
                let empty = Vec::new();
                let rooms_data = rooms_resp
                    .get("data")
                    .and_then(|v| v.as_array())
                    .unwrap_or(&empty);
                Ok(Self::build_device_to_room(rooms_data.iter().filter(
                    |room| {
                        room.get("id")
                            .and_then(serde_json::Value::as_str)
                            .is_some_and(|room_id| {
                                Self::room_is_visible(managed_room_ids.as_ref(), room_id)
                            })
                    },
                )))
            }
        }
    }

    /// Shared implementation: fetch devices and extract typed devices.
    ///
    /// Uses the device→room cache if populated by a prior `discover_rooms()` call,
    /// avoiding a second rooms JSON fetch (~30-50KB saved on constrained
    /// blocking runtimes). Falls back to fetching rooms if the cache is empty.
    fn fetch_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        let device_to_room = self.cached_or_fetch_device_to_room()?;

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
    /// Verified against live Hue bridge responses. The structure is:
    /// ```text
    /// { "configuration": { "device": { "rid": "...", "rtype": "device" } },
    ///   "dependees": [{ "target": { "rid": "...", "rtype": "device" } }, ...] }
    /// ```
    ///
    /// Primary: `configuration.device.rid` (direct device reference).
    /// Fallback: first `dependees[].target` with `rtype == "device"`.
    fn extract_device_from_behavior(bi: &serde_json::Value) -> Option<String> {
        // Primary: configuration.device.rid
        if let Some(rid) = bi
            .pointer("/configuration/device/rid")
            .and_then(|v| v.as_str())
        {
            return Some(rid.to_string());
        }

        // Fallback: dependees[] where target.rtype == "device"
        if let Some(dependees) = bi.get("dependees").and_then(|v| v.as_array()) {
            for dep in dependees {
                if let Some(target) = dep.get("target") {
                    if target.get("rtype").and_then(|v| v.as_str()) == Some("device") {
                        if let Some(rid) = target.get("rid").and_then(|v| v.as_str()) {
                            return Some(rid.to_string());
                        }
                    }
                }
            }
        }

        None
    }
}

impl<H: HueTransport + 'static> HubDiscovery for HueDiscovery<H> {
    fn room_names_are_authoritative(&self) -> bool {
        false
    }

    fn release_resources(&self) {
        self.transport.release_connection();
    }

    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        let managed_room_ids = self.authoritative_room_ids()?;
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
            if !Self::room_is_visible(managed_room_ids.as_ref(), &id) {
                continue;
            }

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

    fn discover_scenes(&self, room_id: &str) -> Result<Vec<SceneDefinition>> {
        let response = self.transport.get_resources(&self.username, "scene")?;
        let managed_scene_ids = self.managed_scene_ids()?;
        let scenes = Self::scenes_for_room(&response, room_id, &managed_scene_ids);
        info!(
            target: "hue_scenes",
            "Discovered {} Hue scenes for room {}",
            scenes.len(),
            room_id
        );
        Ok(scenes)
    }

    fn recall_scene(&self, scene_id: &str, transition_ms: Option<u32>) -> Result<()> {
        self.transport
            .recall_scene(&self.username, scene_id, transition_ms)
    }

    fn apply_managed_scene_projection(
        &self,
        projection: &ManagedSceneProjection,
        transition_ms: Option<u32>,
        ephemeral: bool,
    ) -> Result<bool> {
        let Some(source) = &self.ownership_source else {
            return Ok(false);
        };
        let operation_lock = crate::ownership::controller_operation_lock(&source.bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        let mut ownership = crate::ownership::load_controller_ownership(
            source.storage.as_ref(),
            &source.bridge_id,
        )?
        .filter(|ownership| ownership.phase == crate::ownership::HueOwnershipPhase::Active)
        .ok_or_else(|| anyhow::anyhow!("Hue controller authority is not active"))?;
        let mut batch = self
            .managed_scene_batch
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue managed scene batch cache"))?;
        crate::managed_scenes::apply_managed_scene(
            source.storage.as_ref(),
            &mut ownership,
            self.transport.as_ref(),
            &self.username,
            projection,
            transition_ms,
            ephemeral,
            batch.as_mut(),
        )?;
        Ok(true)
    }

    fn begin_managed_scene_batch(&self) {
        if let Ok(mut batch) = self.managed_scene_batch.lock() {
            *batch = Some(crate::managed_scenes::ManagedSceneBridgeCache::default());
        }
    }

    fn end_managed_scene_batch(&self) {
        if let Ok(mut batch) = self.managed_scene_batch.lock() {
            *batch = None;
        }
    }

    fn delete_managed_scene_projection(&self, scene_id: &str) -> Result<bool> {
        let Some(source) = &self.ownership_source else {
            return Ok(false);
        };
        let operation_lock = crate::ownership::controller_operation_lock(&source.bridge_id);
        let _operation = operation_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock Hue controller operations"))?;
        let mut ownership = crate::ownership::load_controller_ownership(
            source.storage.as_ref(),
            &source.bridge_id,
        )?
        .filter(|ownership| ownership.phase == crate::ownership::HueOwnershipPhase::Active)
        .ok_or_else(|| anyhow::anyhow!("Hue controller authority is not active"))?;
        crate::managed_scenes::delete_managed_scene(
            source.storage.as_ref(),
            &mut ownership,
            self.transport.as_ref(),
            &self.username,
            scene_id,
        )?;
        Ok(true)
    }

    /// Discover all devices with full hardware identity information.
    ///
    /// Returns identities for ALL device types (lights, buttons, motion) with
    /// names, manufacturer/model, and MAC addresses from zigbee_connectivity.
    /// Used by the canonical device registry for cross-hub deduplication.
    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        self.endpoint_capabilities_cache
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock endpoint capability cache"))?
            .clear();
        let device_to_room = self.cached_or_fetch_device_to_room()?;

        // Get room names cache
        let room_names = {
            let cache = self
                .room_name_cache
                .lock()
                .map_err(|_| anyhow::anyhow!("Failed to lock room_name cache"))?;
            cache.clone().unwrap_or_default()
        };

        // Fetch all devices
        let dev_resp = self
            .transport
            .get_resources(&self.username, "device")
            .context("Failed to fetch Hue device identities")?;
        let dev_data = dev_resp
            .get("data")
            .and_then(|v| v.as_array())
            .ok_or_else(|| anyhow::anyhow!("No data array in Hue device identity response"))?;

        // Fetch zigbee_connectivity for MAC addresses
        let zigbee_resp = self
            .transport
            .get_resources(&self.username, "zigbee_connectivity")
            .context("Failed to fetch Hue Zigbee identities")?;
        let zigbee_data = zigbee_resp
            .get("data")
            .and_then(|value| value.as_array())
            .ok_or_else(|| anyhow::anyhow!("No data array in Hue Zigbee identity response"))?;
        let zigbee_mac_map = Self::build_zigbee_mac_map(zigbee_data);

        let identities =
            Self::extract_identities(dev_data, &device_to_room, &room_names, &zigbee_mac_map);
        Self::validate_identity_completeness(
            dev_data,
            &device_to_room,
            &zigbee_mac_map,
            &identities,
        )?;
        let light_count = identities
            .iter()
            .filter(|i| i.device_type == DeviceType::Light)
            .count();
        if light_count > 0 {
            match self.transport.get_resources(&self.username, "light") {
                Ok(response) => match response.get("data").and_then(serde_json::Value::as_array) {
                    Some(light_data) => {
                        *self.endpoint_capabilities_cache.lock().map_err(|_| {
                            anyhow::anyhow!("Failed to lock endpoint capability cache")
                        })? = Self::extract_endpoint_capabilities(dev_data, light_data);
                    }
                    None => warn!(target: "hue_discovery",
                        "Hue light capability response had no data array; retaining unknown capability state"),
                },
                Err(error) => warn!(target: "hue_discovery",
                    "Hue light capability discovery failed; retaining unknown capability state: {}",
                    error),
            }
        }
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

    fn endpoint_capabilities(&self, native_id: &str) -> Option<serde_json::Value> {
        self.endpoint_capabilities_cache
            .lock()
            .ok()
            .and_then(|capabilities| capabilities.get(native_id).cloned())
    }

    fn discover_motion_state(&self) -> Result<Vec<DiscoveredMotionState>> {
        let device_to_room = self.cached_or_fetch_device_to_room()?;
        let resp = self.transport.get_resources(&self.username, "motion")?;
        let empty = Vec::new();
        let data = resp
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        let states = Self::extract_motion_states(data, &device_to_room);
        let active_count = states.iter().filter(|s| s.is_active).count();
        info!(target: "hue_discovery",
            "Discovered {} motion states ({} active) from Hue bridge",
            states.len(), active_count);
        Ok(states)
    }

    fn discover_configured_devices(&self) -> Result<Vec<(String, String)>> {
        if self.is_authoritative()? {
            return Ok(Vec::new());
        }
        let resp = self
            .transport
            .get_resources(&self.username, "behavior_instance")?;
        let empty = Vec::new();
        let data = resp
            .get("data")
            .and_then(|v| v.as_array())
            .unwrap_or(&empty);

        debug!(target: "hue_discovery", "behavior_instance response: {} entries", data.len());

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
/// Roomless devices are still registered (their button/type mappings are needed
/// so events can be identified) but with `room_id: None` — routing remains
/// unrouted until the user assigns a Rhythm room.
///
/// Returns `Ok(true)` if the device was registered, `Ok(false)` if the target
/// service was not found on the device.
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

    // Step 2: Look up the device's room (None for roomless devices).
    let room_id = {
        let reg = registry
            .lock()
            .map_err(|_| anyhow::anyhow!("Registry lock failed"))?;
        reg.get_room_for_device(device_id)
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
                    reg.upsert_device(rid, room_id.as_deref(), &[], DeviceType::Motion);
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
        reg.upsert_device(device_id, room_id.as_deref(), &buttons, DeviceType::Button);
    }

    match room_id.as_deref() {
        Some(rid) => info!(target: "hue_discovery", "Discovered {} {} on device {} for room {}",
            resource_type, resource_id, device_id, rid),
        None => {
            info!(target: "hue_discovery", "Discovered {} {} on roomless device {} — awaiting room assignment",
            resource_type, resource_id, device_id)
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct StaticHueTransport {
        resources: HashMap<String, serde_json::Value>,
        failed_resources: BTreeSet<String>,
    }

    impl StaticHueTransport {
        fn with_resource(mut self, resource_type: &str, response: serde_json::Value) -> Self {
            self.resources.insert(resource_type.to_string(), response);
            self
        }

        fn failing_resource(mut self, resource_type: &str) -> Self {
            self.failed_resources.insert(resource_type.to_string());
            self
        }
    }

    impl crate::transport::HueTransport for StaticHueTransport {
        fn test_connection(&self, _username: &str) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn warmup_tls(&self) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_grouped_light(
            &self,
            _username: &str,
            _grouped_light_id: &str,
            _on: bool,
            _brightness: Option<u8>,
            _kelvin: Option<u16>,
            _xy: Option<(f32, f32)>,
            _fade_ms: Option<u16>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn set_light(
            &self,
            _username: &str,
            _light_id: &str,
            _on: bool,
            _brightness: Option<u8>,
            _kelvin: Option<u16>,
            _xy: Option<(f32, f32)>,
            _fade_ms: Option<u16>,
        ) -> anyhow::Result<()> {
            Ok(())
        }

        fn is_grouped_light_on(
            &self,
            _username: &str,
            _grouped_light_id: &str,
        ) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn is_light_on(&self, _username: &str, _light_id: &str) -> anyhow::Result<bool> {
            Ok(false)
        }

        fn identify_light(&self, _username: &str, _light_id: &str) -> anyhow::Result<()> {
            Ok(())
        }

        fn get_resources(
            &self,
            _username: &str,
            resource_type: &str,
        ) -> anyhow::Result<serde_json::Value> {
            if self.failed_resources.contains(resource_type) {
                anyhow::bail!("injected Hue resource failure");
            }
            Ok(self
                .resources
                .get(resource_type)
                .cloned()
                .unwrap_or_else(|| serde_json::json!({ "data": [] })))
        }

        fn recall_scene(
            &self,
            _username: &str,
            _scene_id: &str,
            _transition_ms: Option<u32>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn discovery_maps_rooms_devices_identities_and_configured_behaviors() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "room",
                serde_json::json!({
                    "data": [
                        {
                            "id": "room-1",
                            "metadata": { "name": "Kitchen" },
                            "children": [
                                { "rtype": "device", "rid": "dev-light" },
                                { "rtype": "device", "rid": "dev-button" },
                                { "rtype": "device", "rid": "dev-motion" }
                            ],
                            "services": [
                                { "rtype": "grouped_light", "rid": "grouped-kitchen" }
                            ]
                        },
                        {
                            "id": "room-without-grouped-light",
                            "metadata": { "name": "Ignored" },
                            "children": [
                                { "rtype": "device", "rid": "ignored-device" }
                            ],
                            "services": []
                        }
                    ]
                }),
            )
            .with_resource(
                "device",
                serde_json::json!({
                    "data": [
                        {
                            "id": "dev-light",
                            "metadata": { "name": "Kitchen Ceiling" },
                            "product_data": {
                                "manufacturer_name": "Signify",
                                "model_id": "LCA001"
                            },
                            "services": [
                                { "rtype": "light", "rid": "light-1" }
                            ]
                        },
                        {
                            "id": "dev-button",
                            "metadata": { "name": "Kitchen Dimmer" },
                            "product_data": {
                                "manufacturer_name": "Signify",
                                "model_id": "RWL022"
                            },
                            "services": [
                                { "rtype": "button", "rid": "button-1" },
                                { "rtype": "button", "rid": "button-2" }
                            ]
                        },
                        {
                            "id": "dev-motion",
                            "metadata": { "name": "Kitchen Motion" },
                            "product_data": {
                                "manufacturer_name": "Signify",
                                "model_id": "SML004"
                            },
                            "services": [
                                { "rtype": "motion", "rid": "motion-1" }
                            ]
                        },
                        {
                            "id": "dev-bridge",
                            "metadata": { "name": "Bridge" },
                            "services": [
                                { "rtype": "bridge", "rid": "bridge-1" }
                            ]
                        }
                    ]
                }),
            )
            .with_resource(
                "zigbee_connectivity",
                serde_json::json!({
                    "data": [
                        {
                            "owner": { "rtype": "device", "rid": "dev-light" },
                            "mac_address": "00:17:88:01:00:00:01"
                        },
                        {
                            "owner": { "rtype": "device", "rid": "dev-button" },
                            "mac_address": "00:17:88:01:00:00:02"
                        },
                        {
                            "owner": { "rtype": "device", "rid": "dev-motion" },
                            "mac_address": "00:17:88:01:00:00:03"
                        }
                    ]
                }),
            )
            .with_resource(
                "light",
                serde_json::json!({
                    "data": [{
                        "id": "light-1",
                        "owner": { "rtype": "device", "rid": "dev-light" },
                        "color_temperature": {
                            "mirek": 250,
                            "mirek_valid": true,
                            "mirek_schema": {
                                "mirek_minimum": 153,
                                "mirek_maximum": 500
                            }
                        }
                    }]
                }),
            )
            .with_resource(
                "behavior_instance",
                serde_json::json!({
                    "data": [
                        {
                            "id": "behavior-direct",
                            "configuration": {
                                "device": { "rtype": "device", "rid": "dev-button" }
                            }
                        },
                        {
                            "id": "behavior-dependee",
                            "configuration": {},
                            "dependees": [
                                { "target": { "rtype": "room", "rid": "room-1" } },
                                { "target": { "rtype": "device", "rid": "dev-motion" } }
                            ]
                        },
                        {
                            "id": "behavior-unmapped",
                            "configuration": {},
                            "dependees": []
                        }
                    ]
                }),
            );
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let rooms = discovery.discover_rooms().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].id, "room-1");
        assert_eq!(rooms[0].name, "Kitchen");
        assert_eq!(rooms[0].grouped_light_id, "grouped-kitchen");
        assert_eq!(
            rooms[0].device_ids,
            vec![
                "dev-light".to_string(),
                "dev-button".to_string(),
                "dev-motion".to_string()
            ]
        );

        let devices = discovery.discover_devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Button
                && device.device_id == "dev-button"
                && device.room_id.as_deref() == Some("room-1")
                && device.buttons == vec![("button-1".to_string(), 1), ("button-2".to_string(), 2)]
        }));
        assert!(devices.iter().any(|device| {
            device.device_type == DeviceType::Motion
                && device.device_id == "motion-1"
                && device.room_id.as_deref() == Some("room-1")
        }));

        let identities = discovery.discover_identities().unwrap();
        assert_eq!(identities.len(), 3);
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Light
                && identity.native_id == "dev-light"
                && identity.room_name.as_deref() == Some("Kitchen")
                && identity.hardware_ids == vec![HardwareId::mac("00:17:88:01:00:00:01")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Button
                && identity.native_id == "dev-button"
                && identity.hardware_ids == vec![HardwareId::mac("00:17:88:01:00:00:02")]
        }));
        assert!(identities.iter().any(|identity| {
            identity.device_type == DeviceType::Motion
                && identity.native_id == "motion-1"
                && identity.hardware_ids == vec![HardwareId::mac("00:17:88:01:00:00:03")]
        }));
        let light_capabilities = discovery
            .endpoint_capabilities("dev-light")
            .expect("Hue V2 light schema should be normalized by device identity");
        assert_eq!(
            light_capabilities.pointer("/light_capabilities/color_temperature"),
            Some(&serde_json::json!({
                "min_kelvin": 2000,
                "max_kelvin": 6536
            }))
        );

        let configured = discovery.discover_configured_devices().unwrap();
        assert_eq!(
            configured,
            vec![
                ("behavior-direct".to_string(), "dev-button".to_string()),
                ("behavior-dependee".to_string(), "dev-motion".to_string())
            ]
        );
    }

    #[test]
    fn hue_light_capabilities_intersect_multi_service_and_preserve_unknown() {
        let devices = serde_json::json!([
            {
                "id": "multi-light-device",
                "services": [
                    {"rtype": "light", "rid": "light-wide"},
                    {"rtype": "light", "rid": "light-narrow"}
                ]
            },
            {
                "id": "brightness-only-device",
                "services": [{"rtype": "light", "rid": "light-white"}]
            },
            {
                "id": "xy-only-device",
                "services": [{"rtype": "light", "rid": "light-color"}]
            },
            {
                "id": "missing-resource-device",
                "services": [{"rtype": "light", "rid": "light-missing"}]
            },
            {
                "id": "malformed-schema-device",
                "services": [{"rtype": "light", "rid": "light-malformed"}]
            }
        ]);
        let lights = serde_json::json!([
            {
                "id": "light-wide",
                "color_temperature": {
                    "mirek_schema": {"mirek_minimum": 153, "mirek_maximum": 500}
                }
            },
            {
                "id": "light-narrow",
                "color_temperature": {
                    "mirek_schema": {"mirek_minimum": 200, "mirek_maximum": 454}
                }
            },
            {"id": "light-white", "dimming": {"brightness": 42.0}},
            {
                "id": "light-color",
                "color": {
                    "xy": {"x": 0.3, "y": 0.3},
                    "gamut_type": "C"
                }
            },
            {"id": "light-malformed", "color_temperature": {"mirek": 250}}
        ]);

        let capabilities = HueDiscovery::<StaticHueTransport>::extract_endpoint_capabilities(
            devices.as_array().unwrap(),
            lights.as_array().unwrap(),
        );

        assert_eq!(
            capabilities["multi-light-device"].pointer("/light_capabilities/color_temperature"),
            Some(&serde_json::json!({
                "min_kelvin": 2203,
                "max_kelvin": 5000
            }))
        );
        assert_eq!(
            capabilities["brightness-only-device"],
            serde_json::json!({
                "light_capabilities": {},
                "automatic_naming": {"color_kind": "white"}
            }),
            "a complete Hue light resource without color_temperature is explicitly non-CT"
        );
        assert_eq!(
            capabilities["xy-only-device"],
            serde_json::json!({
                "light_capabilities": {},
                "automatic_naming": {"color_kind": "color"}
            }),
            "XY color is retained so the shared resolver can expose CCT-curve fallback"
        );
        assert!(
            !capabilities.contains_key("missing-resource-device"),
            "a partial light snapshot must remain unknown"
        );
        assert!(
            !capabilities.contains_key("malformed-schema-device"),
            "a malformed schema must remain unknown rather than becoming unsupported"
        );
    }

    #[test]
    fn identity_discovery_keeps_working_when_light_capability_fetch_fails() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "device",
                serde_json::json!({"data": [{
                    "id": "light-device",
                    "metadata": {"name": "Future Hue"},
                    "product_data": {
                        "manufacturer_name": "Signify Netherlands B.V.",
                        "model_id": "FUTURE-HUE"
                    },
                    "services": [{"rtype": "light", "rid": "light-service"}]
                }]}),
            )
            .with_resource(
                "zigbee_connectivity",
                serde_json::json!({"data": [{
                    "owner": {"rtype": "device", "rid": "light-device"},
                    "mac_address": "00:17:88:01:00:00:09"
                }]}),
            )
            .failing_resource("light");
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let identities = discovery.discover_identities().unwrap();

        assert_eq!(identities.len(), 1);
        assert_eq!(identities[0].native_id, "light-device");
        assert!(discovery.endpoint_capabilities("light-device").is_none());
    }

    #[test]
    fn identity_discovery_rejects_missing_or_non_array_device_data() {
        for malformed_response in [
            serde_json::json!({}),
            serde_json::json!({"data": null}),
            serde_json::json!({"data": {}}),
        ] {
            let transport = StaticHueTransport::default()
                .with_resource("device", malformed_response)
                .with_resource("zigbee_connectivity", serde_json::json!({"data": []}));
            let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

            let error = discovery.discover_identities().unwrap_err();

            assert!(
                error
                    .to_string()
                    .contains("No data array in Hue device identity response"),
                "unexpected error: {error:#}"
            );
        }
    }

    #[test]
    fn identity_discovery_propagates_zigbee_connectivity_failure() {
        let transport = StaticHueTransport::default()
            .with_resource("device", serde_json::json!({"data": []}))
            .failing_resource("zigbee_connectivity");
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let error = discovery.discover_identities().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("Failed to fetch Hue Zigbee identities"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn identity_discovery_accepts_complete_empty_snapshots() {
        let transport = StaticHueTransport::default()
            .with_resource("room", serde_json::json!({"data": []}))
            .with_resource("device", serde_json::json!({"data": []}))
            .with_resource("zigbee_connectivity", serde_json::json!({"data": []}));
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let identities = discovery.discover_identities().unwrap();

        assert!(identities.is_empty());
    }

    #[test]
    fn identity_discovery_rejects_room_child_missing_from_device_snapshot() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "room",
                serde_json::json!({"data": [{
                    "id": "room-1",
                    "children": [{"rtype": "device", "rid": "missing-device"}],
                    "services": [{"rtype": "grouped_light", "rid": "grouped-1"}]
                }]}),
            )
            .with_resource("device", serde_json::json!({"data": []}))
            .with_resource("zigbee_connectivity", serde_json::json!({"data": []}));
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let error = discovery.discover_identities().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("a room child is absent from the device response"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn identity_discovery_rejects_light_without_durable_zigbee_identity() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "room",
                serde_json::json!({"data": [{
                    "id": "room-1",
                    "children": [{"rtype": "device", "rid": "light-device"}],
                    "services": [{"rtype": "grouped_light", "rid": "grouped-1"}]
                }]}),
            )
            .with_resource(
                "device",
                serde_json::json!({"data": [{
                    "id": "light-device",
                    "services": [{"rtype": "light", "rid": "light-service"}]
                }]}),
            )
            .with_resource("zigbee_connectivity", serde_json::json!({"data": []}));
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let error = discovery.discover_identities().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("a light is missing durable Zigbee identity"),
            "unexpected error: {error:#}"
        );
    }

    #[test]
    fn scene_discovery_filters_to_room_and_projects_hue_palette() {
        let response = serde_json::json!({
            "data": [
                {
                    "id": "scene-cool",
                    "metadata": {"name": "Arctic aurora"},
                    "group": {"rid": "room-1", "rtype": "room"},
                    "palette": {
                        "color": [
                            {
                                "color": {"xy": {"x": 0.21, "y": 0.24}},
                                "dimming": {"brightness": 63.4}
                            }
                        ],
                        "color_temperature": [
                            {
                                "color_temperature": {"mirek": 250},
                                "dimming": {"brightness": 40}
                            }
                        ]
                    },
                    "actions": [
                        {
                            "target": {"rid": "light-1", "rtype": "light"},
                            "action": {
                                "on": {"on": true},
                                "dimming": {"brightness": 63.4},
                                "color": {"xy": {"x": 0.21, "y": 0.24}}
                            }
                        },
                        {
                            "target": {"rid": "light-2", "rtype": "light"},
                            "action": {
                                "on": {"on": true},
                                "dimming": {"brightness": 40},
                                "color_temperature": {"mirek": 250}
                            }
                        }
                    ]
                },
                {
                    "id": "scene-static",
                    "metadata": {"name": "Ordinary static scene"},
                    "group": {"rid": "room-1", "rtype": "room"},
                    "actions": [{
                        "action": {
                            "on": {"on": true},
                            "color_temperature": {"mirek": 300}
                        }
                    }]
                },
                {
                    "id": "scene-other-room",
                    "metadata": {"name": "Other"},
                    "group": {"rid": "room-2", "rtype": "room"},
                    "actions": [{
                        "action": {
                            "on": {"on": true},
                            "color_temperature": {"mirek": 300}
                        }
                    }]
                },
                {
                    "id": "scene-zone",
                    "metadata": {"name": "Zone"},
                    "group": {"rid": "room-1", "rtype": "zone"},
                    "actions": [{
                        "action": {
                            "on": {"on": true},
                            "color_temperature": {"mirek": 300}
                        }
                    }]
                }
            ]
        });

        let scenes = HueDiscovery::<StaticHueTransport>::scenes_for_room(
            &response,
            "room-1",
            &BTreeSet::new(),
        );

        assert_eq!(scenes.len(), 2);
        let scene = &scenes[0];
        assert_eq!(scene.id, "native-hue-scene-cool");
        assert_eq!(scene.name, "Arctic aurora");
        assert_eq!(
            scene.source,
            SceneSource::Imported {
                provider: "hue".to_string(),
                external_id: Some("scene-cool".to_string()),
            }
        );
        let light = scene.light.as_ref().unwrap();
        assert_eq!(light.palette.len(), 2);
        assert_eq!(light.palette[0].brightness, 63);
        assert!(matches!(
            light.palette[0].color,
            Some(LightSceneColor::Xy { .. })
        ));
        assert_eq!(
            light.palette[1].color,
            Some(LightSceneColor::Kelvin { kelvin: 4000 })
        );
        assert_eq!(
            scene.extensions.get("hue_palette_scene"),
            Some(&serde_json::Value::Bool(true))
        );
        let ordinary = &scenes[1];
        assert_eq!(ordinary.id, "native-hue-scene-static");
        assert_eq!(ordinary.name, "Ordinary static scene");
        assert_eq!(
            ordinary
                .light
                .as_ref()
                .and_then(|light| light.palette.first())
                .and_then(|output| output.color.clone()),
            Some(LightSceneColor::Kelvin { kelvin: 3333 })
        );
        assert_eq!(
            ordinary.extensions.get("hue_palette_scene"),
            None,
            "ordinary Hue scenes must remain explicitly unmarked"
        );
        let filtered = HueDiscovery::<StaticHueTransport>::scenes_for_room(
            &response,
            "room-1",
            &BTreeSet::from(["scene-static".to_string()]),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].id, "native-hue-scene-cool");
    }

    #[test]
    fn extract_device_from_hue_accessories_behavior() {
        // Real structure from a live Hue bridge (RWL022 dimmer switch)
        let bi: serde_json::Value = serde_json::json!({
            "id": "7e929366-df9e-4044-8f4c-0ccec3a43d5a",
            "type": "behavior_instance",
            "script_id": "67d9395b-4403-42cc-b5f0-740b699d67c6",
            "enabled": true,
            "configuration": {
                "device": {
                    "rid": "4c7a67d9-2c1f-4c8e-b456-018c48f5521b",
                    "rtype": "device"
                },
                "model_id": "RWL022"
            },
            "dependees": [
                { "target": { "rid": "4c7a67d9-2c1f-4c8e-b456-018c48f5521b", "rtype": "device" }, "level": "critical", "type": "ResourceDependee" },
                { "target": { "rid": "b902b016-69af-418a-9b47-597ae712626b", "rtype": "room" }, "level": "critical", "type": "ResourceDependee" }
            ]
        });

        let device_id =
            HueDiscovery::<crate::test_support::SpyHueTransport>::extract_device_from_behavior(&bi);
        assert_eq!(
            device_id.as_deref(),
            Some("4c7a67d9-2c1f-4c8e-b456-018c48f5521b")
        );
    }

    #[test]
    fn extract_device_falls_back_to_dependees() {
        // Behavior instance without configuration.device but with dependees
        let bi: serde_json::Value = serde_json::json!({
            "id": "test-behavior",
            "configuration": {},
            "dependees": [
                { "target": { "rid": "room-1", "rtype": "room" }, "level": "critical", "type": "ResourceDependee" },
                { "target": { "rid": "device-abc", "rtype": "device" }, "level": "critical", "type": "ResourceDependee" }
            ]
        });

        let device_id =
            HueDiscovery::<crate::test_support::SpyHueTransport>::extract_device_from_behavior(&bi);
        assert_eq!(device_id.as_deref(), Some("device-abc"));
    }

    #[test]
    fn extract_device_returns_none_when_no_device_ref() {
        let bi: serde_json::Value = serde_json::json!({
            "id": "test-behavior",
            "configuration": {},
            "dependees": [
                { "target": { "rid": "room-1", "rtype": "room" }, "level": "critical", "type": "ResourceDependee" }
            ]
        });

        let device_id =
            HueDiscovery::<crate::test_support::SpyHueTransport>::extract_device_from_behavior(&bi);
        assert!(device_id.is_none());
    }

    #[test]
    fn discover_motion_state_reads_hue_motion_resources() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "room",
                serde_json::json!({
                    "data": [
                        {
                            "id": "room-1",
                            "children": [
                                { "rtype": "device", "rid": "motion-device-1" }
                            ],
                            "services": [
                                { "rtype": "grouped_light", "rid": "grouped-light-1" }
                            ],
                            "metadata": { "name": "Guest room" }
                        }
                    ]
                }),
            )
            .with_resource(
                "motion",
                serde_json::json!({
                    "data": [
                        {
                            "id": "motion-svc-1",
                            "owner": {
                                "rtype": "device",
                                "rid": "motion-device-1"
                            },
                            "motion": { "motion": true }
                        },
                        {
                            "id": "roomless-motion",
                            "owner": {
                                "rtype": "device",
                                "rid": "roomless-device"
                            },
                            "motion": { "motion": false }
                        }
                    ]
                }),
            );
        let discovery = HueDiscovery::new(Arc::new(transport), "test-user".to_string());

        let states = discovery.discover_motion_state().unwrap();

        assert_eq!(states.len(), 2);
        assert_eq!(states[0].sensor_id, "motion-svc-1");
        assert_eq!(states[0].room_id.as_deref(), Some("room-1"));
        assert!(states[0].is_active);
        assert_eq!(states[1].sensor_id, "roomless-motion");
        assert_eq!(states[1].room_id, None);
        assert!(!states[1].is_active);
    }

    #[test]
    fn authoritative_discovery_exposes_manifest_rooms_and_native_scenes_but_hides_automations() {
        let transport = StaticHueTransport::default()
            .with_resource(
                "room",
                serde_json::json!({"data": [
                    {
                        "id": "managed-room",
                        "metadata": {"name": "Managed"},
                        "children": [{"rtype": "device", "rid": "managed-button"}],
                        "services": [{"rtype": "grouped_light", "rid": "managed-group"}]
                    },
                    {
                        "id": "native-room",
                        "metadata": {"name": "Native"},
                        "children": [{"rtype": "device", "rid": "native-button"}],
                        "services": [{"rtype": "grouped_light", "rid": "native-group"}]
                    }
                ]}),
            )
            .with_resource(
                "device",
                serde_json::json!({"data": [
                    {
                        "id": "managed-button",
                        "services": [{"rtype": "button", "rid": "button-1"}]
                    },
                    {
                        "id": "native-button",
                        "services": [{"rtype": "button", "rid": "button-2"}]
                    }
                ]}),
            )
            .with_resource(
                "scene",
                serde_json::json!({"data": [{
                    "id": "native-scene",
                    "group": {"rtype": "room", "rid": "managed-room"},
                    "metadata": {"name": "Native scene"},
                    "actions": [{
                        "action": {
                            "on": {"on": true},
                            "color_temperature": {"mirek": 300}
                        }
                    }]
                }]}),
            )
            .with_resource(
                "behavior_instance",
                serde_json::json!({"data": [{
                    "id": "native-behavior",
                    "configuration": {
                        "device": {"rtype": "device", "rid": "managed-button"}
                    }
                }]}),
            );
        let discovery = HueDiscovery::new_authoritative(
            Arc::new(transport),
            "test-user".to_string(),
            BTreeSet::from(["managed-room".to_string()]),
        );

        let rooms = discovery.discover_rooms().unwrap();
        assert_eq!(rooms.len(), 1);
        assert_eq!(rooms[0].id, "managed-room");
        let devices = discovery.discover_devices().unwrap();
        assert_eq!(devices.len(), 2);
        assert!(devices
            .iter()
            .any(|device| { device.device_id == "native-button" && device.room_id.is_none() }));
        assert_eq!(
            discovery.discover_scenes("managed-room").unwrap()[0].id,
            "native-hue-native-scene"
        );
        assert!(discovery.discover_configured_devices().unwrap().is_empty());
        discovery.recall_scene("native-scene", None).unwrap();
    }

    #[test]
    fn extract_devices_emits_roomless_motion_sensor() {
        // A motion sensor device that is not a child of any Hue room.
        let device_data = vec![serde_json::json!({
            "id": "roomless-motion-device",
            "services": [
                { "rtype": "motion", "rid": "motion-svc-1" }
            ]
        })];
        let device_to_room = HashMap::new(); // empty: no room ownership

        let devices = HueDiscovery::<crate::test_support::SpyHueTransport>::extract_devices(
            &device_data,
            &device_to_room,
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_id, "motion-svc-1");
        assert_eq!(devices[0].device_type, DeviceType::Motion);
        assert!(
            devices[0].room_id.is_none(),
            "roomless device should have room_id None, got {:?}",
            devices[0].room_id
        );
    }

    #[test]
    fn extract_devices_emits_roomless_button_device() {
        let device_data = vec![serde_json::json!({
            "id": "roomless-button-device",
            "services": [
                { "rtype": "button", "rid": "btn-1" },
                { "rtype": "button", "rid": "btn-2" }
            ]
        })];
        let device_to_room = HashMap::new();

        let devices = HueDiscovery::<crate::test_support::SpyHueTransport>::extract_devices(
            &device_data,
            &device_to_room,
        );

        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].device_type, DeviceType::Button);
        assert!(devices[0].room_id.is_none());
        assert_eq!(devices[0].buttons.len(), 2);
    }

    #[test]
    fn extract_identities_emits_roomless_motion_with_none_room() {
        let device_data = vec![serde_json::json!({
            "id": "roomless-motion-device",
            "metadata": { "name": "Hallway Motion" },
            "product_data": {
                "manufacturer_name": "Signify Netherlands B.V.",
                "model_id": "SML004"
            },
            "services": [
                { "rtype": "motion", "rid": "motion-svc-1" }
            ]
        })];
        let device_to_room = HashMap::new();
        let room_names = HashMap::new();
        let mut zigbee_mac_map = HashMap::new();
        zigbee_mac_map.insert(
            "roomless-motion-device".to_string(),
            "00:17:88:01:0b:c0:ff:ee".to_string(),
        );

        let identities = HueDiscovery::<crate::test_support::SpyHueTransport>::extract_identities(
            &device_data,
            &device_to_room,
            &room_names,
            &zigbee_mac_map,
        );

        assert_eq!(identities.len(), 1);
        let id = &identities[0];
        assert_eq!(id.native_id, "motion-svc-1");
        assert_eq!(id.device_type, DeviceType::Motion);
        assert!(id.room_id.is_none());
        assert!(id.room_name.is_none());
        assert_eq!(id.name, "Hallway Motion");
        assert_eq!(id.hardware_ids.len(), 1);
    }
}
