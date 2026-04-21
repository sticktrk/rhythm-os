//! Abstract persistence interface for Rhythm OS.
//!
//! Platform crates implement this trait to provide concrete storage
//! (e.g., filesystem on Linux, SQLite on Raspberry Pi, or future custom backends).

use anyhow::Context;
use anyhow::Result;
use log::{debug, info, warn};
use rhythm_core::room::RoomManager;
use rhythm_core::RuntimeConfig;
use rhythm_core::{
    LightProfileConfig, ModeChangeCause, ModeConfig, ModeTransitionConfig, RhythmMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canonical::identity::HubKey;
use crate::hub::HubCredentials;

/// Abstract persistence interface.
///
/// Each method loads/saves a single domain object. Implementations should
/// be safe to call from any thread (`Send + Sync`).
pub trait Storage: Send + Sync {
    fn load_rooms(&self) -> Result<RoomManager>;
    fn save_rooms(&self, rooms: &RoomManager) -> Result<()>;
    fn load_light_profiles(&self) -> Result<StoredLightProfiles>;
    fn save_light_profiles(&self, config: &StoredLightProfiles) -> Result<()>;
    fn load_location(&self) -> Result<StoredLocation>;
    fn save_location(&self, loc: &StoredLocation) -> Result<()>;
    fn load_settings(&self) -> Result<StoredSettings>;
    fn save_settings(&self, settings: &StoredSettings) -> Result<()>;
    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>>;
    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()>;
    fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>>;
    fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()>;

    /// Load the canonical device registry. Default: returns None (not persisted).
    fn load_canonical_registry(&self) -> Result<Option<Value>> {
        Ok(None)
    }

    /// Save the canonical device registry. Default: no-op.
    fn save_canonical_registry(&self, _data: &Value) -> Result<()> {
        Ok(())
    }

    /// Load the room topology store. Default: returns None (not persisted).
    fn load_topology(&self) -> Result<Option<Value>> {
        Ok(None)
    }

    /// Save the room topology store. Default: no-op.
    fn save_topology(&self, _data: &Value) -> Result<()> {
        Ok(())
    }

    /// Load stored appliance Wi-Fi credentials used for accessory commissioning.
    fn load_commissioning_wifi_credentials(
        &self,
    ) -> Result<Option<crate::provisioning::WifiCredentials>> {
        Ok(None)
    }

    /// Persist appliance Wi-Fi credentials used for accessory commissioning.
    fn save_commissioning_wifi_credentials(
        &self,
        _creds: &crate::provisioning::WifiCredentials,
    ) -> Result<()> {
        Ok(())
    }

    /// Clear persisted appliance Wi-Fi credentials.
    fn clear_commissioning_wifi_credentials(&self) -> Result<()> {
        Ok(())
    }

    /// Clear all persisted state that should not survive a full factory reset.
    ///
    /// Active desktop/server platforms use this to remove stale keyed hub
    /// registries and any other persisted artifacts that would otherwise be
    /// resurrected after a reset.
    fn clear_factory_reset_state(&self) -> Result<()> {
        self.clear_commissioning_wifi_credentials()
    }
}

/// Stored light profile configurations for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLightProfiles {
    pub solar_noon_hour: f32,
    pub profiles: Vec<LightProfileConfig>,
}

impl StoredLightProfiles {
    pub fn from_state(
        profiles: &std::collections::BTreeMap<String, LightProfileConfig>,
        runtime_config: &RuntimeConfig,
    ) -> Self {
        Self {
            solar_noon_hour: runtime_config.solar_noon_hour,
            profiles: profiles.values().cloned().collect(),
        }
    }

    pub fn apply_to_state(
        &self,
        profiles: &mut std::collections::BTreeMap<String, LightProfileConfig>,
        runtime_config: &mut RuntimeConfig,
    ) {
        *profiles = crate::factory_default_config::factory_default_light_profile_config_map();
        for profile in &self.profiles {
            let mut normalized = profile.clone();
            rhythm_core::normalize_builtin_state_profile_config(&mut normalized);
            profiles.insert(normalized.id.clone(), normalized);
        }
        runtime_config.solar_noon_hour = self.solar_noon_hour;
    }
}

/// Location for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredLocation {
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub utc_offset_hours: f32,
    /// IANA timezone name (e.g. "America/New_York"). Used to recompute
    /// `utc_offset_hours` on startup so DST transitions are handled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timezone_name: Option<String>,
}

impl StoredLocation {
    /// Apply location to state fields, recomputing UTC offset and solar noon
    /// from the timezone name if available (handles DST transitions).
    pub fn apply_to_state(
        &self,
        latitude: &mut Option<f32>,
        longitude: &mut Option<f32>,
        utc_offset_hours: &mut f32,
        runtime_config: &mut RuntimeConfig,
        timezone_name: &mut Option<String>,
    ) {
        *latitude = self.latitude;
        *longitude = self.longitude;
        *timezone_name = self.timezone_name.clone();

        if let Some(ref tz_name) = self.timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            let (year, month, day, hour) =
                tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
            *utc_offset_hours = tz.utc_offset(year, month, day, hour);
            if let Some(lon) = self.longitude {
                runtime_config.solar_noon_hour =
                    rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
            }
            info!(target: "sys", "Loaded location: lat={:?}, lon={:?}, tz={}, utc_offset={}, solar_noon={:.2}",
                latitude, longitude, tz_name, utc_offset_hours, runtime_config.solar_noon_hour);
        } else {
            *utc_offset_hours = self.utc_offset_hours;
            info!(target: "sys", "Loaded location: lat={:?}, lon={:?}, utc_offset={}",
                latitude, longitude, utc_offset_hours);
        }
    }
}

/// Global settings for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSettings {
    pub power_save: bool,
    pub active_mode: RhythmMode,
    #[serde(default)]
    pub last_active_mode_cause: ModeChangeCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_mode_transition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_active_mode_change_utc_ms: Option<i64>,
    #[serde(default)]
    pub modes: Vec<ModeConfig>,
    #[serde(default)]
    pub mode_transitions: Vec<ModeTransitionConfig>,
}

// ---------------------------------------------------------------------------
// FileStorage — filesystem backend (desktop targets only)
// ---------------------------------------------------------------------------

/// Filesystem storage backend.
///
/// Implements [`Storage`] using JSON files with atomic writes (write to .tmp,
/// then rename). Used by rhythm-addon and rhythm-server.
pub struct FileStorage {
    dir: std::path::PathBuf,
}

impl FileStorage {
    /// Create a new `FileStorage` rooted at `dir`.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(dir: &str) -> Result<Self> {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create data dir: {}", dir.display()))?;
        Ok(Self { dir })
    }

    fn file_path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }

    /// Atomic write: write to .tmp then rename.
    fn write_atomic(&self, name: &str, data: &[u8]) -> Result<()> {
        let path = self.file_path(name);
        let tmp = self.file_path(&format!("{}.tmp", name));

        std::fs::write(&tmp, data).with_context(|| format!("Failed to write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("Failed to rename {} -> {}", tmp.display(), path.display()))?;

        Ok(())
    }

    fn read_json<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<T> {
        let path = self.file_path(name);
        let data = std::fs::read_to_string(&path)
            .with_context(|| format!("Failed to read {}", path.display()))?;
        serde_json::from_str(&data).with_context(|| format!("Failed to parse {}", path.display()))
    }

    fn remove_if_exists(&self, name: &str) -> Result<()> {
        let path = self.file_path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(anyhow::anyhow!(e)).with_context(|| format!("removing {}", path.display()))
            }
        }
    }
}

impl Storage for FileStorage {
    fn load_rooms(&self) -> Result<rhythm_core::room::RoomManager> {
        self.read_json("rooms.json")
    }

    fn save_rooms(&self, rooms: &rhythm_core::room::RoomManager) -> Result<()> {
        let data = serde_json::to_string_pretty(rooms)?;
        self.write_atomic("rooms.json", data.as_bytes())
    }

    fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
        self.read_json("light_profiles.json")
    }

    fn save_light_profiles(&self, config: &StoredLightProfiles) -> Result<()> {
        let data = serde_json::to_string_pretty(config)?;
        self.write_atomic("light_profiles.json", data.as_bytes())
    }

    fn load_location(&self) -> Result<StoredLocation> {
        self.read_json("location.json")
    }

    fn save_location(&self, loc: &StoredLocation) -> Result<()> {
        let data = serde_json::to_string_pretty(loc)?;
        self.write_atomic("location.json", data.as_bytes())
    }

    fn load_settings(&self) -> Result<StoredSettings> {
        self.read_json("settings.json")
    }

    fn save_settings(&self, settings: &StoredSettings) -> Result<()> {
        let data = serde_json::to_string_pretty(settings)?;
        self.write_atomic("settings.json", data.as_bytes())
    }

    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
        let path = self.file_path("hub_credentials.json");
        if !path.exists() {
            return Ok(Vec::new());
        }
        let creds: Vec<HubCredentials> = self.read_json("hub_credentials.json")?;
        Ok(creds.into_iter().filter(|c| c.is_configured()).collect())
    }

    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
        let data = serde_json::to_string_pretty(creds)?;
        self.write_atomic("hub_credentials.json", data.as_bytes())
    }

    fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>> {
        let filename = format!("hub_registry_{}.json", sanitize_hub_key(key));
        let path = self.file_path(&filename);
        match self.read_json::<Value>(&filename) {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load hub registry {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No keyed hub registry found at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()> {
        let filename = format!("hub_registry_{}.json", sanitize_hub_key(key));
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic(&filename, json.as_bytes())
    }

    fn load_canonical_registry(&self) -> Result<Option<serde_json::Value>> {
        let path = self.file_path("canonical_registry.json");
        match self.read_json::<serde_json::Value>("canonical_registry.json") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load canonical registry {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted canonical registry at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_canonical_registry(&self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic("canonical_registry.json", json.as_bytes())
    }

    fn load_topology(&self) -> Result<Option<serde_json::Value>> {
        let path = self.file_path("topology.json");
        match self.read_json::<serde_json::Value>("topology.json") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load topology {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(target: "sys", "No persisted topology at {}", path.display());
                }
                Ok(None)
            }
        }
    }

    fn save_topology(&self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic("topology.json", json.as_bytes())
    }

    fn load_commissioning_wifi_credentials(
        &self,
    ) -> Result<Option<crate::provisioning::WifiCredentials>> {
        let path = self.file_path("commissioning_wifi.json");
        match self.read_json::<crate::provisioning::WifiCredentials>("commissioning_wifi.json") {
            Ok(creds) => Ok(Some(creds)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load commissioning Wi-Fi config {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted commissioning Wi-Fi config at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_commissioning_wifi_credentials(
        &self,
        creds: &crate::provisioning::WifiCredentials,
    ) -> Result<()> {
        let json = serde_json::to_string_pretty(creds)?;
        self.write_atomic("commissioning_wifi.json", json.as_bytes())
    }

    fn clear_commissioning_wifi_credentials(&self) -> Result<()> {
        let path = self.file_path("commissioning_wifi.json");
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => {
                Err(anyhow::anyhow!(e)).with_context(|| format!("removing {}", path.display()))
            }
        }
    }

    fn clear_factory_reset_state(&self) -> Result<()> {
        for name in [
            "rooms.json",
            "light_profiles.json",
            "location.json",
            "settings.json",
            "hub_credentials.json",
            "hub_registry.json",
            "canonical_registry.json",
            "topology.json",
            "commissioning_wifi.json",
        ] {
            self.remove_if_exists(name)?;
        }

        for entry in std::fs::read_dir(&self.dir)
            .with_context(|| format!("reading {}", self.dir.display()))?
        {
            let entry = entry?;
            let Some(name) = entry.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if name.starts_with("hub_registry_") && name.ends_with(".json") {
                match std::fs::remove_file(entry.path()) {
                    Ok(()) => {}
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                    Err(e) => {
                        return Err(anyhow::anyhow!(e))
                            .with_context(|| format!("removing {}", entry.path().display()));
                    }
                }
            }
        }

        for dir in ["matter"] {
            let path = self.dir.join(dir);
            match std::fs::remove_dir_all(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => {
                    return Err(anyhow::anyhow!(e))
                        .with_context(|| format!("removing {}", path.display()));
                }
            }
        }

        Ok(())
    }
}

/// Sanitize a HubKey into a filesystem-safe string for per-hub filenames.
fn sanitize_hub_key(key: &HubKey) -> String {
    format!(
        "{}_{}",
        key.hub_type.as_str(),
        key.address.replace([':', '/', '.'], "_")
    )
}

// ---------------------------------------------------------------------------
// load_persisted_state — shared startup helper
// ---------------------------------------------------------------------------

/// Load all persisted state from storage into AppState.
///
/// Loads config, location, settings, and hub credentials from the
/// configured [`Storage`] backend. Safe to call on any platform.
pub fn load_persisted_state(s: &mut crate::state::AppState) {
    if s.storage.is_none() {
        return;
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_light_profiles() {
            Ok(configs) => {
                configs.apply_to_state(&mut s.light_profile_configs, &mut s.runtime_config);
                s.sync_active_mode_runtime_overrides();
                info!(
                    target: "sys",
                    "Loaded light profiles: {} profiles, solar_noon={}",
                    s.light_profile_configs.len(),
                    s.runtime_config.solar_noon_hour
                );
            }
            Err(e) => {
                debug!(target: "sys", "No persisted light profiles loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_location() {
            Ok(loc) => {
                loc.apply_to_state(
                    &mut s.latitude,
                    &mut s.longitude,
                    &mut s.utc_offset_hours,
                    &mut s.runtime_config,
                    &mut s.timezone_name,
                );
            }
            Err(e) => {
                debug!(target: "sys", "No persisted location loaded: {}", e);
            }
        }
    }

    let loaded_settings = s.storage.as_ref().map(|storage| storage.load_settings());
    if let Some(result) = loaded_settings {
        match result {
            Ok(settings) => {
                let loaded_modes = settings.modes.clone();
                let loaded_transitions = settings.mode_transitions.clone();
                let missing_mode_change_timestamp =
                    settings.last_active_mode_change_utc_ms.is_none();
                let last_active_mode_change_utc_ms = settings
                    .last_active_mode_change_utc_ms
                    .or_else(|| Some(chrono::Utc::now().timestamp_millis()));
                s.power_save = settings.power_save;
                s.active_mode = settings.active_mode;
                s.last_active_mode_cause = settings.last_active_mode_cause;
                s.last_active_mode_transition_id = settings.last_active_mode_transition_id.clone();
                s.last_active_mode_change_utc_ms = last_active_mode_change_utc_ms;
                s.set_mode_configs(settings.modes);
                s.set_mode_transition_configs(settings.mode_transitions);
                s.sync_active_mode_runtime_overrides();
                let normalized_modes = s.mode_configs();
                let normalized_transitions = s.mode_transition_configs();
                let transitions_changed = normalized_transitions != loaded_transitions;
                if normalized_modes != loaded_modes
                    || missing_mode_change_timestamp
                    || transitions_changed
                {
                    info!(target: "sys", "Normalized persisted mode settings during load");
                    if let Some(storage) = s.storage.as_ref() {
                        if let Err(e) = storage.save_settings(&StoredSettings {
                            power_save: s.power_save,
                            active_mode: s.active_mode,
                            last_active_mode_cause: s.last_active_mode_cause,
                            last_active_mode_transition_id: s
                                .last_active_mode_transition_id
                                .clone(),
                            last_active_mode_change_utc_ms,
                            modes: normalized_modes.clone(),
                            mode_transitions: normalized_transitions,
                        }) {
                            warn!(
                                target: "sys",
                                "Failed to persist normalized mode settings: {}",
                                e
                            );
                        }
                    }
                }
                info!(
                    target: "sys",
                    "Loaded settings: active_mode={:?}, modes={}, transitions={}",
                    s.active_mode,
                    s.mode_configs.len(),
                    s.mode_transition_configs.len()
                );
            }
            Err(e) => {
                debug!(target: "sys", "No persisted settings loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_all_hub_credentials() {
            Ok(all_creds) => {
                for creds in all_creds {
                    info!(target: "sys", "Loaded hub credentials: type={:?}, addr={}", creds.hub_type, creds.address);
                    if let Some(key) = creds.hub_key() {
                        s.hub_credentials.insert(key, creds);
                    }
                }
            }
            Err(e) => {
                warn!(target: "sys", "Failed to load hub credentials: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        if let Ok(Some(value)) = storage.load_canonical_registry() {
            match serde_json::from_value::<crate::canonical::registry::CanonicalRegistry>(value) {
                Ok(mut registry) => {
                    registry.rebuild_indices();
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    registry.backfill_unassigned_triage(now);
                    let count = registry.device_count();
                    s.canonical_registry = registry;
                    info!(target: "sys", "Loaded canonical registry: {} devices", count);
                }
                Err(e) => {
                    warn!(target: "sys", "Failed to parse canonical registry (will start fresh): {}", e);
                }
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        if let Ok(Some(value)) = storage.load_topology() {
            match serde_json::from_value::<crate::topology::RoomTopologyStore>(value) {
                Ok(mut topology) => {
                    topology.rebuild_indices();
                    let count = topology.room_count();
                    s.topology = topology;
                    info!(target: "sys", "Loaded topology: {} rooms", count);
                }
                Err(e) => {
                    warn!(target: "sys", "Failed to parse topology (will start fresh): {}", e);
                }
            }
        }
    }

    info!(target: "sys", "Persisted state loaded");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::factory_default_config::factory_default_light_profile_config_map;
    use crate::hub::HubCredentials;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct TestStorage {
        light_profiles: Option<StoredLightProfiles>,
        location: Option<StoredLocation>,
        settings: Option<StoredSettings>,
        saved_settings: Arc<Mutex<Vec<StoredSettings>>>,
        canonical_registry: Option<Value>,
    }

    impl Storage for TestStorage {
        fn load_rooms(&self) -> Result<RoomManager> {
            Ok(RoomManager::new())
        }

        fn save_rooms(&self, _rooms: &RoomManager) -> Result<()> {
            Ok(())
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            self.light_profiles
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing light profiles"))
        }

        fn save_light_profiles(&self, _config: &StoredLightProfiles) -> Result<()> {
            Ok(())
        }

        fn load_location(&self) -> Result<StoredLocation> {
            self.location
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing location"))
        }

        fn save_location(&self, _loc: &StoredLocation) -> Result<()> {
            Ok(())
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            self.settings
                .clone()
                .ok_or_else(|| anyhow::anyhow!("missing settings"))
        }

        fn save_settings(&self, _settings: &StoredSettings) -> Result<()> {
            self.saved_settings.lock().unwrap().push(_settings.clone());
            Ok(())
        }

        fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
            Ok(Vec::new())
        }

        fn save_all_hub_credentials(&self, _creds: &[HubCredentials]) -> Result<()> {
            Ok(())
        }

        fn load_hub_registry_for(&self, _key: &HubKey) -> Result<Option<Value>> {
            Ok(None)
        }

        fn save_hub_registry_for(&self, _key: &HubKey, _data: &Value) -> Result<()> {
            Ok(())
        }

        fn load_canonical_registry(&self) -> Result<Option<Value>> {
            Ok(self.canonical_registry.clone())
        }

        fn save_canonical_registry(&self, _data: &Value) -> Result<()> {
            Ok(())
        }
    }

    // ---- StoredLightProfiles tests (no feature gate needed) ----

    #[test]
    fn stored_light_profiles_from_state_roundtrip() {
        let mut profiles = std::collections::BTreeMap::new();
        let mut config = rhythm_core::default_rhythm_profile();
        config.min_brightness = 5;
        config.max_brightness = 95;
        config.motion_timeout_secs = rhythm_core::TimerSetting::Fixed { value: 300 };
        profiles.insert(config.id.clone(), config.clone());
        let runtime_config = rhythm_core::RuntimeConfig::default().with_solar_noon(12.8);

        let stored = StoredLightProfiles::from_state(&profiles, &runtime_config);
        assert_eq!(stored.profiles.len(), 1);
        assert_eq!(stored.profiles[0].min_brightness, 5);
        assert_eq!(stored.profiles[0].max_brightness, 95);
        assert!((stored.solar_noon_hour - 12.8).abs() < 0.01);

        let mut profiles2 = std::collections::BTreeMap::new();
        let mut runtime_config2 = rhythm_core::RuntimeConfig::default();
        stored.apply_to_state(&mut profiles2, &mut runtime_config2);
        let config2 = profiles2.get(rhythm_core::RHYTHM_PROFILE_ID).unwrap();
        assert_eq!(config2.min_brightness, 5);
        assert_eq!(config2.max_brightness, 95);
        assert_eq!(
            config2.motion_timeout_secs,
            rhythm_core::TimerSetting::Fixed { value: 300 }
        );
        assert!(profiles2.contains_key(rhythm_core::SLEEP_PROFILE_ID));
        assert!(profiles2.contains_key(rhythm_core::DAY_IDLE_PROFILE_ID));
        assert!(profiles2.contains_key(rhythm_core::SLEEP_IDLE_PROFILE_ID));
        assert!((runtime_config2.solar_noon_hour - 12.8).abs() < 0.01);
    }

    #[test]
    fn stored_light_profiles_apply_state_seeds_builtins_and_keeps_custom_profiles() {
        let custom = rhythm_core::LightProfileConfig {
            id: "custom".into(),
            name: "Custom".into(),
            ..rhythm_core::default_day_idle_profile()
        };
        let stored = StoredLightProfiles {
            solar_noon_hour: 12.5,
            profiles: vec![rhythm_core::default_rhythm_profile(), custom],
        };

        let mut profiles = std::collections::BTreeMap::new();
        let mut runtime_config = rhythm_core::RuntimeConfig::default();
        stored.apply_to_state(&mut profiles, &mut runtime_config);

        assert!(profiles.contains_key(rhythm_core::RHYTHM_PROFILE_ID));
        assert!(profiles.contains_key(rhythm_core::SLEEP_PROFILE_ID));
        assert!(profiles.contains_key(rhythm_core::DAY_IDLE_PROFILE_ID));
        assert!(profiles.contains_key(rhythm_core::SLEEP_IDLE_PROFILE_ID));
        assert!(profiles.contains_key("custom"));
    }

    #[test]
    fn stored_light_profiles_empty_payload_restores_bundled_profiles() {
        let stored = StoredLightProfiles {
            solar_noon_hour: 11.75,
            profiles: vec![],
        };

        let mut profiles = std::collections::BTreeMap::new();
        profiles.insert(
            "junk".into(),
            rhythm_core::LightProfileConfig {
                id: "junk".into(),
                name: "Junk".into(),
                ..rhythm_core::default_rhythm_profile()
            },
        );
        let mut runtime_config = rhythm_core::RuntimeConfig::default();
        stored.apply_to_state(&mut profiles, &mut runtime_config);

        assert_eq!(profiles, factory_default_light_profile_config_map());
        assert!((runtime_config.solar_noon_hour - 11.75).abs() < 0.01);
    }

    #[test]
    fn stored_light_profiles_normalize_bad_day_idle_constant_defaults() {
        let stored = StoredLightProfiles {
            solar_noon_hour: 12.5,
            profiles: vec![
                rhythm_core::default_rhythm_profile(),
                rhythm_core::default_sleep_profile(),
                rhythm_core::default_sleep_idle_profile(),
                rhythm_core::LightProfileConfig {
                    id: rhythm_core::DAY_IDLE_PROFILE_ID.into(),
                    name: rhythm_core::DAY_IDLE_PROFILE_NAME.into(),
                    curve: rhythm_core::LightCurveShape::Constant {
                        brightness: 15.0,
                        color_temp: 0.0,
                        direct_color: Some(rhythm_core::LightDirectColor {
                            xy: rhythm_core::rgb_to_xy(rhythm_core::Rgb::new(38, 191, 255)),
                            rgb: rhythm_core::Rgb::new(38, 191, 255),
                        }),
                    },
                    min_brightness: 15,
                    max_brightness: 15,
                    min_color_temp: 0,
                    max_color_temp: 0,
                    max_dim_steps: 1,
                    fade_ms: rhythm_core::TimerSetting::Auto,
                    motion_timeout_secs: rhythm_core::TimerSetting::Auto,
                    rhythm_interval_secs: rhythm_core::TimerSetting::Auto,
                },
            ],
        };

        let mut profiles = std::collections::BTreeMap::new();
        let mut runtime_config = rhythm_core::RuntimeConfig::default();
        stored.apply_to_state(&mut profiles, &mut runtime_config);

        let config = profiles
            .get(rhythm_core::DAY_IDLE_PROFILE_ID)
            .expect("day_idle missing");
        assert_eq!(config.min_brightness, 1);
        assert_eq!(config.max_brightness, 1);
        assert!(matches!(
            config.curve,
            rhythm_core::LightCurveShape::Constant {
                brightness: 1.0,
                ..
            }
        ));
    }

    #[test]
    fn load_persisted_state_missing_active_profile_falls_back_to_rhythm() {
        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: rhythm_core::default_builtin_profiles().into(),
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(app.active_mode, RhythmMode::Day);
        assert_eq!(app.active_mode_profile_id(), rhythm_core::RHYTHM_PROFILE_ID);
        assert!(app.last_active_mode_change_utc_ms.is_some());
    }

    #[test]
    fn load_persisted_state_day_idle_active_profile_falls_back_to_rhythm() {
        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: rhythm_core::default_builtin_profiles().into(),
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![rhythm_core::ModeConfig {
                    mode: RhythmMode::Day,
                    active_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                    idle_profile_id: Some(rhythm_core::DAY_IDLE_PROFILE_ID.into()),
                    wake_profile_id: None,
                    warning_profile_id: None,
                    room_defaults: vec![],
                }],
                mode_transitions: vec![],
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(app.active_mode, RhythmMode::Day);
        assert_eq!(app.active_mode_profile_id(), rhythm_core::RHYTHM_PROFILE_ID);
    }

    #[test]
    fn load_persisted_state_sleep_idle_active_profile_falls_back_to_sleep() {
        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: rhythm_core::default_builtin_profiles().into(),
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![rhythm_core::ModeConfig {
                    mode: RhythmMode::Sleep,
                    active_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                    idle_profile_id: Some(rhythm_core::SLEEP_IDLE_PROFILE_ID.into()),
                    wake_profile_id: None,
                    warning_profile_id: None,
                    room_defaults: vec![],
                }],
                mode_transitions: vec![],
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(app.active_mode, RhythmMode::Sleep);
        assert_eq!(app.active_mode_profile_id(), rhythm_core::SLEEP_PROFILE_ID);
    }

    #[test]
    fn load_persisted_state_syncs_motion_timeout_from_restored_active_profile() {
        let mut sleep = rhythm_core::default_sleep_profile();
        sleep.motion_timeout_secs = rhythm_core::TimerSetting::Fixed { value: 42 };
        sleep.rhythm_interval_secs = rhythm_core::TimerSetting::Fixed { value: 17 };

        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: vec![
                    rhythm_core::default_rhythm_profile(),
                    sleep,
                    rhythm_core::default_day_idle_profile(),
                    rhythm_core::default_sleep_idle_profile(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(app.active_mode, RhythmMode::Sleep);
        assert_eq!(app.active_mode_profile_id(), rhythm_core::SLEEP_PROFILE_ID);
        assert_eq!(app.default_motion_timeout_secs, 42);
        assert_eq!(app.runtime_config.update_interval_secs, 17);
    }

    #[test]
    fn load_persisted_state_backfills_unassigned_triage_for_roomless_devices() {
        let mut registry = crate::canonical::registry::CanonicalRegistry::new();
        let hub_key = HubKey::new(crate::hub::HubType::new("matter"), "local");
        let identity = crate::canonical::identity::DiscoveredIdentity {
            native_id: "matter-device-1".to_string(),
            room_id: String::new(),
            room_name: String::new(),
            name: "Desk Lamp".to_string(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: vec![crate::canonical::identity::HardwareId::matter(
                "matter-device-1",
            )],
            manufacturer: None,
            model: None,
        };
        let canonical_id = match registry.resolve(&identity, &hub_key, 1000) {
            crate::canonical::registry::ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {:?}", other),
        };
        assert_eq!(registry.triage().pending_unassigned_count(), 0);

        let storage = TestStorage {
            canonical_registry: Some(serde_json::to_value(&registry).unwrap()),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert!(app.canonical_registry.get(&canonical_id).is_some());
        assert_eq!(
            app.canonical_registry.triage().pending_unassigned_count(),
            1
        );
        let pending = app
            .canonical_registry
            .triage()
            .pending_by_kind(crate::canonical::triage::TriageKind::UnassignedDevice);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].canonical_id.as_deref(),
            Some(canonical_id.as_str())
        );
    }

    #[test]
    fn load_persisted_state_restores_idle_palette_override() {
        let mut idle = rhythm_core::default_day_idle_profile();
        idle.curve = rhythm_core::LightCurveShape::Palette {
            keyframes: vec![
                rhythm_core::LightPaletteKeyframe {
                    hour: 0.0,
                    r: 255,
                    g: 0,
                    b: 0,
                },
                rhythm_core::LightPaletteKeyframe {
                    hour: 24.0,
                    r: 255,
                    g: 0,
                    b: 0,
                },
            ],
        };

        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: vec![
                    rhythm_core::default_rhythm_profile(),
                    rhythm_core::default_sleep_profile(),
                    rhythm_core::default_sleep_idle_profile(),
                    idle.clone(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(Box::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(
            app.light_profile_config(rhythm_core::DAY_IDLE_PROFILE_ID)
                .unwrap(),
            &idle
        );
    }

    // ---- FileStorage tests (desktop only) ----

    mod file_storage_tests {
        use super::*;
        use std::sync::atomic::{AtomicU64, Ordering};

        static NEXT_TEMP_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

        fn temp_storage() -> (FileStorage, std::path::PathBuf) {
            let dir = std::env::temp_dir().join(format!("rhythm_test_{}", std::process::id()));
            let unique_id = NEXT_TEMP_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
            // Add a monotonic counter so parallel tests never race on the same path.
            let unique = dir.join(format!(
                "{}-{unique_id}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            let storage = FileStorage::new(unique.to_str().unwrap()).unwrap();
            (storage, unique)
        }

        fn cleanup(path: &std::path::Path) {
            let _ = std::fs::remove_dir_all(path);
        }

        #[test]
        fn new_creates_directory() {
            let (_, path) = temp_storage();
            assert!(path.exists());
            assert!(path.is_dir());
            cleanup(&path);
        }

        #[test]
        fn light_profiles_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let config = StoredLightProfiles {
                solar_noon_hour: 13.25,
                profiles: vec![rhythm_core::default_rhythm_profile()],
            };
            storage.save_light_profiles(&config).unwrap();
            let loaded = storage.load_light_profiles().unwrap();
            assert_eq!(loaded.profiles.len(), 1);
            assert_eq!(loaded.profiles[0].id, rhythm_core::RHYTHM_PROFILE_ID);
            assert!((loaded.solar_noon_hour - 13.25).abs() < 0.01);
            cleanup(&path);
        }

        #[test]
        fn light_profiles_save_load_minimal_json() {
            let (storage, path) = temp_storage();
            let json = r#"{"solar_noon_hour":12.5,"profiles":[{"id":"rhythm","name":"Day","curve":{"type":"super-gaussian"}}]}"#;
            std::fs::write(path.join("light_profiles.json"), json).unwrap();
            let loaded = storage.load_light_profiles().unwrap();
            assert_eq!(loaded.profiles.len(), 1);
            assert_eq!(loaded.profiles[0].id, rhythm_core::RHYTHM_PROFILE_ID);
            cleanup(&path);
        }

        #[test]
        fn location_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let loc = StoredLocation {
                latitude: Some(40.7128),
                longitude: Some(-74.006),
                utc_offset_hours: -5.0,
                timezone_name: Some("America/New_York".to_string()),
            };
            storage.save_location(&loc).unwrap();
            let loaded = storage.load_location().unwrap();
            assert_eq!(loaded.latitude, Some(40.7128));
            assert_eq!(loaded.longitude, Some(-74.006));
            assert!((loaded.utc_offset_hours - (-5.0)).abs() < 0.01);
            assert_eq!(loaded.timezone_name.as_deref(), Some("America/New_York"));
            cleanup(&path);
        }

        #[test]
        fn settings_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let settings = StoredSettings {
                power_save: true,
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Schedule,
                last_active_mode_transition_id: Some("sleep_to_day".into()),
                last_active_mode_change_utc_ms: Some(1_234_567_890),
                modes: rhythm_core::default_mode_configs(),
                mode_transitions: rhythm_core::default_mode_transition_configs(),
            };
            storage.save_settings(&settings).unwrap();
            let loaded = storage.load_settings().unwrap();
            assert!(loaded.power_save);
            assert_eq!(loaded.active_mode, RhythmMode::Sleep);
            assert_eq!(loaded.last_active_mode_cause, ModeChangeCause::Schedule);
            assert_eq!(
                loaded.last_active_mode_transition_id.as_deref(),
                Some("sleep_to_day")
            );
            assert_eq!(loaded.last_active_mode_change_utc_ms, Some(1_234_567_890));
            assert_eq!(loaded.modes.len(), 2);
            assert_eq!(loaded.mode_transitions.len(), 2);
            assert_eq!(
                loaded.mode_transitions[0].trigger,
                rhythm_core::ModeTransitionTrigger::AstronomicalTwilight
            );
            assert_eq!(
                loaded.mode_transitions[1].trigger,
                rhythm_core::ModeTransitionTrigger::NauticalTwilight
            );
            cleanup(&path);
        }

        #[test]
        fn hub_credentials_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let creds = HubCredentials::new(
                "hue",
                "192.168.1.1",
                serde_json::json!({"username": "test"}),
            );
            storage.save_all_hub_credentials(&[creds]).unwrap();
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].hub_type.as_ref().unwrap().as_str(), "hue");
            assert_eq!(loaded[0].address, "192.168.1.1");
            assert_eq!(loaded[0].get_str("username"), Some("test"));
            cleanup(&path);
        }

        #[test]
        fn hub_registry_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let key = HubKey::new(crate::hub::HubType::new("hue"), "192.168.1.50");
            let data = serde_json::json!({
                "devices": [{"id": "light-1", "name": "Desk Lamp"}],
                "buttons": [{"id": "switch-1", "name": "Wall Switch"}]
            });
            storage.save_hub_registry_for(&key, &data).unwrap();
            let loaded = storage.load_hub_registry_for(&key).unwrap();
            assert!(loaded.is_some());
            let loaded = loaded.unwrap();
            assert_eq!(loaded["devices"][0]["id"], "light-1");
            assert_eq!(loaded["buttons"][0]["name"], "Wall Switch");
            cleanup(&path);
        }

        #[test]
        fn hub_registry_missing_returns_none() {
            let (storage, path) = temp_storage();
            let key = HubKey::new(crate::hub::HubType::new("hue"), "192.168.1.50");
            let loaded = storage.load_hub_registry_for(&key).unwrap();
            assert!(loaded.is_none());
            cleanup(&path);
        }

        #[test]
        fn canonical_registry_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let data = serde_json::json!({
                "devices": {"dev-1": {"id": "dev-1", "name": "Light 1", "device_type": "light", "hardware_ids": [], "endpoints": []}},
                "triage": {"entries": []}
            });
            storage.save_canonical_registry(&data).unwrap();
            let loaded = storage.load_canonical_registry().unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap()["devices"]["dev-1"]["name"], "Light 1");
            cleanup(&path);
        }

        #[test]
        fn canonical_registry_missing_returns_none() {
            let (storage, path) = temp_storage();
            let loaded = storage.load_canonical_registry().unwrap();
            assert!(loaded.is_none());
            cleanup(&path);
        }

        #[test]
        fn topology_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let data = serde_json::json!({
                "rooms": {"room-1": {"id": "room-1", "name": "Kitchen", "hub_targets": [], "devices": []}}
            });
            storage.save_topology(&data).unwrap();
            let loaded = storage.load_topology().unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap()["rooms"]["room-1"]["name"], "Kitchen");
            cleanup(&path);
        }

        #[test]
        fn topology_missing_returns_none() {
            let (storage, path) = temp_storage();
            let loaded = storage.load_topology().unwrap();
            assert!(loaded.is_none());
            cleanup(&path);
        }

        #[test]
        fn commissioning_wifi_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let creds = crate::provisioning::WifiCredentials {
                ssid: "RhythmNet".to_string(),
                password: "secret-pass".to_string(),
            };

            storage.save_commissioning_wifi_credentials(&creds).unwrap();
            let loaded = storage.load_commissioning_wifi_credentials().unwrap();
            assert_eq!(loaded, Some(creds.clone()));

            storage.clear_commissioning_wifi_credentials().unwrap();
            let cleared = storage.load_commissioning_wifi_credentials().unwrap();
            assert!(cleared.is_none());
            cleanup(&path);
        }

        #[test]
        fn clear_factory_reset_state_removes_persisted_files() {
            let (storage, path) = temp_storage();
            storage
                .save_rooms(&rhythm_core::room::RoomManager::new())
                .unwrap();
            storage
                .save_light_profiles(&StoredLightProfiles {
                    solar_noon_hour: 12.5,
                    profiles: vec![rhythm_core::default_rhythm_profile()],
                })
                .unwrap();
            storage
                .save_location(&StoredLocation {
                    latitude: Some(1.0),
                    longitude: Some(2.0),
                    utc_offset_hours: 3.0,
                    timezone_name: Some("Etc/UTC".into()),
                })
                .unwrap();
            storage
                .save_settings(&StoredSettings {
                    power_save: true,
                    active_mode: RhythmMode::Sleep,
                    last_active_mode_cause: ModeChangeCause::Manual,
                    last_active_mode_transition_id: None,
                    last_active_mode_change_utc_ms: Some(123),
                    modes: rhythm_core::default_mode_configs(),
                    mode_transitions: rhythm_core::default_mode_transition_configs(),
                })
                .unwrap();
            storage
                .save_all_hub_credentials(&[HubCredentials::new(
                    "hue",
                    "192.168.1.2",
                    serde_json::json!({"username": "abc"}),
                )])
                .unwrap();
            storage
                .save_hub_registry_for(
                    &crate::canonical::identity::HubKey::new(
                        crate::hub::HubType::new("hue"),
                        "192.168.1.2",
                    ),
                    &serde_json::json!({"devices": []}),
                )
                .unwrap();
            storage
                .save_canonical_registry(
                    &serde_json::json!({"devices": {}, "triage": {"entries": []}}),
                )
                .unwrap();
            storage
                .save_topology(&serde_json::json!({"rooms": {}}))
                .unwrap();
            storage
                .save_commissioning_wifi_credentials(&crate::provisioning::WifiCredentials {
                    ssid: "RhythmNet".into(),
                    password: "secret".into(),
                })
                .unwrap();
            std::fs::create_dir_all(path.join("matter").join("captures")).unwrap();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::write(
                path.join("matter").join("captures").join("device-1.json"),
                "{}",
            )
            .unwrap();
            std::fs::write(
                path.join("matter")
                    .join("chip")
                    .join("controller-storage.json"),
                "{}",
            )
            .unwrap();

            storage.clear_factory_reset_state().unwrap();

            for name in [
                "rooms.json",
                "light_profiles.json",
                "location.json",
                "settings.json",
                "hub_credentials.json",
                "canonical_registry.json",
                "topology.json",
                "commissioning_wifi.json",
                "hub_registry_hue_192_168_1_2.json",
            ] {
                assert!(!path.join(name).exists(), "{} should be removed", name);
            }
            assert!(
                !path.join("matter").exists(),
                "integration runtime state should be removed"
            );

            cleanup(&path);
        }

        #[test]
        fn rooms_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let mut rooms = rhythm_core::room::RoomManager::new();
            rooms.get_or_create("living-room", "Living Room");
            rooms.get_or_create("bedroom", "Bedroom");
            assert_eq!(rooms.len(), 2);

            storage.save_rooms(&rooms).unwrap();
            let loaded = storage.load_rooms().unwrap();
            assert_eq!(loaded.len(), 2);
            assert!(loaded.get("living-room").is_some());
            assert_eq!(loaded.get("living-room").unwrap().name, "Living Room");
            assert!(loaded.get("bedroom").is_some());
            assert_eq!(loaded.get("bedroom").unwrap().name, "Bedroom");
            cleanup(&path);
        }

        // ---- Multi-hub credential tests ----

        #[test]
        fn all_hub_credentials_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let creds = vec![
                HubCredentials::new("hue", "192.168.1.1", serde_json::json!({"username": "abc"})),
                HubCredentials::new(
                    "homeassistant",
                    "supervisor:80",
                    serde_json::json!({"token": "xyz"}),
                ),
            ];
            storage.save_all_hub_credentials(&creds).unwrap();
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded[0].hub_type.as_ref().unwrap().as_str(), "hue");
            assert_eq!(
                loaded[1].hub_type.as_ref().unwrap().as_str(),
                "homeassistant"
            );
            cleanup(&path);
        }

        #[test]
        fn all_hub_credentials_preserves_redacted_placeholders() {
            let (storage, path) = temp_storage();
            let creds = vec![HubCredentials::redacted_placeholder("matter", "local")];
            storage.save_all_hub_credentials(&creds).unwrap();
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].hub_type.as_ref().unwrap().as_str(), "matter");
            assert_eq!(loaded[0].address, "local");
            assert!(loaded[0].secrets_redacted);
            assert!(!loaded[0].can_connect());
            cleanup(&path);
        }

        #[test]
        fn all_hub_credentials_filters_unconfigured() {
            let (storage, path) = temp_storage();
            let creds = vec![
                HubCredentials::new("hue", "1.2.3.4", serde_json::json!({})),
                HubCredentials::default(), // unconfigured
            ];
            storage.save_all_hub_credentials(&creds).unwrap();
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert_eq!(loaded.len(), 1); // unconfigured one is filtered
            cleanup(&path);
        }

        #[test]
        fn all_hub_credentials_empty_file_returns_empty() {
            let (storage, path) = temp_storage();
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert!(loaded.is_empty());
            cleanup(&path);
        }

        #[test]
        fn hub_registry_for_per_hub_file() {
            let (storage, path) = temp_storage();
            let key = HubKey::new(crate::hub::HubType::new("hue"), "192.168.1.50");
            let data = serde_json::json!({"rooms": [{"id": "kitchen"}]});
            storage.save_hub_registry_for(&key, &data).unwrap();

            let loaded = storage.load_hub_registry_for(&key).unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap()["rooms"][0]["id"], "kitchen");
            cleanup(&path);
        }

        #[test]
        fn sanitize_hub_key_replaces_special_chars() {
            let key = HubKey::new(crate::hub::HubType::new("hue"), "192.168.1.100");
            let sanitized = super::sanitize_hub_key(&key);
            assert_eq!(sanitized, "hue_192_168_1_100");
            assert!(!sanitized.contains('.'));
            assert!(!sanitized.contains(':'));
        }
    }
}
