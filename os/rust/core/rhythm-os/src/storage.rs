//! Abstract persistence interface for Rhythm OS.
//!
//! Platform crates implement this trait to provide concrete storage
//! (e.g., NVS on ESP32, filesystem on Linux, SQLite on Raspberry Pi).

#[cfg(feature = "desktop")]
use anyhow::Context;
use anyhow::Result;
use chrono::{Datelike, Timelike};
use log::info;
use rhythm_core::room::RoomManager;
use rhythm_core::LightProfileConfig;
use rhythm_core::RuntimeConfig;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canonical::identity::HubKey;
use crate::hub::HubCredentials;

/// Abstract persistence interface.
///
/// Each method loads/saves a single domain object. Implementations should
/// be safe to call from any thread (`Send + Sync`).
///
/// The canonical registry and topology store methods have default no-op
/// implementations for backwards compatibility (ESP32 doesn't use them).
pub trait Storage: Send + Sync {
    fn load_rooms(&self) -> Result<RoomManager>;
    fn save_rooms(&self, rooms: &RoomManager) -> Result<()>;
    fn load_light_profiles(&self) -> Result<StoredLightProfiles>;
    fn save_light_profiles(&self, config: &StoredLightProfiles) -> Result<()>;
    fn load_location(&self) -> Result<StoredLocation>;
    fn save_location(&self, loc: &StoredLocation) -> Result<()>;
    fn load_settings(&self) -> Result<StoredSettings>;
    fn save_settings(&self, settings: &StoredSettings) -> Result<()>;
    fn load_hub_credentials(&self) -> Result<HubCredentials>;
    fn save_hub_credentials(&self, creds: &HubCredentials) -> Result<()>;

    /// Load all hub credentials (multi-hub support).
    ///
    /// Default implementation wraps `load_hub_credentials` into a single-element vec.
    /// The FileStorage implementation handles both legacy single-object and new array format.
    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
        match self.load_hub_credentials() {
            Ok(creds) if creds.is_configured() => Ok(vec![creds]),
            Ok(_) => Ok(Vec::new()),
            Err(_) => Ok(Vec::new()),
        }
    }

    /// Save all hub credentials (multi-hub support).
    ///
    /// Default implementation saves the first credential via `save_hub_credentials`.
    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
        if let Some(first) = creds.first() {
            self.save_hub_credentials(first)
        } else {
            self.save_hub_credentials(&HubCredentials::default())
        }
    }

    fn load_hub_registry(&self) -> Result<Option<Value>>;
    fn save_hub_registry(&self, data: &Value) -> Result<()>;

    /// Load hub registry for a specific hub key. Default: falls back to load_hub_registry.
    fn load_hub_registry_for(&self, _key: &HubKey) -> Result<Option<Value>> {
        self.load_hub_registry()
    }

    /// Save hub registry for a specific hub key. Default: falls back to save_hub_registry.
    fn save_hub_registry_for(&self, _key: &HubKey, data: &Value) -> Result<()> {
        self.save_hub_registry(data)
    }

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
        profiles.clear();
        for profile in &self.profiles {
            profiles.insert(profile.id.clone(), profile.clone());
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
            let now = chrono::Utc::now().naive_utc();
            let (year, month, day) = (now.date().year(), now.date().month(), now.date().day());
            *utc_offset_hours = tz.utc_offset(year, month, day, now.time().hour());
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
    pub active_light_profile: String,
}

// ---------------------------------------------------------------------------
// FileStorage — filesystem backend (desktop targets only)
// ---------------------------------------------------------------------------

/// Filesystem storage backend.
///
/// Implements [`Storage`] using JSON files with atomic writes (write to .tmp,
/// then rename). Used by rhythm-addon and rhythm-server.
#[cfg(feature = "desktop")]
pub struct FileStorage {
    dir: std::path::PathBuf,
}

#[cfg(feature = "desktop")]
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
}

#[cfg(feature = "desktop")]
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

    fn load_hub_credentials(&self) -> Result<HubCredentials> {
        self.read_json("hub_credentials.json")
    }

    fn save_hub_credentials(&self, creds: &HubCredentials) -> Result<()> {
        let data = serde_json::to_string_pretty(creds)?;
        self.write_atomic("hub_credentials.json", data.as_bytes())
    }

    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>> {
        let path = self.file_path("hub_credentials.json");
        let data = match std::fs::read_to_string(&path) {
            Ok(d) => d,
            Err(_) => return Ok(Vec::new()),
        };
        let value: serde_json::Value = serde_json::from_str(&data)
            .with_context(|| format!("Failed to parse {}", path.display()))?;

        // Handle both legacy single-object and new array format
        if value.is_array() {
            let creds: Vec<HubCredentials> = serde_json::from_value(value)?;
            Ok(creds.into_iter().filter(|c| c.is_configured()).collect())
        } else {
            let creds: HubCredentials = serde_json::from_value(value)?;
            if creds.is_configured() {
                Ok(vec![creds])
            } else {
                Ok(Vec::new())
            }
        }
    }

    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()> {
        // Always write as array for forward compat
        let data = serde_json::to_string_pretty(creds)?;
        self.write_atomic("hub_credentials.json", data.as_bytes())
    }

    fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>> {
        let filename = format!("hub_registry_{}.json", sanitize_hub_key(key));
        match self.read_json::<Value>(&filename) {
            Ok(v) => Ok(Some(v)),
            Err(_) => {
                // Fallback to legacy un-keyed file
                self.load_hub_registry()
            }
        }
    }

    fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()> {
        let filename = format!("hub_registry_{}.json", sanitize_hub_key(key));
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic(&filename, json.as_bytes())
    }

    fn load_hub_registry(&self) -> Result<Option<serde_json::Value>> {
        match self.read_json::<serde_json::Value>("hub_registry.json") {
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }

    fn save_hub_registry(&self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic("hub_registry.json", json.as_bytes())
    }

    fn load_canonical_registry(&self) -> Result<Option<serde_json::Value>> {
        match self.read_json::<serde_json::Value>("canonical_registry.json") {
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }

    fn save_canonical_registry(&self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic("canonical_registry.json", json.as_bytes())
    }

    fn load_topology(&self) -> Result<Option<serde_json::Value>> {
        match self.read_json::<serde_json::Value>("topology.json") {
            Ok(v) => Ok(Some(v)),
            Err(_) => Ok(None),
        }
    }

    fn save_topology(&self, data: &serde_json::Value) -> Result<()> {
        let json = serde_json::to_string_pretty(data)?;
        self.write_atomic("topology.json", json.as_bytes())
    }
}

/// Sanitize a HubKey into a filesystem-safe string for per-hub filenames.
#[cfg(feature = "desktop")]
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
        if let Ok(configs) = storage.load_light_profiles() {
            configs.apply_to_state(&mut s.light_profile_configs, &mut s.runtime_config);
            s.ensure_active_light_profile();
            s.sync_active_light_profile_runtime_overrides();
            info!(
                target: "sys",
                "Loaded light profiles: {} profiles, solar_noon={}",
                s.light_profile_configs.len(),
                s.runtime_config.solar_noon_hour
            );
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        if let Ok(loc) = storage.load_location() {
            loc.apply_to_state(
                &mut s.latitude,
                &mut s.longitude,
                &mut s.utc_offset_hours,
                &mut s.runtime_config,
                &mut s.timezone_name,
            );
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        if let Ok(settings) = storage.load_settings() {
            s.power_save = settings.power_save;
            s.active_light_profile_id = settings.active_light_profile;
            s.ensure_active_light_profile();
            s.sync_active_light_profile_runtime_overrides();
            info!(
                target: "sys",
                "Loaded settings: active_light_profile={}",
                s.active_light_profile_id
            );
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_all_hub_credentials() {
            Ok(all_creds) if !all_creds.is_empty() => {
                for creds in all_creds {
                    info!(target: "sys", "Loaded hub credentials: type={:?}, addr={}", creds.hub_type, creds.address);
                    if let Some(key) = creds.hub_key() {
                        s.hub_credentials.insert(key, creds);
                    }
                }
            }
            _ => {
                if let Ok(creds) = storage.load_hub_credentials() {
                    if creds.is_configured() {
                        info!(target: "sys", "Loaded hub credentials: type={:?}, addr={}", creds.hub_type, creds.address);
                        if let Some(key) = creds.hub_key() {
                            s.hub_credentials.insert(key, creds);
                        }
                    }
                }
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        if let Ok(Some(value)) = storage.load_canonical_registry() {
            match serde_json::from_value::<crate::canonical::registry::CanonicalRegistry>(value) {
                Ok(mut registry) => {
                    registry.rebuild_indices();
                    let count = registry.device_count();
                    s.canonical_registry = registry;
                    info!(target: "sys", "Loaded canonical registry: {} devices", count);
                }
                Err(e) => {
                    info!(target: "sys", "Failed to parse canonical registry (will start fresh): {}", e);
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
                    info!(target: "sys", "Failed to parse topology (will start fresh): {}", e);
                }
            }
        }
    }

    info!(target: "sys", "Persisted state loaded");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::HubCredentials;

    #[derive(Default)]
    struct TestStorage {
        light_profiles: Option<StoredLightProfiles>,
        location: Option<StoredLocation>,
        settings: Option<StoredSettings>,
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
            Ok(())
        }

        fn load_hub_credentials(&self) -> Result<HubCredentials> {
            Ok(HubCredentials::default())
        }

        fn save_hub_credentials(&self, _creds: &HubCredentials) -> Result<()> {
            Ok(())
        }

        fn load_hub_registry(&self) -> Result<Option<Value>> {
            Ok(None)
        }

        fn save_hub_registry(&self, _data: &Value) -> Result<()> {
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
        assert!((runtime_config2.solar_noon_hour - 12.8).abs() < 0.01);
    }

    #[test]
    fn load_persisted_state_missing_active_profile_falls_back_to_rhythm() {
        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: vec![
                    rhythm_core::default_rhythm_profile(),
                    rhythm_core::default_sleep_profile(),
                    rhythm_core::default_idle_profile(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_light_profile: "missing-profile".to_string(),
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(storage));
        load_persisted_state(&mut app);

        assert_eq!(app.active_light_profile_id, rhythm_core::RHYTHM_PROFILE_ID);
    }

    #[test]
    fn load_persisted_state_idle_active_profile_falls_back_to_rhythm() {
        let storage = TestStorage {
            light_profiles: Some(StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: vec![
                    rhythm_core::default_rhythm_profile(),
                    rhythm_core::default_sleep_profile(),
                    rhythm_core::default_idle_profile(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_light_profile: rhythm_core::IDLE_PROFILE_ID.to_string(),
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(storage));
        load_persisted_state(&mut app);

        assert_eq!(app.active_light_profile_id, rhythm_core::RHYTHM_PROFILE_ID);
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
                    rhythm_core::default_idle_profile(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_light_profile: rhythm_core::SLEEP_PROFILE_ID.to_string(),
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(storage));
        load_persisted_state(&mut app);

        assert_eq!(app.active_light_profile_id, rhythm_core::SLEEP_PROFILE_ID);
        assert_eq!(app.default_motion_timeout_secs, 42);
        assert_eq!(app.runtime_config.update_interval_secs, 17);
    }

    #[test]
    fn load_persisted_state_restores_idle_palette_override() {
        let mut idle = rhythm_core::default_idle_profile();
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
                    idle.clone(),
                ],
            }),
            settings: Some(StoredSettings {
                power_save: false,
                active_light_profile: rhythm_core::RHYTHM_PROFILE_ID.to_string(),
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState::default();
        app.storage = Some(Box::new(storage));
        load_persisted_state(&mut app);

        assert_eq!(
            app.light_profile_config(rhythm_core::IDLE_PROFILE_ID)
                .unwrap(),
            &idle
        );
    }

    // ---- FileStorage tests (desktop only) ----

    #[cfg(feature = "desktop")]
    mod file_storage_tests {
        use super::*;

        fn temp_storage() -> (FileStorage, std::path::PathBuf) {
            let dir = std::env::temp_dir().join(format!("rhythm_test_{}", std::process::id()));
            // Unique subdirectory per test to avoid conflicts
            let unique = dir.join(format!(
                "{}",
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
                active_light_profile: rhythm_core::SLEEP_PROFILE_ID.to_string(),
            };
            storage.save_settings(&settings).unwrap();
            let loaded = storage.load_settings().unwrap();
            assert!(loaded.power_save);
            assert_eq!(loaded.active_light_profile, rhythm_core::SLEEP_PROFILE_ID);
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
            storage.save_hub_credentials(&creds).unwrap();
            let loaded = storage.load_hub_credentials().unwrap();
            assert!(loaded.is_configured());
            assert_eq!(loaded.hub_type.as_ref().unwrap().as_str(), "hue");
            assert_eq!(loaded.address, "192.168.1.1");
            assert_eq!(loaded.get_str("username"), Some("test"));
            cleanup(&path);
        }

        #[test]
        fn hub_registry_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let data = serde_json::json!({
                "devices": [{"id": "light-1", "name": "Desk Lamp"}],
                "buttons": [{"id": "switch-1", "name": "Wall Switch"}]
            });
            storage.save_hub_registry(&data).unwrap();
            let loaded = storage.load_hub_registry().unwrap();
            assert!(loaded.is_some());
            let loaded = loaded.unwrap();
            assert_eq!(loaded["devices"][0]["id"], "light-1");
            assert_eq!(loaded["buttons"][0]["name"], "Wall Switch");
            cleanup(&path);
        }

        #[test]
        fn hub_registry_missing_returns_none() {
            let (storage, path) = temp_storage();
            let loaded = storage.load_hub_registry().unwrap();
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
        fn all_hub_credentials_loads_legacy_single_object() {
            let (storage, path) = temp_storage();
            // Write legacy single-object format
            let creds =
                HubCredentials::new("hue", "10.0.0.1", serde_json::json!({"username": "u"}));
            storage.save_hub_credentials(&creds).unwrap();
            // load_all should parse it as a single-element vec
            let loaded = storage.load_all_hub_credentials().unwrap();
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].address, "10.0.0.1");
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
        fn hub_registry_for_falls_back_to_legacy() {
            let (storage, path) = temp_storage();
            // Write legacy hub_registry.json
            let data = serde_json::json!({"rooms": [{"id": "legacy_room"}]});
            storage.save_hub_registry(&data).unwrap();

            // Load via per-hub key that doesn't have its own file
            let key = HubKey::new(crate::hub::HubType::new("hue"), "unknown_ip");
            let loaded = storage.load_hub_registry_for(&key).unwrap();
            assert!(loaded.is_some());
            assert_eq!(loaded.unwrap()["rooms"][0]["id"], "legacy_room");
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
