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

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Result;
    use serde_json::{json, Value};
    use std::sync::{Arc, Mutex};

    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::hub::HubType;
    use rhythm_os::state::AppState;
    use rhythm_os::storage::FileStorage;

    use crate::provider::ha_credentials;
    use crate::transport::EntityState;

    struct FakeTransport {
        config: Result<Value, String>,
    }

    impl FakeTransport {
        fn ok(config: Value) -> Self {
            Self { config: Ok(config) }
        }

        fn err(message: &str) -> Self {
            Self {
                config: Err(message.to_string()),
            }
        }
    }

    impl HaTransport for FakeTransport {
        fn call_service(&self, _domain: &str, _service: &str, _data: &Value) -> Result<()> {
            Ok(())
        }

        fn get_states(&self) -> Result<Vec<EntityState>> {
            Ok(Vec::new())
        }

        fn get_state(&self, entity_id: &str) -> Result<EntityState> {
            Ok(EntityState {
                entity_id: entity_id.to_string(),
                state: "off".to_string(),
                attributes: Value::Null,
            })
        }

        fn test_connection(&self) -> Result<bool> {
            Ok(true)
        }

        fn get_config(&self) -> Result<Value> {
            self.config
                .clone()
                .map_err(|message| anyhow::anyhow!(message))
        }
    }

    fn shared_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn temp_storage(prefix: &str) -> FileStorage {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "rhythm-ha-post-connect-{}-{}-{}",
            prefix,
            std::process::id(),
            id
        ));
        if path.exists() {
            std::fs::remove_dir_all(&path).unwrap();
        }
        FileStorage::new(path.to_str().unwrap()).unwrap()
    }

    #[test]
    fn fetch_ha_config_imports_new_location_timezone_and_persists_it() {
        let state = shared_state();
        state.lock().unwrap().storage = Some(Box::new(temp_storage("import")));
        let transport = FakeTransport::ok(json!({
            "latitude": 35.22,
            "longitude": -80.84,
            "time_zone": "America/New_York"
        }));

        fetch_ha_config(&state, &transport);

        let guard = state.lock().unwrap();
        assert_eq!(guard.latitude, Some(35.22));
        assert_eq!(guard.longitude, Some(-80.84));
        assert_eq!(guard.timezone_name.as_deref(), Some("America/New_York"));
        assert_ne!(guard.utc_offset_hours, 0.0);
        assert!(
            guard
                .storage
                .as_ref()
                .unwrap()
                .load_location()
                .unwrap()
                .timezone_name
                .as_deref()
                == Some("America/New_York")
        );
    }

    #[test]
    fn fetch_ha_config_refreshes_timezone_without_overwriting_existing_location() {
        let state = shared_state();
        {
            let mut guard = state.lock().unwrap();
            guard.latitude = Some(12.0);
            guard.longitude = Some(34.0);
            guard.utc_offset_hours = 2.0;
        }
        let transport = FakeTransport::ok(json!({
            "latitude": 35.22,
            "longitude": -80.84,
            "time_zone": "Europe/Berlin"
        }));

        fetch_ha_config(&state, &transport);

        let guard = state.lock().unwrap();
        assert_eq!(guard.latitude, Some(12.0));
        assert_eq!(guard.longitude, Some(34.0));
        assert_eq!(guard.timezone_name.as_deref(), Some("Europe/Berlin"));
    }

    #[test]
    fn fetch_ha_config_returns_on_transport_error_or_incomplete_location() {
        let state = shared_state();
        fetch_ha_config(&state, &FakeTransport::err("offline"));
        assert!(state.lock().unwrap().latitude.is_none());

        fetch_ha_config(
            &state,
            &FakeTransport::ok(json!({
                "latitude": 35.22,
                "time_zone": "America/New_York"
            })),
        );
        assert!(state.lock().unwrap().latitude.is_none());
    }

    #[test]
    fn config_from_state_picks_stored_homeassistant_credentials() {
        let state = shared_state();
        let hub_key = HubKey::new(HubType::new("homeassistant"), "https://ha.local:9443");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(hub_key, ha_credentials("https://ha.local:9443", "token-1"));

        let config = config_from_state(&state).unwrap();

        assert_eq!(config.host, "ha.local");
        assert_eq!(config.port, 9443);
        assert_eq!(config.token, "token-1");
        assert!(config.use_ssl);
    }

    #[test]
    fn config_from_state_and_cache_population_return_when_credentials_are_missing() {
        let state = shared_state();

        assert!(config_from_state(&state).is_none());
        populate_device_area_cache(&state);
        assert!(state.lock().unwrap().hubs.is_empty());
    }
}
