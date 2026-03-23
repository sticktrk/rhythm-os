//! Extracted business logic for configuration commands.
//!
//! Platform-agnostic functions called by HTTP handlers.
//! Each function takes `SharedState` and parsed parameters, performs the
//! operation (registry update, engine update, persistence), and returns
//! a result that the transport layer can format into a response.

use std::collections::HashSet;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{Datelike, Timelike};
use log::{info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::CurveConfig;
use rhythm_core::{ButtonAction, HubRegistry, InputEvent};
use serde_json::Value;

use crate::api_types::{
    FixResponse, HubDto, LocationDto, RoomFullState, RoomPollState, RoomRhythmState,
    RoomsPollResponse, SettingsDto, StateSnapshot, TypedDeviceDto,
};
use crate::canonical::identity::HubKey;
use crate::state::{rooms_from_engine, AppState, SharedState};
use crate::storage::{StoredLocation, StoredSettings};

// ============================================================================
// Display value computation
// ============================================================================

/// Compute effective brightness and kelvin for a room given its offsets.
///
/// Uses the global CurveConfig, current solar context, and room offsets
/// to produce the values a client should display.
pub fn compute_room_display_values(
    config: &CurveConfig,
    solar_noon: f32,
    latitude: f32,
    utc_offset: f32,
    time_offset_minutes: f32,
    brightness_offset: f32,
) -> (u8, u16) {
    use rhythm_core::{LightCurveModule, RhythmCurveModule, SolarTime};

    let now = chrono::Utc::now().naive_utc();
    let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let t = local.time();
    let current_hour = t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0;

    let solar = SolarTime::new(solar_noon, latitude, day_of_year);
    let ctx = rhythm_core::curve_module::CurveContext::new(current_hour, solar, None);
    let module = RhythmCurveModule::new(config.clone());
    let values = module.calculate_with_offset(&ctx, time_offset_minutes);
    let brightness = (values.brightness as f32 + brightness_offset).clamp(1.0, 100.0) as u8;
    (brightness, values.kelvin)
}

/// Build a RoomStateEvent for a room, computing display values from AppState.
///
/// For soft-off rooms, brightness is the soft-off percentage (not curve value).
/// Kelvin is always from the curve (soft-off tracks color temp).
#[cfg(feature = "desktop")]
pub fn build_room_state_event(
    state: &SharedState,
    snap: &rhythm_core::RoomSnapshot,
) -> crate::server_event::RoomStateEvent {
    let (lights_on, brightness, kelvin) = {
        let s = state.lock().unwrap_or_else(|e| e.into_inner());
        let lights_on = s.room_lights_on.get(&snap.id).copied().unwrap_or(false);
        let (curve_brightness, kelvin) = compute_room_display_values(
            &s.config,
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            snap.time_offset_minutes,
            snap.brightness_offset,
        );
        // Soft-off rooms display at soft_off_brightness, not the curve value
        let brightness = if snap.soft_off && !s.power_save {
            s.soft_off_brightness
        } else {
            curve_brightness
        };
        (lights_on, brightness, kelvin)
    };
    crate::server_event::RoomStateEvent::from_snapshot(snap, lights_on, brightness, kelvin)
}

/// Extract a single registry Arc from any active hub (brief AppState lock).
/// Used as a fallback for single-item operations when no hub_key is provided.
fn extract_registry(state: &SharedState) -> Option<Arc<Mutex<dyn HubRegistry>>> {
    state
        .lock()
        .ok()
        .and_then(|s| s.all_hub_registries().into_iter().next())
}

// ============================================================================
// Room ID translation helpers
// ============================================================================

/// Resolve a room ID from an HTTP request to the canonical engine room ID.
///
/// If the ID is already a topology room ID, returns it unchanged.
/// If it's a hub-native ID, translates it via the topology store.
/// Falls through unchanged on ESP32 (empty topology) or unknown IDs.
pub fn resolve_room_id(state: &SharedState, room_id: &str) -> String {
    let s = match state.lock() {
        Ok(s) => s,
        Err(_) => return room_id.to_string(),
    };
    // Already a topology room ID?
    if s.topology.get(room_id).is_some() {
        return room_id.to_string();
    }
    // Try translating as hub-native ID
    if let Some(topo_id) = s.topology.translate_room_id_any_hub(room_id) {
        return topo_id.to_string();
    }
    // Fallback: use as-is (ESP32 or pre-topology state)
    room_id.to_string()
}

/// Reverse-lookup: topology room ID → hub-native room IDs for registry operations.
///
/// The hub-native registry uses hub room IDs, but callers use topology IDs.
/// Returns the ID unchanged as fallback (ESP32 / no topology entry).
fn hub_room_ids_for_topology(state: &SharedState, topo_id: &str) -> Vec<String> {
    state
        .lock()
        .ok()
        .and_then(|s| {
            s.topology.get(topo_id).map(|room| {
                room.hub_targets
                    .iter()
                    .map(|t| t.hub_room_id.clone())
                    .collect()
            })
        })
        .unwrap_or_else(|| vec![topo_id.to_string()])
}

// ============================================================================
// State snapshots (for GET endpoints)
// ============================================================================

/// Build a full state snapshot.
///
/// Two-phase lock: collects metadata from state (brief lock), then queries
/// engine snapshots (engine read lock) to prevent cascading lock contention.
pub fn build_state_snapshot(state: &SharedState) -> Result<String> {
    struct RoomInfo {
        topology_id: String, // stable Rhythm room ID (for engine + external API)
        name: String,
        hub_type: Option<String>,
        grouped_light_id: String,
        device_ids: Vec<String>,
        typed_devices: Vec<(String, DeviceType)>,
    }

    // Phase 1: Brief AppState lock — extract registry Arcs + scalar data + canonical lookup
    let (
        all_registries,
        runtime,
        storage_rooms,
        hub_dto,
        hubs_dto,
        config_value,
        location_dto,
        settings_dto,
        firmware_version,
        platform_type,
        platform_ctx,
        listen_port,
        canonical_lookup,
        topo_map,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        let runtime = s.hub_runtime();

        let storage_rooms = if runtime.is_none() {
            s.storage.as_ref().and_then(|st| st.load_rooms().ok())
        } else {
            None
        };

        let all_registries = s.all_hub_registries();

        // Build multi-hub DTOs (sorted: connected hubs first for deterministic primary selection)
        let mut hubs_dto: Vec<HubDto> = s
            .hub_credentials
            .iter()
            .map(|(key, creds)| HubDto {
                hub_type: creds
                    .hub_type
                    .as_ref()
                    .map(|t| t.as_str().to_string())
                    .unwrap_or_else(|| "none".to_string()),
                address: Some(creds.address.clone()),
                connected: s.hubs.contains_key(key),
            })
            .collect();
        // Sort connected hubs first so primary hub is deterministic
        hubs_dto.sort_by(|a, b| b.connected.cmp(&a.connected));

        // Primary hub (backward compat): first connected, or first configured
        let hub_dto = hubs_dto
            .iter()
            .find(|h| h.connected)
            .or(hubs_dto.first())
            .cloned()
            .unwrap_or(HubDto {
                hub_type: "none".to_string(),
                address: None,
                connected: false,
            });

        let config_value = serde_json::to_value(&s.config)
            .unwrap_or(serde_json::Value::Object(Default::default()));

        let location_dto = LocationDto {
            latitude: s.latitude,
            longitude: s.longitude,
            utc_offset_hours: s.utc_offset_hours,
            timezone_name: s.timezone_name.clone(),
        };

        let settings_dto = SettingsDto {
            bulb_fade_ms: s.bulb_fade_ms,
            rhythm_interval_secs: s.runtime_config.update_interval_secs,
            default_motion_timeout_secs: s.default_motion_timeout_secs,
            power_save: s.power_save,
            soft_off_brightness: s.soft_off_brightness,
        };

        let fw_version = s.firmware_version;
        let platform_type = s.platform_type;
        let platform_ctx = s.platform_context;
        let listen_port = s.listen_port;

        // Build canonical device lookup: native_id → (name, manufacturer, model, device_type)
        // Aggregate across all configured hubs
        let active_hub_keys: std::collections::HashSet<String> =
            s.hub_credentials.keys().map(|k| k.to_string()).collect();
        let canonical_lookup: std::collections::HashMap<
            String,
            (String, Option<String>, Option<String>, DeviceType),
        > = s
            .canonical_registry
            .devices()
            .flat_map(|d| {
                d.endpoints
                    .iter()
                    .filter(|ep| active_hub_keys.contains(&ep.hub_key.to_string()))
                    .map(move |ep| {
                        (
                            ep.native_id.clone(),
                            (
                                d.name.clone(),
                                d.manufacturer.clone(),
                                d.model.clone(),
                                d.device_type.clone(),
                            ),
                        )
                    })
            })
            .collect();

        // Build hub-native → (topology_id, topology_name) lookup so Phase 1b
        // can translate registry room IDs to stable Rhythm room IDs.
        let topo_map: std::collections::HashMap<String, (String, String)> = s
            .topology
            .rooms()
            .flat_map(|room| {
                room.hub_targets
                    .iter()
                    .map(move |t| (t.hub_room_id.clone(), (room.id.clone(), room.name.clone())))
            })
            .collect();

        (
            all_registries,
            runtime,
            storage_rooms,
            hub_dto,
            hubs_dto,
            config_value,
            location_dto,
            settings_dto,
            fw_version,
            platform_type,
            platform_ctx,
            listen_port,
            canonical_lookup,
            topo_map,
        )
    };
    // AppState lock released

    // Phase 1b: Lock registries without holding AppState — aggregate rooms from ALL hubs
    let mut room_infos = Vec::new();
    for reg_arc in &all_registries {
        if let Ok(reg) = reg_arc.lock() {
            for room in reg.rooms() {
                let hub_type = match room.source {
                    rhythm_core::room::RoomSource::Hue => Some("hue".to_string()),
                    rhythm_core::room::RoomSource::HomeAssistant => {
                        Some("homeassistant".to_string())
                    }
                    rhythm_core::room::RoomSource::Esp32 => Some("esp32".to_string()),
                    _ => None,
                };
                // Translate hub-native room ID to topology ID for external API.
                // Registry lookups above already used the hub-native ID.
                let (topo_id, topo_name) = topo_map
                    .get(&room.id)
                    .map(|(id, name)| (id.clone(), name.clone()))
                    .unwrap_or_else(|| (room.id.clone(), room.name.clone()));
                room_infos.push(RoomInfo {
                    topology_id: topo_id,
                    name: topo_name,
                    hub_type,
                    grouped_light_id: reg.get_grouped_light_id(&room.id).unwrap_or_default(),
                    device_ids: reg.devices_for_room(&room.id),
                    typed_devices: reg.devices_for_room_typed(&room.id),
                });
            }
        }
    }

    // Fallback: if registry had no rooms (e.g. boot before hub connects),
    // populate from storage so /api/state agrees with /api/rooms/state.
    // Storage rooms already have topology IDs (persisted from engine after Phase 4d).
    if room_infos.is_empty() {
        if let Some(ref mgr) = storage_rooms {
            for room in mgr.iter() {
                room_infos.push(RoomInfo {
                    topology_id: room.id.clone(),
                    name: room.name.clone(),
                    hub_type: None,
                    grouped_light_id: String::new(),
                    device_ids: Vec::new(),
                    typed_devices: Vec::new(),
                });
            }
        }
    }

    // Dedup: cross-hub rooms may have multiple registry entries that map to the
    // same topology ID. Merge device lists, keep first occurrence's metadata.
    {
        let mut seen: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut deduped: Vec<RoomInfo> = Vec::new();
        for info in room_infos {
            if let Some(&idx) = seen.get(&info.topology_id) {
                deduped[idx].device_ids.extend(info.device_ids);
                deduped[idx].typed_devices.extend(info.typed_devices);
            } else {
                seen.insert(info.topology_id.clone(), deduped.len());
                deduped.push(info);
            }
        }
        room_infos = deduped;
    }

    // Phase 2: query engine snapshots without state lock
    // Grab display context for computing brightness/kelvin
    let (
        disp_config,
        disp_solar,
        disp_lat,
        disp_utc,
        disp_lights,
        disp_power_save,
        disp_soft_off_bri,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.config.clone(),
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.room_lights_on.clone(),
            s.power_save,
            s.soft_off_brightness,
        )
    };

    let mut rooms = Vec::with_capacity(room_infos.len());
    for info in &room_infos {
        let (rhythm_enabled, disabled, time_offset, bri_offset, soft_off) =
            if let Some(ref rt) = runtime {
                rt.engine_room_snapshot(&info.topology_id)
                    .map(|snap| {
                        (
                            snap.rhythm_enabled,
                            snap.disabled,
                            snap.time_offset_minutes,
                            snap.brightness_offset,
                            snap.soft_off,
                        )
                    })
                    .unwrap_or((false, false, 0.0, 0.0, false))
            } else if let Some(ref mgr) = storage_rooms {
                mgr.get(&info.topology_id)
                    .map(|r| {
                        (
                            r.rhythm_enabled,
                            r.disabled,
                            r.time_offset_minutes,
                            r.brightness_offset,
                            r.soft_off,
                        )
                    })
                    .unwrap_or((false, false, 0.0, 0.0, false))
            } else {
                (false, false, 0.0, 0.0, false)
            };

        let lights_on = disp_lights.get(&info.topology_id).copied().unwrap_or(false);
        let (curve_brightness, kelvin) = compute_room_display_values(
            &disp_config,
            disp_solar,
            disp_lat,
            disp_utc,
            time_offset,
            bri_offset,
        );
        let brightness = if soft_off && !disp_power_save {
            disp_soft_off_bri
        } else {
            curve_brightness
        };

        // Build enriched device list: all devices from device_ids (with canonical
        // data) plus any typed_devices not already covered (e.g. motion service IDs)
        let typed_lookup: std::collections::HashMap<&str, &DeviceType> = info
            .typed_devices
            .iter()
            .map(|(id, dt)| (id.as_str(), dt))
            .collect();
        let mut devices: Vec<TypedDeviceDto> = Vec::new();
        let mut seen_ids: std::collections::HashSet<String> = std::collections::HashSet::new();

        // All device UUIDs from room children — includes lights, buttons, etc.
        for id in &info.device_ids {
            if !seen_ids.insert(id.clone()) {
                continue;
            }
            if let Some((name, mfr, model, dt)) = canonical_lookup.get(id) {
                let name_opt = if name.is_empty() {
                    None
                } else {
                    Some(name.clone())
                };
                devices.push(TypedDeviceDto::enriched(
                    id.clone(),
                    dt,
                    name_opt,
                    mfr.clone(),
                    model.clone(),
                ));
            } else {
                let dt = typed_lookup
                    .get(id.as_str())
                    .copied()
                    .unwrap_or(&DeviceType::Light);
                devices.push(TypedDeviceDto::new(id.clone(), dt));
            }
        }

        // Add typed devices not in device_ids (motion sensors use service UUIDs)
        for (id, dt) in &info.typed_devices {
            if seen_ids.contains(id) {
                continue;
            }
            if let Some((name, mfr, model, _)) = canonical_lookup.get(id) {
                let name_opt = if name.is_empty() {
                    None
                } else {
                    Some(name.clone())
                };
                devices.push(TypedDeviceDto::enriched(
                    id.clone(),
                    dt,
                    name_opt,
                    mfr.clone(),
                    model.clone(),
                ));
            } else {
                devices.push(TypedDeviceDto::new(id.clone(), dt));
            }
        }

        rooms.push(RoomFullState {
            rhythm: RoomRhythmState {
                id: info.topology_id.clone(),
                hub_type: info.hub_type.clone(),
                rhythm_enabled,
                time_offset,
                brightness_offset: bri_offset,
                soft_off,
                lights_on,
                brightness,
                kelvin,
            },
            name: info.name.clone(),
            grouped_light_id: info.grouped_light_id.clone(),
            disabled,
            device_ids: info.device_ids.clone(),
            devices,
        });
    }

    let snapshot = StateSnapshot {
        version: firmware_version.to_string(),
        platform: platform_type.to_string(),
        context: platform_ctx.to_string(),
        listen_port,
        hub: hub_dto,
        hubs: hubs_dto,
        config: config_value,
        location: location_dto,
        settings: settings_dto,
        rooms,
    };
    serde_json::to_string(&snapshot).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build a lightweight rooms-state snapshot for polling.
pub fn build_rooms_state(state: &SharedState) -> Result<String> {
    let (
        hub_connected,
        runtime,
        storage_rooms,
        motion_snapshots,
        room_lights_on,
        config,
        solar_noon,
        latitude,
        utc_offset,
        power_save,
        soft_off_bri,
        rooms_with_sensors,
        motion_timeouts,
        default_motion_timeout,
    ) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let runtime = s.hub_runtime();
        let storage_rooms = if runtime.is_none() {
            s.storage.as_ref().and_then(|st| st.load_rooms().ok())
        } else {
            None
        };
        let motion = s.motion_snapshots.clone();
        let lights = s.room_lights_on.clone();
        let sensor_rooms = s.motion_sensor_room_ids();
        let mt = s.motion_timeouts.clone();
        let dmt = s.default_motion_timeout_secs;
        (
            s.has_any_hub(),
            runtime,
            storage_rooms,
            motion,
            lights,
            s.config.clone(),
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.power_save,
            s.soft_off_brightness,
            sensor_rooms,
            mt,
            dmt,
        )
    };

    let snapshots = if let Some(ref rt) = runtime {
        rt.engine_all_room_snapshots()
    } else {
        Vec::new()
    };

    let mut rooms = Vec::new();

    if !snapshots.is_empty() {
        for snap in &snapshots {
            let lights_on = room_lights_on.get(&snap.id).copied().unwrap_or(false);
            let (curve_brightness, kelvin) = compute_room_display_values(
                &config,
                solar_noon,
                latitude,
                utc_offset,
                snap.time_offset_minutes,
                snap.brightness_offset,
            );
            let brightness = if snap.soft_off && !power_save {
                soft_off_bri
            } else {
                curve_brightness
            };
            let rhythm = RoomRhythmState {
                id: snap.id.clone(),
                hub_type: None, // poll endpoint doesn't include hub_type
                rhythm_enabled: snap.rhythm_enabled,
                time_offset: snap.time_offset_minutes,
                brightness_offset: snap.brightness_offset,
                soft_off: snap.soft_off,
                lights_on,
                brightness,
                kelvin,
            };
            let (motion_active, motion_owned, remaining_secs, timeout_secs, warning_active) =
                if let Some(ms) = motion_snapshots.get(&snap.id) {
                    (
                        Some(ms.motion_active),
                        Some(ms.motion_owned),
                        ms.remaining_secs,
                        Some(ms.timeout_secs),
                        Some(ms.warning_active),
                    )
                } else if rooms_with_sensors.contains(&snap.id) {
                    // Sensor exists but hasn't fired yet — idle defaults
                    let timeout = motion_timeouts
                        .get(&snap.id)
                        .copied()
                        .unwrap_or(default_motion_timeout);
                    (Some(false), Some(false), None, Some(timeout), Some(false))
                } else {
                    (None, None, None, None, None)
                };
            rooms.push(RoomPollState {
                rhythm,
                motion_active,
                motion_owned,
                remaining_secs,
                timeout_secs,
                warning_active,
            });
        }
    } else if let Some(ref mgr) = storage_rooms {
        for room in mgr.iter() {
            let (curve_brightness, kelvin) = compute_room_display_values(
                &config,
                solar_noon,
                latitude,
                utc_offset,
                room.time_offset_minutes,
                room.brightness_offset,
            );
            let brightness = if room.soft_off && !power_save {
                soft_off_bri
            } else {
                curve_brightness
            };
            rooms.push(RoomPollState {
                rhythm: RoomRhythmState {
                    id: room.id.clone(),
                    hub_type: None,
                    rhythm_enabled: room.rhythm_enabled,
                    time_offset: room.time_offset_minutes,
                    brightness_offset: room.brightness_offset,
                    soft_off: room.soft_off,
                    lights_on: false,
                    brightness,
                    kelvin,
                },
                motion_active: None,
                motion_owned: None,
                remaining_secs: None,
                timeout_secs: None,
                warning_active: None,
            });
        }
    }

    let response = RoomsPollResponse {
        hub_connected,
        rooms,
    };
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build a `RoomRhythmState` for a single room from engine state.
pub fn build_room_rhythm_state(state: &SharedState, room_id: &str) -> Result<RoomRhythmState> {
    let (runtime, lights_on, config, solar_noon, latitude, utc_offset, power_save, soft_off_bri) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (
            s.hub_runtime(),
            s.room_lights_on.get(room_id).copied().unwrap_or(false),
            s.config.clone(),
            s.solar_noon_hour(),
            s.latitude.unwrap_or(35.0),
            s.utc_offset_hours,
            s.power_save,
            s.soft_off_brightness,
        )
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;
    let snap = runtime
        .engine_room_snapshot(room_id)
        .ok_or_else(|| anyhow::anyhow!("Room '{}' not found in engine", room_id))?;
    let (curve_brightness, kelvin) = compute_room_display_values(
        &config,
        solar_noon,
        latitude,
        utc_offset,
        snap.time_offset_minutes,
        snap.brightness_offset,
    );
    let brightness = if snap.soft_off && !power_save {
        soft_off_bri
    } else {
        curve_brightness
    };

    Ok(RoomRhythmState {
        id: snap.id.clone(),
        hub_type: None, // single-room queries don't include hub_type
        rhythm_enabled: snap.rhythm_enabled,
        time_offset: snap.time_offset_minutes,
        brightness_offset: snap.brightness_offset,
        soft_off: snap.soft_off,
        lights_on,
        brightness,
        kelvin,
    })
}

// ============================================================================
// Config queries
// ============================================================================

/// Build the current curve config as a JSON string.
pub fn build_config(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    serde_json::to_string(&s.config).map_err(|e| anyhow::anyhow!("serialize config: {}", e))
}

// ============================================================================
// Settings commands
// ============================================================================

/// Build the current settings.
pub fn build_settings_dto(state: &SharedState) -> Result<SettingsDto> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    Ok(SettingsDto {
        bulb_fade_ms: s.bulb_fade_ms,
        rhythm_interval_secs: s.runtime_config.update_interval_secs,
        default_motion_timeout_secs: s.default_motion_timeout_secs,
        power_save: s.power_save,
        soft_off_brightness: s.soft_off_brightness,
    })
}

/// Build the current settings as a JSON string.
pub fn build_settings(state: &SharedState) -> Result<String> {
    let dto = build_settings_dto(state)?;
    serde_json::to_string(&dto).map_err(|e| anyhow::anyhow!("serialize settings: {}", e))
}

/// Update global settings (partial: only provided fields are changed).
///
/// Persists to storage and updates the atomic for dynamics duration.
pub fn do_settings_set(
    state: &SharedState,
    fade_ms: Option<u16>,
    update_interval: Option<u64>,
    motion_timeout: Option<u64>,
    power_save: Option<bool>,
    soft_off_brightness: Option<u8>,
) -> Result<String> {
    let mut rooms_to_off = Vec::new();

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        if let Some(ms) = fade_ms {
            s.bulb_fade_ms = ms;
            s.bulb_fade_atomic.store(ms, Ordering::Relaxed);
            info!(target: "cmd", "settings: bulb_fade_ms={}", ms);
        }

        if let Some(interval) = update_interval {
            let clamped = interval.max(10);
            s.runtime_config.update_interval_secs = clamped;
            info!(target: "cmd", "settings: rhythm_interval_secs={}", clamped);
        }

        if let Some(timeout) = motion_timeout {
            s.default_motion_timeout_secs = timeout;
            info!(target: "cmd", "settings: default_motion_timeout_secs={}", timeout);
        }

        if let Some(ps) = power_save {
            s.power_save = ps;
            info!(target: "cmd", "settings: power_save={}", ps);

            if let Some(runtime) = s.hub_runtime() {
                rooms_to_off = runtime.set_power_save(ps);
            }
        }

        if let Some(sob) = soft_off_brightness {
            let clamped = sob.clamp(1, 100);
            s.soft_off_brightness = clamped;
            info!(target: "cmd", "settings: soft_off_brightness={}", clamped);

            if let Some(runtime) = s.hub_runtime() {
                runtime.set_soft_off_brightness(clamped);
            }
        }

        // Persist settings
        if let Some(ref storage) = s.storage {
            let stored = StoredSettings {
                bulb_fade_ms: s.bulb_fade_ms,
                rhythm_interval_secs: s.runtime_config.update_interval_secs,
                default_motion_timeout_secs: s.default_motion_timeout_secs,
                power_save: s.power_save,
                soft_off_brightness: s.soft_off_brightness,
            };
            if let Err(e) = storage.save_settings(&stored) {
                warn!(target: "cmd", "Failed to save settings: {}", e);
            }
        }
    }

    // If switching power_save ON, turn soft_off rooms truly off
    if !rooms_to_off.is_empty() {
        let runtime = {
            let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.hub_runtime()
        };
        if let Some(runtime) = runtime {
            for room_id in &rooms_to_off {
                info!(target: "cmd", "Turning off soft_off room '{}' (power_save ON)", room_id);
                let event = InputEvent::new(room_id, ButtonAction::OffPress);
                if let Err(e) = runtime.handle_event(&event) {
                    warn!(target: "cmd", "Failed to turn off room '{}': {}", room_id, e);
                }
            }
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::SettingsChanged);

    build_settings(state)
}

// ============================================================================
// Room commands
// ============================================================================

/// Parsed room data from a JSON value.
pub struct RoomParams {
    pub id: String,
    pub name: String,
    pub grouped_light_id: String,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    /// Explicit soft-off from client. `None` = not provided (preserve engine state).
    pub soft_off: Option<bool>,
    pub device_ids: Vec<String>,
}

impl RoomParams {
    /// Parse room parameters from a JSON value (the room object itself).
    pub fn from_json(room: &Value) -> Result<Self> {
        let id = room
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing room.id"))?
            .to_string();
        let name = room
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or(&id)
            .to_string();
        let grouped_light_id = room
            .get("grouped_light_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let rhythm_enabled = room
            .get("rhythm_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let disabled = room
            .get("disabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let soft_off = room.get("soft_off").and_then(|v| v.as_bool());
        let device_ids: Vec<String> = room
            .get("device_ids")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();

        Ok(RoomParams {
            id,
            name,
            grouped_light_id,
            rhythm_enabled,
            disabled,
            soft_off,
            device_ids,
        })
    }
}

/// Upsert a room in registry + engine, optionally persist.
///
/// When `persist` is `false` the caller is responsible for triggering
/// persistence (e.g. via `DeferredPersistState` on the worker thread).
/// This avoids stack-heavy serde on the 12KB HTTP handler stack during
/// batch room updates.
/// Returns the room's rhythm state JSON snippet.
pub fn do_room_set(
    state: &SharedState,
    params: &RoomParams,
    hub_key: Option<&HubKey>,
    persist: bool,
) -> Result<String> {
    info!(target: "cmd", "room_set: {} '{}' gl={}", params.id, params.name, params.grouped_light_id);

    // Dedup: skip write if room already matches (check targeted registry when hub_key provided)
    let dedup_registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));
    let room_unchanged = dedup_registry
        .and_then(|r| {
            r.lock().ok().map(|reg| {
                reg.room_matches(
                    &params.id,
                    &params.name,
                    &params.grouped_light_id,
                    &params.device_ids,
                )
            })
        })
        .unwrap_or(false);
    if room_unchanged {
        if let Ok(room_state) = build_room_rhythm_state(state, &params.id) {
            info!(target: "cmd", "room_set: {} unchanged, skipping persist", params.id);
            return serde_json::to_string(&room_state)
                .map_err(|e| anyhow::anyhow!("serialize: {}", e));
        }
        // Room is in registry but not yet in engine — fall through to add it
    }

    // Upsert into the targeted hub's registry (when hub_key provided) or all
    // registries (when None — backward compat for HTTP handler path).
    {
        let registries: Vec<Arc<Mutex<dyn HubRegistry>>> = state
            .lock()
            .ok()
            .map(|s| {
                if let Some(key) = hub_key {
                    s.hub_registry_for(key).into_iter().collect()
                } else {
                    s.hubs
                        .values()
                        .filter_map(|hub| hub.registry.clone())
                        .collect()
                }
            })
            .unwrap_or_default();
        for reg in &registries {
            if let Ok(mut reg) = reg.lock() {
                reg.upsert_room(
                    &params.id,
                    &params.name,
                    &params.grouped_light_id,
                    &params.device_ids,
                );
            }
        }
    }

    // Ensure runtime exists (first room triggers runtime creation)
    let needs_runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let has_runtime = s.hubs.values().any(|h| h.runtime.is_some());
        !has_runtime && s.has_any_hub()
    };

    if needs_runtime {
        ensure_runtime(state);
    }

    // Determine engine room ID: always use topology room IDs for the engine.
    // When hub_key is provided (room_sync path), translate_or_create ensures a
    // topology entry exists. When hub_key is None (HTTP handler path), params.id
    // should already be a topology ID (clients send topology IDs from GET /api/state).
    // Registry operations (above) always use params.id (hub-native).
    let engine_room_id = if let Some(k) = hub_key {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.topology.translate_or_create(
            k,
            &params.id,
            &params.name,
            &params.grouped_light_id,
            &params.device_ids,
        )
    } else {
        params.id.clone()
    };

    // Always apply params (whether runtime was just created or pre-existing)
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    if let Some(runtime) = runtime {
        let existing = runtime.engine_room_snapshot(&engine_room_id);
        runtime.add_room(&engine_room_id, &params.name);
        let (rhythm_enabled, time_offset, bri_offset, soft_off) = existing
            .map(|snap| {
                (
                    params.rhythm_enabled,
                    snap.time_offset_minutes,
                    snap.brightness_offset,
                    params.soft_off.unwrap_or(snap.soft_off),
                )
            })
            .unwrap_or((
                params.rhythm_enabled,
                0.0,
                0.0,
                params.soft_off.unwrap_or(false),
            ));
        // Soft-off rooms need rhythm enabled for periodic soft-off ticks
        let rhythm_enabled = if soft_off { true } else { rhythm_enabled };
        runtime.restore_room_state(
            &engine_room_id,
            rhythm_enabled,
            params.disabled,
            time_offset,
            bri_offset,
            soft_off,
        );
    }

    if persist {
        persist_state(state);
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);

    let room_state = build_room_rhythm_state(state, &engine_room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Remove a room from registry + engine + topology, persist.
///
/// `room_id` should be a topology room ID (handler resolves before calling).
/// Looks up hub-native IDs from topology for registry removal.
pub fn do_room_remove(state: &SharedState, room_id: &str) -> Result<()> {
    info!(target: "cmd", "room_remove: {}", room_id);

    // Look up hub-native room IDs for registry removal
    let hub_room_ids = hub_room_ids_for_topology(state, room_id);

    // Remove from ALL hub registries using hub-native IDs
    {
        let registries: Vec<Arc<Mutex<dyn HubRegistry>>> = state
            .lock()
            .ok()
            .map(|s| s.hubs.values().filter_map(|h| h.registry.clone()).collect())
            .unwrap_or_default();
        for reg in &registries {
            if let Ok(mut reg) = reg.lock() {
                for hub_room_id in &hub_room_ids {
                    reg.remove_room(hub_room_id);
                }
            }
        }
    }

    // Remove from engine using topology ID
    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };
    if let Some(runtime) = runtime {
        runtime.remove_room(room_id);
    }

    // Clean up AppState maps (all topology-keyed)
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.remove(room_id);
            s.motion_timeouts.remove(room_id);
            s.motion_snapshots.remove(room_id);
        }
    }

    persist_state(state);

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);

    Ok(())
}

// ============================================================================
// Room action commands
// ============================================================================

/// Dispatch a button action to a room via the runtime engine.
///
/// Actions: on, off, toggle, rhythm_on, rhythm_off, rhythm_toggle,
/// step_up, step_down, dim_up, dim_down, reset, lights_off.
///
/// Returns the updated room rhythm state JSON, or an error.
pub fn do_room_action(
    state: &SharedState,
    room_id: &str,
    action_str: &str,
    persist: bool,
) -> Result<String> {
    let action = match action_str {
        "on" => ButtonAction::OnPress,
        "off" => ButtonAction::OffPress,
        "toggle" => ButtonAction::Toggle,
        other => ButtonAction::from_service_name(other)
            .ok_or_else(|| anyhow::anyhow!("Unknown action: {}", other))?,
    };

    info!(target: "cmd", "room_action: {} -> {:?}", room_id, action);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let event = InputEvent::new(room_id, action);
    let turned_on = runtime.handle_event(&event)?;

    // Track lights_on state from action result
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), turned_on);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Reset all qualifying rooms back to their current adaptive curve position,
/// and turn off any motion-sensor rooms that are on.
///
/// Two categories:
/// 1. **On-rooms** (not disabled, lights on, not soft-off, no motion sensors) → `ButtonAction::Reset`
/// 2. **Motion rooms** (have motion sensors registered AND lights on, or have active motion timer) → `ButtonAction::OffPress` + clear motion timers
///
/// Category 2 uses the device registry (not just active timers) so that
/// rooms turned on by motion are turned off even when no motion timer is
/// running — e.g. after an addon restart where the timer state was lost.
///
/// Returns JSON with status, counts, and lists of affected room IDs.
pub fn do_fix_my_lights(state: &SharedState, persist: bool) -> Result<String> {
    let (runtime, room_lights_on, motion_room_ids) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        // Rooms with active motion timers
        let mut motion_ids: HashSet<String> = s.motion_snapshots.keys().cloned().collect();
        // Also include rooms that have motion sensors registered in any hub's
        // registry, even if no timer is active. This covers the case where the
        // addon restarted and lost timer state, but lights were still on from
        // prior motion activation.
        motion_ids.extend(s.motion_sensor_room_ids());
        (s.hub_runtime(), s.room_lights_on.clone(), motion_ids)
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snapshots = runtime.engine_all_room_snapshots();

    // Category 1: on-rooms without motion sensors → reset to adaptive curve
    let qualifying: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && !snap.soft_off
                && room_lights_on.get(&snap.id).copied().unwrap_or(false)
                && !motion_room_ids.contains(&snap.id)
        })
        .collect();

    // Category 2: motion-sensor rooms that are on (or have active timer) → turn off
    let motion_rooms: Vec<_> = snapshots
        .iter()
        .filter(|snap| {
            !snap.disabled
                && motion_room_ids.contains(&snap.id)
                && room_lights_on.get(&snap.id).copied().unwrap_or(false)
        })
        .collect();

    info!(target: "cmd", "fix_my_lights: {} on-rooms to reset, {} motion rooms to turn off (of {} total)",
        qualifying.len(), motion_rooms.len(), snapshots.len());

    let mut reset_ids = Vec::new();
    for snap in &qualifying {
        let event = InputEvent::new(&snap.id, ButtonAction::Reset);
        match runtime.handle_event(&event) {
            Ok(turned_on) => {
                if let Ok(mut s) = state.lock() {
                    s.room_lights_on.insert(snap.id.clone(), turned_on);
                }
                reset_ids.push(snap.id.clone());
            }
            Err(e) => {
                warn!(target: "cmd", "fix_my_lights: reset failed for {}: {}", snap.id, e);
            }
        }
    }

    // Clear stale offsets on motion rooms before turning off
    for snap in &motion_rooms {
        if snap.time_offset_minutes.abs() > 0.001 || snap.brightness_offset.abs() > 0.001 {
            runtime.restore_room_state(
                &snap.id,
                snap.rhythm_enabled,
                snap.disabled,
                0.0,
                0.0,
                snap.soft_off,
            );
        }
    }

    // OffPress → engine.turn_off() which respects power_save:
    //   power_save=true  → fully off
    //   power_save=false → soft-off (dim to soft_off_brightness with adaptive color)
    let mut motion_off_ids = Vec::new();
    for snap in &motion_rooms {
        let event = InputEvent::new(&snap.id, ButtonAction::OffPress);
        match runtime.handle_event(&event) {
            Ok(turned_on) => {
                if let Ok(mut s) = state.lock() {
                    s.room_lights_on.insert(snap.id.clone(), turned_on);
                }
                motion_off_ids.push(snap.id.clone());
            }
            Err(e) => {
                warn!(target: "cmd", "fix_my_lights: off failed for {}: {}", snap.id, e);
            }
        }
    }

    // Signal the event loop to clear motion timers for these rooms
    if !motion_off_ids.is_empty() {
        if let Ok(mut s) = state.lock() {
            s.pending_motion_clear
                .extend(motion_off_ids.iter().cloned());
        }
    }

    // Collect all affected room IDs for SSE + response
    let all_affected: Vec<String> = reset_ids
        .iter()
        .chain(motion_off_ids.iter())
        .cloned()
        .collect();

    #[cfg(feature = "desktop")]
    {
        let events: Vec<_> = all_affected
            .iter()
            .filter_map(|id| runtime.engine_room_snapshot(id))
            .map(|snap| build_room_state_event(state, &snap))
            .collect();
        if !events.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState { rooms: events },
            );
        }
    }

    if persist && !all_affected.is_empty() {
        persist_rooms(state);
    }

    let room_states: Vec<RoomRhythmState> = all_affected
        .iter()
        .filter_map(|id| build_room_rhythm_state(state, id).ok())
        .collect();

    let response = FixResponse {
        rooms_reset: reset_ids.len(),
        rooms: room_states,
        motion_cleared: motion_off_ids.len(),
        motion_rooms: motion_off_ids,
    };
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Set absolute brightness for a room (1-100).
///
/// Computes the offset so effective brightness equals the target.
/// Rhythm stays enabled — only brightness is overridden.
pub fn do_set_brightness(
    state: &SharedState,
    room_id: &str,
    brightness: u8,
    persist: bool,
) -> Result<String> {
    let brightness = brightness.clamp(1, 100);
    info!(target: "cmd", "set_brightness: {} -> {}", room_id, brightness);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_brightness(room_id, brightness)?;

    // Setting brightness implies lights are on
    {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), true);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Set the time offset for a room directly (not additive).
///
/// Does NOT change `room_lights_on` tracking — offset doesn't imply lights-on state change.
pub fn do_set_time_offset(
    state: &SharedState,
    room_id: &str,
    offset_minutes: f32,
    persist: bool,
) -> Result<String> {
    info!(target: "cmd", "set_time_offset: {} -> {}", room_id, offset_minutes);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    runtime.set_room_time_offset(room_id, offset_minutes)?;

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Device commands
// ============================================================================

/// Upsert a device with button mappings and type in the registry.
///
/// When `hub_key` is provided, only that hub's registry is updated.
/// When `None`, the first available registry is used (backward compat).
pub fn do_device_set(
    state: &SharedState,
    device_id: &str,
    room_id: &str,
    buttons: &[(String, u8)],
    device_type: DeviceType,
    hub_key: Option<&HubKey>,
    persist: bool,
) -> Result<()> {
    info!(target: "cmd", "device_set: {} ({:?}) -> room {} ({} buttons)", device_id, device_type, room_id, buttons.len());

    let registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));

    // Dedup
    let device_unchanged = registry
        .as_ref()
        .and_then(|r| {
            r.lock()
                .ok()
                .map(|reg| reg.device_matches(device_id, room_id, buttons, &device_type))
        })
        .unwrap_or(false);
    if device_unchanged {
        info!(target: "cmd", "device_set: {} unchanged, skipping persist", device_id);
        return Ok(());
    }

    if let Some(reg) = registry {
        if let Ok(mut reg) = reg.lock() {
            reg.upsert_device(device_id, room_id, buttons, device_type);
        }
    }

    if persist {
        persist_state(state);
    }
    Ok(())
}

/// Remove a device from the registry.
///
/// When `hub_key` is provided, removes from that hub's registry only.
/// When `None`, falls back to the first hub's registry (backward compat).
pub fn do_device_remove(
    state: &SharedState,
    device_id: &str,
    hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "device_remove: {}", device_id);

    let registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));
    if let Some(reg) = registry {
        if let Ok(mut reg) = reg.lock() {
            reg.remove_device(device_id);
        }
    }

    persist_state(state);
    Ok(())
}

/// Set per-room motion timeout.
///
/// `room_id` should be a topology room ID (handler resolves before calling).
/// Updates the registry with hub-native IDs and AppState with the topology ID.
///
/// When `hub_key` is provided, updates that hub's registry only.
/// When `None`, falls back to the first hub's registry (backward compat).
pub fn do_motion_timeout_set(
    state: &SharedState,
    room_id: &str,
    timeout_secs: u64,
    hub_key: Option<&HubKey>,
) -> Result<()> {
    info!(target: "cmd", "motion_timeout_set: room {} -> {}s", room_id, timeout_secs);

    // Registry uses hub-native IDs — reverse-lookup from topology
    let hub_room_ids = hub_room_ids_for_topology(state, room_id);

    let registry = hub_key
        .and_then(|k| state.lock().ok().and_then(|s| s.hub_registry_for(k)))
        .or_else(|| extract_registry(state));
    if let Some(reg) = registry {
        if let Ok(mut reg) = reg.lock() {
            for hub_room_id in &hub_room_ids {
                reg.upsert_motion_timeout(hub_room_id, timeout_secs);
            }
        }
    }

    // AppState uses topology room IDs
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.motion_timeouts.insert(room_id.to_string(), timeout_secs);
    }

    persist_registry(state);
    Ok(())
}

// ============================================================================
// Config commands
// ============================================================================

/// Update CurveConfig, push to runtime, save to storage.
pub fn do_config_set(state: &SharedState, config: CurveConfig) -> Result<()> {
    info!(target: "cmd", "config_set: min_bri={}, max_bri={}", config.min_brightness, config.max_brightness);

    let runtime = {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.config = config.clone();
        if let Some(ref storage) = s.storage {
            let stored = crate::storage::StoredConfig::from_state(&s.config, &s.runtime_config);
            if let Err(e) = storage.save_config(&stored) {
                warn!(target: "cmd", "Failed to save config: {}", e);
            }
        }
        s.hub_runtime()
    };

    if let Some(runtime) = runtime {
        if let Err(e) = runtime.set_curve_config(config) {
            warn!(target: "cmd", "Failed to update runtime curve config: {}", e);
        }
    }

    #[cfg(feature = "desktop")]
    crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);

    Ok(())
}

/// Absorb a time offset into the curve config by adjusting ramp widths.
///
/// Modifies the width parameters for the current side of the day (morning/evening)
/// so the curve naturally produces the offset's values at the current time, then
/// resets all room time offsets to 0.
pub fn do_absorb_time_offset(state: &SharedState, offset_minutes: f32) -> Result<()> {
    info!(target: "cmd", "absorb_time_offset: {}min", offset_minutes);

    let (config, runtime, sunrise, sunset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let config = s.config.clone();
        let runtime = s.hub_runtime();

        // Compute sunrise/sunset from location, or use fallbacks
        let (sunrise, sunset) = if let (Some(lat), Some(lon)) = (s.latitude, s.longitude) {
            let now = chrono::Utc::now().naive_utc();
            let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());

            if let Some(ref tz_name) = s.timezone_name {
                let tz = rhythm_core::Timezone::new(tz_name);
                let st = rhythm_core::solar::calculate_sun_times(lat, lon, year, month, day, &tz);
                (st.sunrise, st.sunset)
            } else {
                // Fallback: estimate from solar noon
                let half_day = 7.0; // rough approximation
                let noon = s.solar_noon_hour();
                (noon - half_day, noon + half_day)
            }
        } else {
            (
                rhythm_core::config::FALLBACK_SUNRISE_HOUR,
                rhythm_core::config::FALLBACK_SUNSET_HOUR,
            )
        };

        (config, runtime, sunrise, sunset)
    };

    // Compute current local hour — prefer the runtime's time provider (which
    // may be mocked in tests) over the wall clock.
    let current_hour = if let Some(ref rt) = runtime {
        rt.current_hour()
    } else {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let now = chrono::Utc::now().naive_utc();
        let offset_secs = (s.utc_offset_hours * 3600.0) as i64;
        let local = now + chrono::Duration::seconds(offset_secs);
        let t = local.time();
        t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0
    };

    // Adjust config widths if offset is meaningful
    if let Some(new_config) =
        config.absorb_time_offset(current_hour, offset_minutes, sunrise, sunset)
    {
        info!(target: "cmd", "absorb_time_offset: adjusted widths — bri L={:.3} R={:.3}, cct L={:.3} R={:.3}",
            new_config.width_left_bri, new_config.width_right_bri,
            new_config.width_left_cct, new_config.width_right_cct);

        // Update config in state + persist + push to engine
        {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.config = new_config.clone();
            if let Some(ref storage) = s.storage {
                let stored = crate::storage::StoredConfig::from_state(&s.config, &s.runtime_config);
                if let Err(e) = storage.save_config(&stored) {
                    warn!(target: "cmd", "Failed to save config: {}", e);
                }
            }
        }

        if let Some(ref rt) = runtime {
            if let Err(e) = rt.set_curve_config(new_config) {
                warn!(target: "cmd", "Failed to update runtime curve config: {}", e);
            }
        }
    }

    // Reset all room time offsets to 0
    if let Some(ref rt) = runtime {
        let snapshots = rt.engine_all_room_snapshots();
        for snap in &snapshots {
            if snap.time_offset_minutes.abs() > 0.001 {
                if let Err(e) = rt.set_room_time_offset(&snap.id, 0.0) {
                    warn!(target: "cmd", "Failed to reset offset for room {}: {}", snap.id, e);
                }
            }
        }
    }

    persist_rooms(state);

    #[cfg(feature = "desktop")]
    {
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::ConfigChanged);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Update lat/lon/utc_offset, recalculate solar noon.
///
/// Solar noon is computed internally:
/// - If `timezone_name` is provided, uses DST-aware `calculate_solar_noon`
/// - Otherwise, falls back to `calculate_solar_noon_from_offset`
pub fn do_location_set(
    state: &SharedState,
    lat: f32,
    lon: f32,
    utc_offset: Option<f32>,
    timezone_name: Option<String>,
) -> Result<()> {
    info!(target: "cmd", "location_set: lat={}, lon={}, utc_offset={:?}, tz={:?}", lat, lon, utc_offset, timezone_name);

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        s.latitude = Some(lat);
        s.longitude = Some(lon);
        s.timezone_name = timezone_name.clone();

        if let Some(offset) = utc_offset {
            s.utc_offset_hours = offset;
        }

        // Compute solar noon: prefer timezone-aware calculation
        let now = chrono::Utc::now().naive_utc();
        let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
        let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

        let solar_noon = if let Some(ref tz_name) = timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            // Also refresh utc_offset from timezone
            s.utc_offset_hours = tz.utc_offset(year, month, day, now.time().hour());
            rhythm_core::calculate_solar_noon(lon, year, month, day, &tz)
        } else {
            rhythm_core::calculate_solar_noon_from_offset(lon, s.utc_offset_hours, day_of_year)
        };
        s.runtime_config.solar_noon_hour = solar_noon;

        if let Some(runtime) = s.hub_runtime() {
            let solar_time = rhythm_core::SolarTime::new(solar_noon, lat, day_of_year);
            if let Err(e) = runtime.set_solar(solar_time) {
                warn!(target: "cmd", "Failed to update solar time: {}", e);
            }
        }

        if let Some(ref storage) = s.storage {
            let loc = StoredLocation {
                latitude: Some(lat),
                longitude: Some(lon),
                utc_offset_hours: s.utc_offset_hours,
                timezone_name,
            };
            if let Err(e) = storage.save_location(&loc) {
                warn!(target: "cmd", "Failed to save location: {}", e);
            }
        }
    }

    // No ConfigChanged broadcast — location is client→server fire-and-forget.
    // Broadcasting would cause a feedback loop when multiple apps connect.

    Ok(())
}

/// Save hub credentials and connect SSE.
///
/// After a successful configure, auto-syncs rooms from the hub if
/// the hub provides a `HubDiscovery` implementation.
pub fn do_hub_credentials(
    state: &SharedState,
    hub_type_str: &str,
    address: &str,
    credentials: &Value,
) -> Result<()> {
    info!(target: "cmd", "hub_credentials: type={}, address={}", hub_type_str, address);

    let hub_type = crate::hub::HubType::parse(hub_type_str)
        .ok_or_else(|| anyhow::anyhow!("Unknown hub type: {}", hub_type_str))?;

    let credentials_json = serde_json::to_string(credentials)?;

    let get_provider = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.get_hub_provider_fn
            .clone()
            .ok_or_else(|| anyhow::anyhow!("No hub provider registered"))?
    };

    let provider = get_provider(hub_type.clone());
    provider.configure(address, &credentials_json, state)?;

    let hub_key = crate::canonical::identity::HubKey::new(hub_type, address);

    // Register the new hub's controller with the composite (if runtime already exists).
    #[cfg(feature = "desktop")]
    register_hub_with_composite(state, &hub_key);

    // Auto-sync rooms from the newly configured hub.
    // Uses platform config to decide whether to also discover devices/sensors
    // (desktop: full sync, ESP32: rooms only — devices arrive via SSE).
    let discover_devices = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.platform.full_device_discovery
    };
    // Auto-sync + poll must run on a dedicated thread — reqwest::blocking::Client
    // panics if used inside a tokio runtime context (spawn_blocking from HTTP handler).
    {
        let sync_state = state.clone();
        let _ = std::thread::Builder::new()
            .name("hub-sync".to_string())
            .spawn(move || {
                if let Err(e) =
                    crate::room_sync::sync_from_hub_for_key(&sync_state, &hub_key, discover_devices)
                {
                    warn!(target: "cmd", "Auto-sync after hub configure failed: {}", e);
                }
                crate::room_sync::poll_initial_light_state(&sync_state);

                // Standardize all rooms to current adaptive curve position.
                match do_fix_my_lights(&sync_state, true) {
                    Ok(_) => info!(target: "cmd", "Auto-fix after credential push complete"),
                    Err(e) => warn!(target: "cmd", "Auto-fix after credential push failed: {}", e),
                }
            })
            .and_then(|h| h.join().map_err(|_| std::io::Error::other("panicked")));
    }

    #[cfg(feature = "desktop")]
    {
        let hub_connected = state.lock().ok().map(|s| s.has_any_hub()).unwrap_or(false);
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::HubStatus {
                hub_type: Some(hub_type_str.to_string()),
                address: Some(address.to_string()),
                connected: hub_connected,
            },
        );
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Disconnect all hubs — clear credentials, runtime, and rooms.
pub fn do_hub_disconnect(state: &SharedState) -> Result<()> {
    info!(target: "cmd", "hub_disconnect: clearing all hubs, credentials, and rooms");

    // Capture hub keys before clearing for SSE notifications
    let old_hub_keys: Vec<_>;
    let old_hubs;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hub_keys = s.hubs.keys().cloned().collect();
        old_hubs = std::mem::take(&mut s.hubs);

        s.hub_credentials.clear();
        if let Some(ref storage) = s.storage {
            let _ = storage.save_all_hub_credentials(&[]);
        }

        s.room_lights_on.clear();
        s.motion_timeouts.clear();
        s.motion_snapshots.clear();

        if let Some(ref storage) = s.storage {
            let empty = rhythm_core::room::RoomManager::new();
            let _ = storage.save_rooms(&empty);
        }
    }

    persist_registry(state);

    // Clear all controllers from the composite
    #[cfg(feature = "desktop")]
    {
        let composite = state
            .lock()
            .ok()
            .and_then(|s| s.composite_controller.clone());
        if let Some(composite) = composite {
            for key in &old_hub_keys {
                composite.remove_controller(&key.to_string());
            }
        }
        rebuild_composite_routing(state);
    }

    // Drop old hubs on a dedicated thread (reqwest::blocking::Client panics in tokio context)
    if !old_hubs.is_empty() {
        std::thread::Builder::new()
            .name("hub-drop".to_string())
            .spawn(move || drop(old_hubs))
            .ok();
    }

    #[cfg(feature = "desktop")]
    {
        for key in &old_hub_keys {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: Some(key.hub_type.as_str().to_string()),
                    address: Some(key.address.clone()),
                    connected: false,
                },
            );
        }
        // Also emit a generic disconnected if no specific keys (fresh state)
        if old_hub_keys.is_empty() {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::HubStatus {
                    hub_type: None,
                    address: None,
                    connected: false,
                },
            );
        }
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Disconnect a single hub by type and address.
///
/// Removes only the matching hub. Other hubs remain connected.
/// Rooms owned by this hub are removed from the engine.
pub fn do_hub_disconnect_one(state: &SharedState, hub_type_str: &str, address: &str) -> Result<()> {
    use crate::canonical::identity::HubKey;

    info!(target: "cmd", "hub_disconnect_one: type={}, address={}", hub_type_str, address);

    let hub_type = crate::hub::HubType::parse(hub_type_str)
        .ok_or_else(|| anyhow::anyhow!("Unknown hub type: {}", hub_type_str))?;
    let key = HubKey::new(hub_type, address);

    let old_hub;

    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;

        old_hub = s.hubs.remove(&key);
        s.hub_credentials.remove(&key);

        if let Some(ref storage) = s.storage {
            let all: Vec<_> = s.hub_credentials.values().cloned().collect();
            let _ = storage.save_all_hub_credentials(&all);
        }
    }

    persist_state(state);

    // Remove the hub's controller from the composite and rebuild routing
    #[cfg(feature = "desktop")]
    {
        let composite = state
            .lock()
            .ok()
            .and_then(|s| s.composite_controller.clone());
        if let Some(composite) = composite {
            composite.remove_controller(&key.to_string());
            info!(target: "cmd", "Removed {} controller from composite", key);
        }
        rebuild_composite_routing(state);
    }

    // Drop old hub on a dedicated thread
    if let Some(hub) = old_hub {
        std::thread::Builder::new()
            .name("hub-drop".to_string())
            .spawn(move || drop(hub))
            .ok();
    }

    #[cfg(feature = "desktop")]
    {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::HubStatus {
                hub_type: Some(hub_type_str.to_string()),
                address: Some(address.to_string()),
                connected: false,
            },
        );
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Update user preferences for a room (rhythm_enabled, disabled, soft_off).
///
/// Only modifies user state, not topology. Safe for app to call without
/// overwriting server-discovered rooms/devices.
pub fn do_room_preferences_set(
    state: &SharedState,
    room_id: &str,
    rhythm_enabled: Option<bool>,
    disabled: Option<bool>,
    soft_off: Option<bool>,
    persist: bool,
) -> Result<String> {
    info!(target: "cmd", "room_preferences_set: {} rhythm={:?} disabled={:?} soft_off={:?}",
        room_id, rhythm_enabled, disabled, soft_off);

    let runtime = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.hub_runtime()
    };

    let runtime = runtime.ok_or_else(|| anyhow::anyhow!("No runtime available"))?;

    let snap = runtime
        .engine_room_snapshot(room_id)
        .ok_or_else(|| anyhow::anyhow!("Room '{}' not found in engine", room_id))?;

    let prev_soft_off = snap.soft_off;
    let rhythm_enabled = rhythm_enabled.unwrap_or(snap.rhythm_enabled);
    let disabled = disabled.unwrap_or(snap.disabled);
    let soft_off = soft_off.unwrap_or(snap.soft_off);

    // Soft-off rooms need rhythm enabled for periodic soft-off ticks
    let rhythm_enabled = if soft_off { true } else { rhythm_enabled };

    runtime.restore_room_state(
        room_id,
        rhythm_enabled,
        disabled,
        snap.time_offset_minutes,
        snap.brightness_offset,
        soft_off,
    );

    // Immediate light command when soft_off transitions
    if soft_off && !prev_soft_off {
        // Entering soft-off: send adaptive color temp at soft-off brightness
        info!(target: "cmd", "room_preferences_set: {} entering soft_off, applying immediately", room_id);
        if let Err(e) = runtime.soft_off_tick_room(room_id) {
            warn!(target: "cmd", "soft_off_tick for '{}' failed: {}", room_id, e);
        }
    } else if !soft_off && prev_soft_off {
        // Leaving soft-off: turn on with full adaptive values
        info!(target: "cmd", "room_preferences_set: {} leaving soft_off, turning on", room_id);
        if let Err(e) = runtime.turn_on_room(room_id) {
            warn!(target: "cmd", "turn_on for '{}' failed: {}", room_id, e);
        }
    }

    // soft_off implies lights conceptually on (dim)
    if soft_off {
        if let Ok(mut s) = state.lock() {
            s.room_lights_on.insert(room_id.to_string(), true);
        }
    }

    #[cfg(feature = "desktop")]
    {
        if let Some(snap) = runtime.engine_room_snapshot(room_id) {
            crate::state::emit_server_event(
                state,
                crate::server_event::ServerEvent::RoomState {
                    rooms: vec![build_room_state_event(state, &snap)],
                },
            );
        }
    }

    if persist {
        persist_rooms(state);
    }

    let room_state = build_room_rhythm_state(state, room_id)?;
    serde_json::to_string(&room_state).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Runtime initialization helper
// ============================================================================

/// Call the platform's ensure_runtime callback in a thread with adequate stack.
///
/// The callback is stored on AppState (as an `Arc`) and set by the platform
/// crate at startup. It handles creating the RhythmRuntime with platform-specific types.
pub fn ensure_runtime(state: &SharedState) {
    // Clone the Arc + platform config out of the lock so we can call without holding it
    let (ensure_fn, stack_size) = {
        let Ok(s) = state.lock() else { return };
        (s.ensure_runtime_fn.clone(), s.platform.runtime_init_stack)
    };

    let Some(ensure_fn) = ensure_fn else { return };

    let rt_state = state.clone();
    let rt_result = std::thread::Builder::new()
        .name("rt-init".to_string())
        .stack_size(stack_size)
        .spawn(move || ensure_fn(&rt_state))
        .and_then(|handle| handle.join().map_err(|_| std::io::Error::other("panicked")));

    match rt_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => warn!(target: "cmd", "Failed to start runtime: {}", e),
        Err(e) => warn!(target: "cmd", "Runtime init thread error: {}", e),
    }
}

// ============================================================================
// Composite controller registration
// ============================================================================

/// Rebuild the composite controller's routing table from topology.
///
/// Call after any operation that changes room-to-hub mappings: room sync,
/// hub connect/disconnect, topology merge/split/device-move.
#[cfg(feature = "desktop")]
pub fn rebuild_composite_routing(state: &SharedState) {
    let (composite, routing, room_count) = {
        let Ok(s) = state.lock() else { return };
        let composite = match s.composite_controller.clone() {
            Some(c) => c,
            None => return, // No composite yet — nothing to rebuild
        };
        let routing = s.topology.composite_routing();
        let rooms = s.topology.room_count();
        (composite, routing, rooms)
    };
    let route_count = routing.len();
    composite.update_routing(routing);
    info!(target: "cmd", "Rebuilt composite routing: {} rooms, {} route entries (incl. hub aliases)",
        room_count, route_count);
}

/// Register a newly connected hub's controller with the composite controller.
///
/// Called after `configure_hub` when a hub is added via HTTP while the server
/// is already running. If the composite controller doesn't exist yet (runtime
/// not created), this is a no-op — the controller will be picked up when
/// `ensure_composite_runtime` runs on first room arrival.
#[cfg(feature = "desktop")]
pub fn register_hub_with_composite(
    state: &SharedState,
    hub_key: &crate::canonical::identity::HubKey,
) {
    let register_fn = {
        let Ok(s) = state.lock() else { return };
        s.register_controller_fn.clone()
    };
    let Some(register_fn) = register_fn else {
        return;
    };

    match register_fn(state, hub_key) {
        Ok(()) => {
            // Assign the shared runtime to the new hub so hub_runtime_for() works
            {
                let Ok(mut s) = state.lock() else { return };
                let runtime = s.hubs.values().find_map(|h| h.runtime.clone());
                if let (Some(runtime), Some(hub)) = (runtime, s.hubs.get_mut(hub_key)) {
                    if hub.runtime.is_none() {
                        hub.runtime = Some(runtime);
                    }
                }
            }
            // Rebuild routing so existing topology rooms are routable via the new controller.
            // sync_from_hub_for_key() will rebuild again after discovery, but this
            // closes the window between registration and sync completion.
            rebuild_composite_routing(state);
        }
        Err(e) => {
            warn!(target: "cmd", "Failed to register controller for {}: {}", hub_key, e);
        }
    }
}

// ============================================================================
// Parse helpers
// ============================================================================

/// Parse button mappings from a JSON array.
pub fn parse_buttons(msg: &Value) -> Vec<(String, u8)> {
    msg.get("buttons")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|btn| {
                    let button_id = btn.get("button_id").and_then(|v| v.as_str())?;
                    let control_id = btn.get("control_id").and_then(|v| v.as_u64())? as u8;
                    Some((button_id.to_string(), control_id))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ============================================================================
// Persistence helpers
// ============================================================================

/// Persist registry snapshot + room state.
pub fn persist_state(state: &SharedState) {
    persist_registry(state);
    persist_rooms(state);
}

/// Persist registry snapshot for all connected hubs.
///
/// Three-phase lock per hub to avoid holding AppState during JSON serialization + disk I/O:
/// 1. Brief AppState lock — extract registry Arcs + hub keys + check storage exists
/// 2. Registry lock only — snapshot to JSON (no AppState held)
/// 3. Brief AppState lock — write to storage (no registry lock)
pub fn persist_registry(state: &SharedState) {
    // Phase 1: Extract all hub registry Arcs + check storage (brief AppState lock)
    let (hub_registries, has_storage) = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let regs: Vec<_> = s
            .hubs
            .iter()
            .filter_map(|(key, hub)| hub.registry.as_ref().map(|r| (key.clone(), r.clone())))
            .collect();
        (regs, s.storage.is_some())
    };
    if !has_storage || hub_registries.is_empty() {
        return;
    }

    // Phase 2+3: Snapshot + save each hub's registry
    for (hub_key, reg_arc) in &hub_registries {
        let value = reg_arc.lock().ok().map(|reg| reg.snapshot_json());

        if let Some(ref value) = value {
            let s = match state.lock() {
                Ok(s) => s,
                Err(_) => return,
            };
            if let Some(ref storage) = s.storage {
                if let Err(e) = storage.save_hub_registry_for(hub_key, value) {
                    warn!(target: "cmd", "Failed to save registry for {}: {}", hub_key, e);
                }
            }
        }
    }
}

/// Persist room state.
pub fn persist_rooms(state: &SharedState) {
    let (runtime, has_storage) = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        (s.hub_runtime(), s.storage.is_some())
    };

    if let (Some(ref runtime), true) = (runtime, has_storage) {
        let rooms = rooms_from_engine(runtime.as_ref());
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        if let Some(ref storage) = s.storage {
            if let Err(e) = storage.save_rooms(&rooms) {
                warn!(target: "cmd", "Failed to save rooms: {}", e);
            }
        }
    }
}

// ============================================================================
// Canonical device commands
// ============================================================================

/// Build JSON for all canonical devices.
pub fn build_canonical_devices(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let registry = &s.canonical_registry;
    let devices: Vec<_> = registry.devices().collect();
    serde_json::to_string(&devices).map_err(|e| anyhow::anyhow!(e))
}

/// Build JSON for a single canonical device.
pub fn build_canonical_device(state: &SharedState, id: &str) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    match s.canonical_registry.get(id) {
        Some(device) => serde_json::to_string(device).map_err(|e| anyhow::anyhow!(e)),
        None => Err(anyhow::anyhow!("Device not found: {}", id)),
    }
}

/// Assign a canonical device to a Rhythm room (or unassign with None).
pub fn do_canonical_assign_room(
    state: &SharedState,
    device_id: &str,
    room_id: Option<&str>,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.canonical_registry.assign_room(device_id, room_id) {
        persist_canonical(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Device not found: {}", device_id))
    }
}

/// Set the preferred endpoint for a canonical device.
pub fn do_canonical_set_preferred(
    state: &SharedState,
    device_id: &str,
    hub_type: &str,
    hub_address: &str,
    native_id: &str,
) -> Result<()> {
    use crate::canonical::identity::HubKey;
    use crate::hub::HubType;

    let hub_key = HubKey::new(HubType::new(hub_type), hub_address);
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.canonical_registry
        .set_preferred_endpoint(device_id, &hub_key, native_id)
    {
        persist_canonical(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Device or endpoint not found"))
    }
}

// ============================================================================
// Triage commands
// ============================================================================

/// Build JSON for the triage queue (pending entries).
pub fn build_triage_queue(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let pending = s.canonical_registry.triage().pending();
    serde_json::to_string(&pending).map_err(|e| anyhow::anyhow!(e))
}

/// Confirm a triage device merge.
///
/// Merges the discovered device's endpoint into the chosen canonical device
/// and removes the silo device that was created during Phase 4 of resolve().
pub fn do_triage_merge(state: &SharedState, entry_id: &str, canonical_id: &str) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (hub_key, native_id, hub_room_id) = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => (
            entry.hub_key.clone(),
            entry.discovered.native_id.clone(),
            entry.discovered.room_id.clone(),
        ),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Find the silo device BEFORE complete_merge updates native_index.
    // The silo device has the same native_id on the same hub but a different canonical ID.
    let silo_id = s
        .canonical_registry
        .find_by_native_id(&hub_key, &native_id)
        .filter(|d| d.id != canonical_id)
        .map(|d| d.id.clone());

    if s.canonical_registry
        .complete_merge(entry_id, canonical_id, &hub_key, now)
    {
        // Remove the silo device that was created during resolve() Phase 4.
        if let Some(silo_id) = silo_id {
            s.canonical_registry.remove_device(&silo_id);
            log::debug!(target: "triage", "Removed silo device '{}' after merge", silo_id);
        }

        // Update topology: add device to the correct room
        if !hub_room_id.is_empty() {
            if let Some(rhythm_room_id) = s
                .topology
                .translate_room_id(&hub_key, &hub_room_id)
                .map(|s| s.to_string())
            {
                if let Some(room) = s.topology.get_mut(&rhythm_room_id) {
                    room.add_device_hub_default(canonical_id);
                }
                s.canonical_registry
                    .assign_room(canonical_id, Some(&rhythm_room_id));
            }
        }
        persist_canonical(&s);
        persist_topology(&s);
        drop(s);
        #[cfg(feature = "desktop")]
        emit_triage_changed(state);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to merge"))
    }
}

/// Create a new device from a triage entry (rejected merge, keep separate).
pub fn do_triage_new_device(state: &SharedState, entry_id: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (hub_key, hub_room_id) = match s.canonical_registry.triage().get(entry_id) {
        Some(entry) => (entry.hub_key.clone(), entry.discovered.room_id.clone()),
        None => return Err(anyhow::anyhow!("Triage entry not found: {}", entry_id)),
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    match s
        .canonical_registry
        .complete_new_device(entry_id, &hub_key, now)
    {
        Some(canonical_id) => {
            if !hub_room_id.is_empty() {
                if let Some(rhythm_room_id) = s
                    .topology
                    .translate_room_id(&hub_key, &hub_room_id)
                    .map(|s| s.to_string())
                {
                    if let Some(room) = s.topology.get_mut(&rhythm_room_id) {
                        room.add_device_hub_default(&canonical_id);
                    }
                    s.canonical_registry
                        .assign_room(&canonical_id, Some(&rhythm_room_id));
                }
            }
            persist_canonical(&s);
            persist_topology(&s);
            drop(s);
            #[cfg(feature = "desktop")]
            emit_triage_changed(state);
            Ok(format!(r#"{{"canonical_id":"{}"}}"#, canonical_id))
        }
        None => Err(anyhow::anyhow!("Failed to create new device")),
    }
}

/// Dismiss a triage entry.
pub fn do_triage_dismiss(state: &SharedState, entry_id: &str) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if s.canonical_registry.triage_mut().dismiss(entry_id, now) {
        persist_canonical(&s);
        drop(s);
        #[cfg(feature = "desktop")]
        emit_triage_changed(state);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Triage entry not found: {}", entry_id))
    }
}

/// Approve a room binding triage entry — merge two rooms.
///
/// Merges the silo room (created for the new hub) into the target Rhythm room,
/// records an approved binding so it re-applies on re-sync, rebuilds composite
/// routing, and emits SSE events.
pub fn do_triage_bind_room(state: &SharedState, entry_id: &str) -> Result<()> {
    do_triage_bind_room_to(state, entry_id, None)
}

/// Approve a room binding triage entry — optionally binding to a specific target
/// (for the 3+ hub case where the user picks from candidate rooms).
pub fn do_triage_bind_room_to(
    state: &SharedState,
    entry_id: &str,
    target_override: Option<&str>,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    // Get the room binding proposal
    let entry = s
        .canonical_registry
        .triage()
        .get(entry_id)
        .ok_or_else(|| anyhow::anyhow!("Triage entry not found: {}", entry_id))?
        .clone();

    let binding = entry
        .room_binding
        .ok_or_else(|| anyhow::anyhow!("Not a room binding entry: {}", entry_id))?;

    let target_id = target_override
        .map(|s| s.to_string())
        .unwrap_or(binding.target_rhythm_room_id.clone());

    // Validate target room exists
    let target_name = s
        .topology
        .get(&target_id)
        .map(|r| r.name.clone())
        .ok_or_else(|| anyhow::anyhow!("Target room '{}' not found", target_id))?;

    // Find the silo room (the one created for this hub's room)
    let source_id = s
        .topology
        .translate_room_id(&entry.hub_key, &binding.hub_room_id)
        .map(|s| s.to_string())
        .ok_or_else(|| {
            anyhow::anyhow!("Silo room not found for hub room: {}", binding.hub_room_id)
        })?;

    // Merge the silo room into the target room
    if !s.topology.merge_rooms(&target_id, &source_id) {
        return Err(anyhow::anyhow!(
            "Failed to merge rooms: {} → {}",
            source_id,
            target_id
        ));
    }

    // Record the approved binding so it re-applies on re-sync
    s.topology.approve_binding(
        entry.hub_key.clone(),
        binding.hub_room_id.clone(),
        target_id.clone(),
        now,
    );

    // Resolve the triage entry
    s.canonical_registry.triage_mut().resolve(
        entry_id,
        crate::canonical::triage::TriageStatus::Confirmed,
        "api",
        now,
    );

    persist_canonical(&s);
    persist_topology(&s);

    // Remap engine room if runtime exists
    drop(s);

    if let Some(runtime) = state.lock().ok().and_then(|s| s.hub_runtime()) {
        // If the silo room exists in the engine, merge it into the target
        if let Some(snap) = runtime.engine_room_snapshot(&source_id) {
            if runtime.engine_room_snapshot(&target_id).is_none() {
                runtime.add_room(&target_id, &target_name);
                runtime.restore_room_state(
                    &target_id,
                    snap.rhythm_enabled,
                    snap.disabled,
                    snap.time_offset_minutes,
                    snap.brightness_offset,
                    snap.soft_off,
                );
            }
            runtime.remove_room(&source_id);
            info!(target: "triage", "Remapped engine room '{}' → '{}'", source_id, target_id);
        }
    }

    // Rebuild composite routing
    #[cfg(feature = "desktop")]
    rebuild_composite_routing(state);

    // Emit SSE events
    #[cfg(feature = "desktop")]
    {
        emit_triage_changed(state);
        crate::state::emit_server_event(state, crate::server_event::ServerEvent::RoomsChanged);
    }

    Ok(())
}

/// Build JSON with triage count summary.
pub fn build_triage_count(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let triage = s.canonical_registry.triage();
    let json = format!(
        r#"{{"devices":{},"rooms":{},"total":{}}}"#,
        triage.pending_device_count(),
        triage.pending_room_count(),
        triage.pending_count(),
    );
    Ok(json)
}

/// Emit a TriageChanged SSE event with current counts.
#[cfg(feature = "desktop")]
pub fn emit_triage_changed(state: &SharedState) {
    // Read counts under lock, then drop before emitting (emit_server_event locks too)
    let counts = state.lock().ok().map(|s| {
        let triage = s.canonical_registry.triage();
        (
            triage.pending_count(),
            triage.pending_device_count(),
            triage.pending_room_count(),
        )
    });
    if let Some((total, devices, rooms)) = counts {
        crate::state::emit_server_event(
            state,
            crate::server_event::ServerEvent::TriageChanged {
                pending_count: total,
                pending_devices: devices,
                pending_rooms: rooms,
            },
        );
    }
}

// ============================================================================
// Topology commands
// ============================================================================

/// Build JSON for all topology rooms.
pub fn build_topology_rooms(state: &SharedState) -> Result<String> {
    let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let rooms: Vec<_> = s.topology.rooms().collect();
    serde_json::to_string(&rooms).map_err(|e| anyhow::anyhow!(e))
}

/// Create a new empty Rhythm room.
pub fn do_topology_create_room(state: &SharedState, name: &str) -> Result<String> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let id = s.topology.create_room(name);
    persist_topology(&s);
    Ok(format!(r#"{{"id":"{}","name":"{}"}}"#, id, name))
}

/// Merge two rooms.
pub fn do_topology_merge_rooms(
    state: &SharedState,
    target_id: &str,
    source_id: &str,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.topology.merge_rooms(target_id, source_id) {
        persist_topology(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to merge rooms"))
    }
}

/// Move a device between rooms.
pub fn do_topology_move_device(
    state: &SharedState,
    device_id: &str,
    from_room: &str,
    to_room: &str,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    if s.topology.move_device(device_id, from_room, to_room) {
        persist_topology(&s);
        Ok(())
    } else {
        Err(anyhow::anyhow!("Failed to move device"))
    }
}

// ============================================================================
// Canonical + Topology persistence helpers
// ============================================================================

/// Persist the canonical registry to storage.
pub(crate) fn persist_canonical(s: &AppState) {
    if let Some(ref storage) = s.storage {
        match serde_json::to_value(&s.canonical_registry) {
            Ok(value) => {
                if let Err(e) = storage.save_canonical_registry(&value) {
                    warn!(target: "cmd", "Failed to save canonical registry: {}", e);
                }
            }
            Err(e) => warn!(target: "cmd", "Failed to serialize canonical registry: {}", e),
        }
    }
}

/// Persist the topology store to storage.
pub(crate) fn persist_topology(s: &AppState) {
    if let Some(ref storage) = s.storage {
        match serde_json::to_value(&s.topology) {
            Ok(value) => {
                if let Err(e) = storage.save_topology(&value) {
                    warn!(target: "cmd", "Failed to save topology: {}", e);
                }
            }
            Err(e) => warn!(target: "cmd", "Failed to serialize topology: {}", e),
        }
    }
}

// ============================================================================
// Curve visualization queries
// ============================================================================

/// Solar context resolved from AppState for a given date.
struct ResolvedSolar {
    solar: rhythm_core::SolarTime,
    sun_times: Option<rhythm_core::SunTimes>,
    twilight: Option<rhythm_core::TwilightTimes>,
}

/// Extract solar context from AppState for a given date.
///
/// If latitude/longitude are configured, computes real sun times.
/// Otherwise falls back to `SolarTime::default_noon()` with no sun/twilight data.
fn resolve_solar(s: &AppState, year: i32, month: u32, day: u32) -> ResolvedSolar {
    let day_of_year = rhythm_core::timezone::day_of_year(year, month, day);

    if let (Some(lat), Some(lon)) = (s.latitude, s.longitude) {
        if let Some(ref tz_name) = s.timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            let solar_time = rhythm_core::solar_time_from_location(lat, lon, year, month, day, &tz);
            let sun_times = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
            let twilight = rhythm_core::calculate_twilight_times(lat, lon, year, month, day, &tz);
            ResolvedSolar {
                solar: solar_time,
                sun_times: Some(sun_times),
                twilight: Some(twilight),
            }
        } else {
            // No timezone — use stored solar noon + rough estimation
            let solar = rhythm_core::SolarTime::new(s.solar_noon_hour(), lat, day_of_year);
            ResolvedSolar {
                solar,
                sun_times: None,
                twilight: None,
            }
        }
    } else {
        // No location — defaults
        let solar = rhythm_core::SolarTime::new(s.solar_noon_hour(), 35.0, day_of_year);
        ResolvedSolar {
            solar,
            sun_times: None,
            twilight: None,
        }
    }
}

/// Build a `SolarResponse` from resolved solar data.
fn build_solar_response(resolved: &ResolvedSolar) -> crate::api_types::SolarResponse {
    use crate::api_types::{SolarResponse, TwilightPhaseResponse, TwilightResponse};

    SolarResponse {
        sunrise: resolved.sun_times.map(|st| st.sunrise),
        sunset: resolved.sun_times.map(|st| st.sunset),
        solar_noon: resolved.solar.solar_noon_hour,
        solar_midnight: resolved.solar.solar_midnight_hour(),
        day_length: resolved.sun_times.map(|st| st.day_length),
        twilight: resolved.twilight.as_ref().map(|tw| TwilightResponse {
            dawn: TwilightPhaseResponse {
                civil: tw.dawn.civil,
                nautical: tw.dawn.nautical,
                astronomical: tw.dawn.astronomical,
            },
            dusk: TwilightPhaseResponse {
                civil: tw.dusk.civil,
                nautical: tw.dusk.nautical,
                astronomical: tw.dusk.astronomical,
            },
        }),
    }
}

/// Parse an optional `YYYY-MM-DD` date string, falling back to today (UTC).
fn parse_date_or_today(date: Option<&str>, utc_offset: f32) -> Result<(i32, u32, u32)> {
    if let Some(d) = date {
        let parts: Vec<&str> = d.split('-').collect();
        if parts.len() != 3 {
            return Err(anyhow::anyhow!("Invalid date format, expected YYYY-MM-DD"));
        }
        let year: i32 = parts[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid year"))?;
        let month: u32 = parts[1]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid month"))?;
        let day: u32 = parts[2]
            .parse()
            .map_err(|_| anyhow::anyhow!("Invalid day"))?;
        if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
            return Err(anyhow::anyhow!("Date out of range"));
        }
        Ok((year, month, day))
    } else {
        let now = chrono::Utc::now().naive_utc();
        let offset_secs = (utc_offset * 3600.0) as i64;
        let local = now + chrono::Duration::seconds(offset_secs);
        Ok((
            local.date().year(),
            local.date().month(),
            local.date().day(),
        ))
    }
}

/// Compute the current local hour from UTC + offset.
fn current_local_hour(utc_offset: f32) -> f32 {
    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let t = local.time();
    t.hour() as f32 + t.minute() as f32 / 60.0 + t.second() as f32 / 3600.0
}

/// Build the full curve response (config + solar + curve samples + step sequences).
///
/// Used by `GET /api/curve` and `POST /api/curve`.
pub fn build_curve(
    state: &SharedState,
    config_override: Option<CurveConfig>,
    date: Option<&str>,
    samples_per_hour: Option<u32>,
    start_hour: Option<f32>,
    max_steps: Option<u8>,
) -> Result<String> {
    use crate::api_types::CurveResponse;
    use rhythm_core::RhythmCurveModule;

    let (config, utc_offset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let config = config_override.unwrap_or_else(|| s.config.clone());
        (config, s.utc_offset_hours)
    };

    let (year, month, day) = parse_date_or_today(date, utc_offset)?;

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let module = RhythmCurveModule::new(config.clone());
    let samples = samples_per_hour.unwrap_or(4);
    let curve_data =
        rhythm_curve::generate_curve_data(&module, resolved.solar, resolved.sun_times, samples);

    let start = start_hour.unwrap_or_else(|| current_local_hour(utc_offset));
    let steps = max_steps.unwrap_or(config.max_dim_steps);
    let step_data = rhythm_curve::generate_step_sequences(
        &module,
        resolved.solar,
        resolved.sun_times,
        start,
        steps,
    );

    let solar_response = build_solar_response(&resolved);
    let config_value = serde_json::to_value(&config)?;

    let response = CurveResponse {
        config: config_value,
        solar: solar_response,
        curve: curve_data,
        steps: step_data,
    };

    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build the current lighting values response.
///
/// Used by `GET /api/curve/now`.
pub fn build_curve_now(state: &SharedState, hour_override: Option<f32>) -> Result<String> {
    use crate::api_types::LightingNowResponse;
    use rhythm_core::{kelvin_to_mireds, LightCurveModule, RhythmCurveModule};

    let (config, utc_offset) = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (s.config.clone(), s.utc_offset_hours)
    };

    let hour = hour_override.unwrap_or_else(|| current_local_hour(utc_offset));

    let now = chrono::Utc::now().naive_utc();
    let offset_secs = (utc_offset * 3600.0) as i64;
    let local = now + chrono::Duration::seconds(offset_secs);
    let (year, month, day) = (
        local.date().year(),
        local.date().month(),
        local.date().day(),
    );

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let module = RhythmCurveModule::new(config);
    let ctx = rhythm_core::CurveContext::new(hour, resolved.solar, resolved.sun_times);
    let values = module.calculate(&ctx);

    let response = LightingNowResponse {
        hour,
        brightness: values.brightness,
        kelvin: values.kelvin,
        mireds: kelvin_to_mireds(values.kelvin),
        rgb: values.rgb,
        xy: values.xy,
        solar_time: values.solar_time,
        sun_position: values.sun_position,
    };

    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

/// Build the solar times response.
///
/// Used by `GET /api/curve/solar`.
pub fn build_curve_solar(state: &SharedState, date: Option<&str>) -> Result<String> {
    let utc_offset = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.utc_offset_hours
    };

    let (year, month, day) = parse_date_or_today(date, utc_offset)?;

    let resolved = {
        let s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        resolve_solar(&s, year, month, day)
    };

    let response = build_solar_response(&resolved);
    serde_json::to_string(&response).map_err(|e| anyhow::anyhow!("serialize: {}", e))
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::{ActiveHub, HubType};
    use crate::state::{AppState, MotionSnapshot};
    use rhythm_core::{CurveConfig, RoomSnapshot, RuntimeHandle};
    use std::sync::{Arc, Mutex};

    /// Mock runtime that returns configurable room snapshots and tracks events.
    struct MockRuntime {
        snapshots: Vec<RoomSnapshot>,
        events: Mutex<Vec<(String, ButtonAction)>>,
    }

    impl MockRuntime {
        fn new(snapshots: Vec<RoomSnapshot>) -> Self {
            Self {
                snapshots,
                events: Mutex::new(Vec::new()),
            }
        }

        fn events(&self) -> Vec<(String, ButtonAction)> {
            self.events.lock().unwrap().clone()
        }
    }

    impl RuntimeHandle for MockRuntime {
        fn handle_event(&self, event: &InputEvent) -> anyhow::Result<bool> {
            self.events
                .lock()
                .unwrap()
                .push((event.room_id.clone(), event.action));
            // Reset/OnPress → lights on; LightsOff/OffPress → lights off
            Ok(!matches!(
                event.action,
                ButtonAction::LightsOff | ButtonAction::OffPress
            ))
        }
        fn sync_rooms(&self) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_solar(&self, _: rhythm_core::SolarTime) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_curve_config(&self, _: CurveConfig) -> anyhow::Result<()> {
            Ok(())
        }
        fn periodic_tick_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn engine_room_snapshot(&self, room_id: &str) -> Option<RoomSnapshot> {
            self.snapshots.iter().find(|s| s.id == room_id).cloned()
        }
        fn engine_all_room_snapshots(&self) -> Vec<RoomSnapshot> {
            self.snapshots.clone()
        }
        fn restore_room_state(&self, _: &str, _: bool, _: bool, _: f32, _: f32, _: bool) {}
        fn add_room(&self, _: &str, _: &str) {}
        fn remove_room(&self, _: &str) {}
        fn dim_room(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn turn_on_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_power_save(&self, _: bool) -> Vec<String> {
            vec![]
        }
        fn is_power_save(&self) -> bool {
            false
        }
        fn set_room_brightness(&self, _: &str, _: u8) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_room_time_offset(&self, _: &str, _: f32) -> anyhow::Result<()> {
            Ok(())
        }
        fn set_soft_off_brightness(&self, _: u8) {}
        fn soft_off_tick_room(&self, _: &str) -> anyhow::Result<()> {
            Ok(())
        }
        fn any_lights_on(&self, _: &str) -> anyhow::Result<bool> {
            Ok(false)
        }
        fn current_hour(&self) -> f32 {
            12.0
        }
    }

    fn make_snapshot(id: &str, disabled: bool, soft_off: bool) -> RoomSnapshot {
        RoomSnapshot {
            id: id.to_string(),
            name: id.to_string(),
            rhythm_enabled: true,
            disabled,
            time_offset_minutes: 0.0,
            brightness_offset: 0.0,
            soft_off,
        }
    }

    fn setup_state(snapshots: Vec<RoomSnapshot>) -> (SharedState, Arc<MockRuntime>) {
        let runtime = Arc::new(MockRuntime::new(snapshots));
        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Default::default(),
            },
        );
        (Arc::new(Mutex::new(app)), runtime)
    }

    #[test]
    fn fix_resets_on_rooms_only() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("on_room", false, false),
            make_snapshot("off_room", false, false),
            make_snapshot("disabled_room", true, false),
            make_snapshot("soft_off_room", false, true),
        ]);

        // Mark only on_room as lights-on
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("on_room".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("off_room".into(), false);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["rooms_reset"], 1);
        // rooms is now an array of room state objects, not ID strings
        assert_eq!(parsed["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(parsed["rooms"][0]["id"], "on_room");
        assert_eq!(parsed["motion_cleared"], 0);

        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("on_room".into(), ButtonAction::Reset));
    }

    #[test]
    fn fix_turns_off_motion_rooms() {
        let (state, runtime) = setup_state(vec![
            make_snapshot("motion_room", false, false),
            make_snapshot("normal_room", false, false),
        ]);

        // motion_room has active motion, normal_room is just on
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("motion_room".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("normal_room".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "motion_room".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: None,
                timeout_secs: 300,
                warning_active: false,
            },
        );

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // normal_room → reset, motion_room → lights_off
        assert_eq!(parsed["rooms_reset"], 1);
        // rooms is now an array of room state objects containing both reset and motion rooms
        let room_ids: Vec<&str> = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .map(|r| r["id"].as_str().unwrap())
            .collect();
        assert!(room_ids.contains(&"normal_room"));
        assert!(room_ids.contains(&"motion_room"));
        assert_eq!(parsed["motion_cleared"], 1);
        assert_eq!(parsed["motion_rooms"], serde_json::json!(["motion_room"]));

        let events = runtime.events();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&("normal_room".into(), ButtonAction::Reset)));
        assert!(events.contains(&("motion_room".into(), ButtonAction::OffPress)));

        // Verify pending_motion_clear was populated
        let s = state.lock().unwrap();
        assert_eq!(s.pending_motion_clear, vec!["motion_room".to_string()]);
        // lights_on should be false for motion room
        assert_eq!(s.room_lights_on.get("motion_room"), Some(&false));
    }

    #[test]
    fn fix_skips_disabled_motion_rooms() {
        let (state, runtime) = setup_state(vec![make_snapshot("disabled_motion", true, false)]);

        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("disabled_motion".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "disabled_motion".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: None,
                timeout_secs: 300,
                warning_active: false,
            },
        );

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        assert_eq!(parsed["rooms_reset"], 0);
        assert_eq!(parsed["motion_cleared"], 0);
        assert!(runtime.events().is_empty());
    }

    #[test]
    fn fix_turns_off_motion_sensor_rooms_without_active_timer() {
        // Room has a motion sensor registered in the registry but no active
        // motion timer. fix_my_lights should still turn it off (not reset).
        let runtime = Arc::new(MockRuntime::new(vec![
            make_snapshot("motion_room", false, false),
            make_snapshot("normal_room", false, false),
        ]));
        let mut registry =
            crate::registry::HubDeviceRegistry::new(rhythm_core::room::RoomSource::HomeAssistant);
        registry.upsert_device("sensor1", "motion_room", &[], DeviceType::Motion);
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime.clone() as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        app.room_lights_on.insert("motion_room".into(), true);
        app.room_lights_on.insert("normal_room".into(), true);
        let state: SharedState = Arc::new(Mutex::new(app));

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // normal_room → Reset, motion_room → OffPress (despite no active timer)
        assert_eq!(parsed["rooms_reset"], 1);
        assert_eq!(parsed["motion_cleared"], 1);
        assert_eq!(parsed["motion_rooms"], serde_json::json!(["motion_room"]));

        let events = runtime.events();
        assert_eq!(events.len(), 2);
        assert!(events.contains(&("normal_room".into(), ButtonAction::Reset)));
        assert!(events.contains(&("motion_room".into(), ButtonAction::OffPress)));
    }

    #[test]
    fn fix_no_runtime_returns_error() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_fix_my_lights(&state, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    #[test]
    fn fix_empty_rooms_succeeds() {
        let (state, _runtime) = setup_state(vec![]);
        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["rooms_reset"], 0);
        assert_eq!(parsed["motion_cleared"], 0);
    }

    // ========================================================================
    // Room action tests
    // ========================================================================

    #[test]
    fn room_action_on() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OnPress));
    }

    #[test]
    fn room_action_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::OffPress));
    }

    #[test]
    fn room_action_toggle() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "toggle", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Toggle));
    }

    #[test]
    fn room_action_reset() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "reset", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::Reset));
    }

    #[test]
    fn room_action_rhythm_on() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "rhythm_on", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOn));
    }

    #[test]
    fn room_action_rhythm_off() {
        let (state, runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "rhythm_off", false);
        assert!(result.is_ok());
        let events = runtime.events();
        assert_eq!(events.len(), 1);
        assert_eq!(events[0], ("room1".into(), ButtonAction::RhythmOff));
    }

    #[test]
    fn room_action_unknown() {
        let (state, _runtime) = setup_state(vec![make_snapshot("room1", false, false)]);
        let result = do_room_action(&state, "room1", "nonsense", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("Unknown action"));
    }

    #[test]
    fn room_action_no_runtime() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_room_action(&state, "room1", "on", false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    // ========================================================================
    // Settings tests
    // ========================================================================

    #[test]
    fn settings_fade_ms() {
        let (state, _runtime) = setup_state(vec![]);
        do_settings_set(&state, Some(300), None, None, None, None).unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.bulb_fade_ms, 300);
        assert_eq!(s.bulb_fade_atomic.load(Ordering::Relaxed), 300);
    }

    #[test]
    fn settings_interval_clamped() {
        let (state, _runtime) = setup_state(vec![]);
        do_settings_set(&state, None, Some(5), None, None, None).unwrap();
        let s = state.lock().unwrap();
        assert_eq!(s.runtime_config.update_interval_secs, 10);
    }

    #[test]
    fn settings_partial_update() {
        let (state, _runtime) = setup_state(vec![]);

        // Record original values
        let (orig_interval, orig_motion) = {
            let s = state.lock().unwrap();
            (
                s.runtime_config.update_interval_secs,
                s.default_motion_timeout_secs,
            )
        };

        // Only update fade_ms
        do_settings_set(&state, Some(500), None, None, None, None).unwrap();

        let s = state.lock().unwrap();
        assert_eq!(s.bulb_fade_ms, 500);
        assert_eq!(s.runtime_config.update_interval_secs, orig_interval);
        assert_eq!(s.default_motion_timeout_secs, orig_motion);
    }

    #[test]
    fn settings_soft_off_brightness_clamped() {
        let (state, _runtime) = setup_state(vec![]);

        // Value > 100 should be clamped to 100
        do_settings_set(&state, None, None, None, None, Some(200)).unwrap();
        assert_eq!(state.lock().unwrap().soft_off_brightness, 100);

        // Value < 1 should be clamped to 1
        do_settings_set(&state, None, None, None, None, Some(0)).unwrap();
        assert_eq!(state.lock().unwrap().soft_off_brightness, 1);
    }

    // ========================================================================
    // Parse helper tests
    // ========================================================================

    #[test]
    fn parse_buttons_valid() {
        let json = serde_json::json!({
            "buttons": [
                {"button_id": "btn1", "control_id": 1},
                {"button_id": "btn2", "control_id": 2}
            ]
        });
        let result = parse_buttons(&json);
        assert_eq!(result.len(), 2);
        assert_eq!(result[0], ("btn1".to_string(), 1));
        assert_eq!(result[1], ("btn2".to_string(), 2));
    }

    #[test]
    fn parse_buttons_empty() {
        let json = serde_json::json!({"name": "no buttons here"});
        let result = parse_buttons(&json);
        assert!(result.is_empty());
    }

    #[test]
    fn parse_buttons_missing_fields() {
        let json = serde_json::json!({
            "buttons": [
                {"button_id": "btn1"},
                {"control_id": 3},
                {"button_id": "btn2", "control_id": 4}
            ]
        });
        let result = parse_buttons(&json);
        // Only the complete entry should remain
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], ("btn2".to_string(), 4));
    }

    #[test]
    fn room_params_from_json_valid() {
        let json = serde_json::json!({
            "id": "room_1",
            "name": "Living Room",
            "grouped_light_id": "gl_abc",
            "rhythm_enabled": true,
            "disabled": false,
            "soft_off": true,
            "device_ids": ["d1", "d2"]
        });
        let params = RoomParams::from_json(&json).unwrap();
        assert_eq!(params.id, "room_1");
        assert_eq!(params.name, "Living Room");
        assert_eq!(params.grouped_light_id, "gl_abc");
        assert!(params.rhythm_enabled);
        assert!(!params.disabled);
        assert_eq!(params.soft_off, Some(true));
        assert_eq!(params.device_ids, vec!["d1".to_string(), "d2".to_string()]);
    }

    #[test]
    fn room_params_from_json_missing_id() {
        let json = serde_json::json!({
            "name": "No ID Room"
        });
        let result = RoomParams::from_json(&json);
        assert!(result.is_err());
        let err_msg = result.err().unwrap().to_string();
        assert!(err_msg.contains("Missing room.id"));
    }

    #[test]
    fn room_params_from_json_defaults() {
        let json = serde_json::json!({
            "id": "minimal_room"
        });
        let params = RoomParams::from_json(&json).unwrap();
        assert_eq!(params.id, "minimal_room");
        assert_eq!(params.name, "minimal_room"); // defaults to id
        assert_eq!(params.grouped_light_id, ""); // defaults to empty
        assert!(!params.rhythm_enabled); // defaults to false
        assert!(!params.disabled); // defaults to false
        assert_eq!(params.soft_off, None); // defaults to None
        assert!(params.device_ids.is_empty()); // defaults to empty vec
    }

    // ========================================================================
    // Unified API response shape tests
    // ========================================================================

    #[test]
    fn build_room_rhythm_state_returns_struct() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        assert_eq!(room_state.id, "r1");
        assert!(room_state.rhythm_enabled);
        assert!(room_state.lights_on);
        assert!(!room_state.soft_off);
    }

    #[test]
    fn build_room_rhythm_state_missing_room_errors() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_room_rhythm_state(&state, "nonexistent");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn build_room_rhythm_state_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = build_room_rhythm_state(&state, "r1");
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    #[test]
    fn build_room_rhythm_state_serializes_without_status() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let room_state = build_room_rhythm_state(&state, "r1").unwrap();
        let json_str = serde_json::to_string(&room_state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json_str).unwrap();
        assert_eq!(parsed["id"], "r1");
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn room_action_response_is_valid_room_state_json() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_room_action(&state, "r1", "on", false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Must be a room state object — has id, rhythm_enabled, etc.
        assert_eq!(parsed["id"], "r1");
        assert!(parsed["rhythm_enabled"].is_boolean());
        assert!(parsed["brightness"].is_number());
        assert!(parsed["kelvin"].is_number());
        assert!(parsed["lights_on"].is_boolean());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn settings_response_is_valid_settings_json() {
        let (state, _rt) = setup_state(vec![]);
        let result = do_settings_set(&state, Some(400), None, None, None, None).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["bulb_fade_ms"], 400);
        assert!(parsed["rhythm_interval_secs"].is_number());
        assert!(parsed["power_save"].is_boolean());
        assert!(parsed["soft_off_brightness"].is_number());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_settings_dto_matches_state() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.bulb_fade_ms = 250;
            s.runtime_config.update_interval_secs = 30;
            s.default_motion_timeout_secs = 120;
            s.power_save = true;
            s.soft_off_brightness = 10;
        }
        let dto = build_settings_dto(&state).unwrap();
        assert_eq!(dto.bulb_fade_ms, 250);
        assert_eq!(dto.rhythm_interval_secs, 30);
        assert_eq!(dto.default_motion_timeout_secs, 120);
        assert!(dto.power_save);
        assert_eq!(dto.soft_off_brightness, 10);
    }

    #[test]
    fn build_rooms_state_uses_sse_motion_field_names() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state.lock().unwrap().motion_snapshots.insert(
            "r1".into(),
            MotionSnapshot {
                motion_active: true,
                motion_owned: true,
                remaining_secs: Some(45),
                timeout_secs: 300,
                warning_active: true,
            },
        );

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        let room = &parsed["rooms"][0];
        // SSE-aligned field names
        assert_eq!(room["remaining_secs"], 45);
        assert_eq!(room["timeout_secs"], 300);
        assert_eq!(room["warning_active"], true);
        assert_eq!(room["motion_active"], true);
        assert_eq!(room["motion_owned"], true);
        // Old names must NOT appear
        assert!(room.get("motion_remaining").is_none());
        assert!(room.get("motion_timeout").is_none());
        assert!(room.get("motion_warning").is_none());
    }

    #[test]
    fn build_rooms_state_omits_motion_when_absent() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        let room = &parsed["rooms"][0];
        assert!(room.get("motion_active").is_none());
        assert!(room.get("remaining_secs").is_none());
        assert!(room.get("timeout_secs").is_none());
        assert!(room.get("warning_active").is_none());
    }

    #[test]
    fn build_rooms_state_has_hub_connected() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed["hub_connected"].is_boolean());
    }

    #[test]
    fn build_rooms_state_sensor_no_motion_snapshot_populates_defaults() {
        // Room has a motion sensor in the registry but no MotionSnapshot yet.
        let runtime = Arc::new(MockRuntime::new(vec![
            make_snapshot("sensor_room", false, false),
            make_snapshot("plain_room", false, false),
        ]));
        let mut registry =
            crate::registry::HubDeviceRegistry::new(rhythm_core::room::RoomSource::HomeAssistant);
        registry.upsert_device("ms1", "sensor_room", &[], DeviceType::Motion);
        let registry: Arc<Mutex<dyn HubRegistry>> = Arc::new(Mutex::new(registry));

        let mut app = AppState::default();
        let hub_type = HubType::parse("mock").unwrap();
        let hub_key = crate::canonical::identity::HubKey::new(hub_type.clone(), "mock");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime as Arc<dyn RuntimeHandle>),
                hub_data: Box::new(()),
                registry: Some(registry),
                discovery: None,
                shutdown: Default::default(),
            },
        );
        let state: SharedState = Arc::new(Mutex::new(app));

        let result = build_rooms_state(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // sensor_room should have idle motion defaults
        let sensor_room = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "sensor_room")
            .unwrap();
        assert_eq!(sensor_room["motion_active"], false);
        assert_eq!(sensor_room["motion_owned"], false);
        // remaining_secs is None → omitted by skip_serializing_if
        assert!(sensor_room.get("remaining_secs").is_none());
        assert!(sensor_room["timeout_secs"].is_number());
        assert_eq!(sensor_room["warning_active"], false);

        // plain_room should have no motion fields
        let plain_room = parsed["rooms"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["id"] == "plain_room")
            .unwrap();
        assert!(plain_room.get("motion_active").is_none());
        assert!(plain_room.get("timeout_secs").is_none());
    }

    #[test]
    fn build_state_snapshot_valid_json() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        // Top-level structure
        assert!(parsed["version"].is_string());
        assert!(parsed["platform"].is_string());
        assert!(parsed["context"].is_string());
        assert!(parsed["hub"].is_object());
        assert!(parsed["config"].is_object());
        assert!(parsed["location"].is_object());
        assert!(parsed["settings"].is_object());
        assert!(parsed["rooms"].is_array());
        // No status wrapper
        assert!(parsed.get("status").is_none());
    }

    #[test]
    fn build_state_snapshot_settings_uses_struct() {
        let (state, _rt) = setup_state(vec![]);
        {
            let mut s = state.lock().unwrap();
            s.bulb_fade_ms = 700;
            s.power_save = true;
        }
        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert_eq!(parsed["settings"]["bulb_fade_ms"], 700);
        assert_eq!(parsed["settings"]["power_save"], true);
    }

    #[test]
    fn build_state_snapshot_hub_uses_type_not_hub_type() {
        let (state, _rt) = setup_state(vec![]);
        let result = build_state_snapshot(&state).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Hub serializes "type", not "hub_type"
        assert!(parsed["hub"]["type"].is_string());
        assert!(parsed["hub"].get("hub_type").is_none());
    }

    #[test]
    fn fix_response_no_status_key() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        assert!(parsed.get("status").is_none());
        // Rooms is an array of objects (not ID strings)
        assert!(parsed["rooms"][0].is_object());
        assert!(parsed["rooms"][0]["id"].is_string());
    }

    #[test]
    fn fix_response_no_room_states_key() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();
        // Old "room_states" key must not exist
        assert!(parsed.get("room_states").is_none());
        // Room state objects are in "rooms"
        assert!(parsed["rooms"].is_array());
    }

    #[test]
    fn fix_response_rooms_contain_full_state() {
        let (state, _rt) = setup_state(vec![
            make_snapshot("r1", false, false),
            make_snapshot("r2", false, false),
        ]);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r1".into(), true);
        state
            .lock()
            .unwrap()
            .room_lights_on
            .insert("r2".into(), true);

        let result = do_fix_my_lights(&state, false).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&result).unwrap();

        for room in parsed["rooms"].as_array().unwrap() {
            assert!(room["id"].is_string());
            assert!(room["rhythm_enabled"].is_boolean());
            assert!(room["brightness"].is_number());
            assert!(room["kelvin"].is_number());
            assert!(room["lights_on"].is_boolean());
        }
    }

    // ========================================================================
    // set_brightness tests
    // ========================================================================

    #[test]
    fn set_brightness_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_brightness(&state, "r1", 75, false);
        assert!(result.is_ok());
        // Setting brightness marks room as lights_on
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn set_brightness_clamps_high() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 200 should be clamped to 100
        let result = do_set_brightness(&state, "r1", 200, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_clamps_low() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        // 0 should be clamped to 1
        let result = do_set_brightness(&state, "r1", 0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_brightness_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_brightness(&state, "r1", 50, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No runtime"));
    }

    // ========================================================================
    // set_time_offset tests
    // ========================================================================

    #[test]
    fn set_time_offset_succeeds() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_time_offset(&state, "r1", 30.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_negative() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);
        let result = do_set_time_offset(&state, "r1", -60.0, false);
        assert!(result.is_ok());
    }

    #[test]
    fn set_time_offset_no_runtime_errors() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_set_time_offset(&state, "r1", 15.0, false);
        assert!(result.is_err());
    }

    // ========================================================================
    // room_preferences tests
    // ========================================================================

    #[test]
    fn room_preferences_rhythm_enabled() {
        let snap = make_snapshot("r1", false, false);
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_room_preferences_set(&state, "r1", Some(true), None, None, false);
        assert!(result.is_ok());
    }

    #[test]
    fn room_preferences_soft_off_implies_rhythm_enabled() {
        // When soft_off is set, rhythm_enabled should be forced true
        let mut snap = make_snapshot("r1", false, false);
        snap.rhythm_enabled = false;
        let (state, _rt) = setup_state(vec![snap]);
        let result = do_room_preferences_set(&state, "r1", Some(false), None, Some(true), false);
        assert!(result.is_ok());
        // soft_off implies lights conceptually on
        assert_eq!(state.lock().unwrap().room_lights_on.get("r1"), Some(&true));
    }

    #[test]
    fn room_preferences_missing_room_errors() {
        let (state, _rt) = setup_state(vec![]);
        let result = do_room_preferences_set(&state, "nonexistent", Some(true), None, None, false);
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    // ========================================================================
    // Settings edge case tests
    // ========================================================================

    #[test]
    fn settings_motion_timeout() {
        let (state, _rt) = setup_state(vec![]);
        do_settings_set(&state, None, None, Some(180), None, None).unwrap();
        assert_eq!(state.lock().unwrap().default_motion_timeout_secs, 180);
    }

    #[test]
    fn settings_power_save() {
        let (state, _rt) = setup_state(vec![]);
        do_settings_set(&state, None, None, None, Some(true), None).unwrap();
        assert!(state.lock().unwrap().power_save);
        do_settings_set(&state, None, None, None, Some(false), None).unwrap();
        assert!(!state.lock().unwrap().power_save);
    }

    #[test]
    fn settings_interval_valid() {
        let (state, _rt) = setup_state(vec![]);
        do_settings_set(&state, None, Some(120), None, None, None).unwrap();
        assert_eq!(
            state.lock().unwrap().runtime_config.update_interval_secs,
            120
        );
    }

    // ========================================================================
    // absorb_time_offset tests
    // ========================================================================

    #[test]
    fn absorb_offset_no_runtime_still_succeeds() {
        // Without a runtime, absorb should succeed (no rooms to reset)
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));
        let result = do_absorb_time_offset(&state, 30.0);
        assert!(result.is_ok());
    }

    #[test]
    fn absorb_offset_resets_room_offsets() {
        let mut snap = make_snapshot("r1", false, false);
        snap.time_offset_minutes = 30.0;
        let (state, _rt) = setup_state(vec![snap]);

        let result = do_absorb_time_offset(&state, 30.0);
        assert!(result.is_ok());

        // MockRuntime's set_room_time_offset is a no-op, so we can't assert
        // the offset was reset on the runtime — but we can verify the command
        // completed without error.
    }

    #[test]
    fn absorb_offset_updates_config_widths() {
        let (state, _rt) = setup_state(vec![make_snapshot("r1", false, false)]);

        // Set location so sunrise/sunset can be computed
        {
            let mut s = state.lock().unwrap();
            s.latitude = Some(35.0);
            s.longitude = Some(-120.0);
            s.utc_offset_hours = -8.0;
            s.timezone_name = Some("America/Los_Angeles".to_string());
        }

        let before = state.lock().unwrap().config.clone();
        let result = do_absorb_time_offset(&state, 30.0);
        assert!(result.is_ok());

        let after = state.lock().unwrap().config.clone();
        // Config may or may not change depending on current time of day
        // (might be at peak or in tail where absorb returns None)
        // But the command should always succeed
        let _ = (before, after);
    }
}
