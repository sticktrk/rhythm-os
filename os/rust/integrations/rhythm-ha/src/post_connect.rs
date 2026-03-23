//! Post-connect hooks for HA integration.
//!
//! Called after connecting to HA and syncing rooms. Imports location/timezone
//! from HA config and populates the device→area cache for button discovery.

use log::{info, warn};

use rhythm_os::state::SharedState;

use crate::hub_state::HaHubData;
use crate::transport::HaTransport;

/// Build an `HaConnectionConfig` from stored HA credentials in state.
fn config_from_state(state: &SharedState) -> Option<crate::transport::HaConnectionConfig> {
    let s = state.lock().ok()?;
    let ha_creds = s.hub_credentials.values().find(|c| {
        c.hub_type
            .as_ref()
            .is_some_and(|t| t.as_str() == "homeassistant")
    })?;
    crate::provider::config_from_credentials(&ha_creds.address, ha_creds).ok()
}

/// Fetch location and timezone from HA and apply to state.
///
/// Uses the provided transport to call `GET /api/config`. When location is
/// already set, only the timezone/UTC offset is refreshed (handles DST
/// transitions across restarts). When not set, lat/lon are imported.
pub fn fetch_ha_config(state: &SharedState, transport: &dyn HaTransport) {
    let ha_config = match transport.get_config() {
        Ok(v) => v,
        Err(e) => {
            warn!(target: "sys", "Failed to fetch HA config: {}", e);
            return;
        }
    };

    let tz_name = ha_config.get("time_zone").and_then(|v| v.as_str());

    let location_already_set = state.lock().map(|s| s.latitude.is_some()).unwrap_or(false);

    // Build StoredLocation from HA config
    let loc = if !location_already_set {
        let lat = ha_config
            .get("latitude")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);
        let lon = ha_config
            .get("longitude")
            .and_then(|v| v.as_f64())
            .map(|v| v as f32);
        if lat.is_none() || lon.is_none() {
            return;
        }
        rhythm_os::storage::StoredLocation {
            latitude: lat,
            longitude: lon,
            utc_offset_hours: 0.0, // will be recomputed by apply_to_state
            timezone_name: tz_name.map(String::from),
        }
    } else if tz_name.is_some() {
        // Location already persisted — just refresh timezone (DST may have changed)
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        rhythm_os::storage::StoredLocation {
            latitude: s.latitude,
            longitude: s.longitude,
            utc_offset_hours: s.utc_offset_hours,
            timezone_name: tz_name.map(String::from),
        }
    } else {
        return;
    };

    // Apply to state (recomputes UTC offset + solar noon from timezone)
    let mut guard = match state.lock() {
        Ok(s) => s,
        Err(_) => return,
    };
    let s = &mut *guard;
    loc.apply_to_state(
        &mut s.latitude,
        &mut s.longitude,
        &mut s.utc_offset_hours,
        &mut s.runtime_config,
        &mut s.timezone_name,
    );
    if let Some(ref storage) = s.storage {
        if let Err(e) = storage.save_location(&loc) {
            warn!(target: "sys", "Failed to save location: {}", e);
        }
    }

    info!(target: "sys", "Imported HA config (tz={:?}, location_new={})",
        tz_name, !location_already_set);
}

/// Populate the device→area cache for on-demand button/motion discovery.
///
/// Queries HA for all devices and their area assignments, then writes
/// the mapping into `HaHubData.device_area_cache`. This enables
/// `hue_event` button events to auto-register unknown buttons by
/// looking up which area the device belongs to.
pub fn populate_device_area_cache(state: &SharedState) {
    let config = match config_from_state(state) {
        Some(c) => c,
        None => {
            warn!(target: "sys", "Cannot build HA config for cache population");
            return;
        }
    };

    let result = match crate::area_sync::discover_areas_with_device_map(&config) {
        Ok(r) => r,
        Err(e) => {
            warn!(target: "sys", "Device area discovery failed: {}", e);
            return;
        }
    };

    if result.device_area_map.is_empty() {
        return;
    }

    let cache_arc = {
        let s = match state.lock() {
            Ok(s) => s,
            Err(_) => return,
        };
        let ha_data = s.hubs.values().find_map(|h| h.data::<HaHubData>());
        match ha_data {
            Some(d) => d.device_area_cache.clone(),
            None => return,
        }
    };

    let mut cache = match cache_arc.lock() {
        Ok(c) => c,
        Err(_) => return,
    };
    let device_count = result.device_area_map.len();
    let sensor_count = result.binary_sensor_areas.len();
    *cache = result.device_area_map;

    // Also add entity_id → area_id for all binary_sensor entities.
    // The event translator filters for motion/occupancy device_class at event time,
    // so we cache broadly here (some integrations don't set original_device_class
    // in the entity registry).
    cache.extend(result.binary_sensor_areas);

    // Also add entity_id → area_id for all event.* entities (button events).
    // Enables on-demand button discovery from HA event entities (universal
    // button support across Zigbee2MQTT, deCONZ, Matter, native Hue, etc.).
    let event_count = result.event_entity_areas.len();
    cache.extend(result.event_entity_areas);

    info!(target: "sys",
        "Populated event cache with {} device + {} binary_sensor + {} event entries",
        device_count, sensor_count, event_count);
}
