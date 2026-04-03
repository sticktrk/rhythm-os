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
use rhythm_core::CurveConfig;
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
    fn load_config(&self) -> Result<StoredConfig>;
    fn save_config(&self, config: &StoredConfig) -> Result<()>;
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

/// Curve configuration for persistence.
///
/// Legacy `utc_offset_hours` field is accepted on deserialization but ignored —
/// location is the single owner of UTC offset.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredConfig {
    pub min_brightness: u8,
    pub max_brightness: u8,
    pub min_color_temp: u16,
    pub max_color_temp: u16,
    pub solar_noon_hour: f32,
    #[serde(default = "default_width_left_bri")]
    pub width_left_bri: f32,
    #[serde(default = "default_width_right_bri")]
    pub width_right_bri: f32,
    #[serde(default = "default_width_left_cct")]
    pub width_left_cct: f32,
    #[serde(default = "default_width_right_cct")]
    pub width_right_cct: f32,
    #[serde(default = "default_shape_p")]
    pub shape_p: f32,
    #[serde(default = "default_max_dim_steps")]
    pub max_dim_steps: u8,
    #[serde(default, deserialize_with = "deserialize_fade_ms")]
    pub fade_ms: Option<u16>,
    #[serde(default, deserialize_with = "deserialize_motion_timeout_secs")]
    pub motion_timeout_secs: Option<u16>,
}

fn default_width_left_bri() -> f32 {
    rhythm_core::config::DEFAULT_WIDTH_LEFT_BRI
}
fn default_width_right_bri() -> f32 {
    rhythm_core::config::DEFAULT_WIDTH_RIGHT_BRI
}
fn default_width_left_cct() -> f32 {
    rhythm_core::config::DEFAULT_WIDTH_LEFT_CCT
}
fn default_width_right_cct() -> f32 {
    rhythm_core::config::DEFAULT_WIDTH_RIGHT_CCT
}
fn default_shape_p() -> f32 {
    rhythm_core::config::DEFAULT_SHAPE_P
}
fn default_max_dim_steps() -> u8 {
    rhythm_core::config::DEFAULT_MAX_DIM_STEPS
}
/// Migrate old stored `fade_ms: 500` (the old default) to `None` (auto).
/// Accepts both integer (old format) and null/missing (new format).
fn deserialize_fade_ms<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    let v: Option<u16> = Option::deserialize(d)?;
    Ok(v.filter(|&x| x != rhythm_core::config::DEFAULT_FADE_MS))
}

/// Migrate old stored `motion_timeout_secs: 1200` (the old default) to `None` (auto).
/// Accepts both integer (old format) and null/missing (new format).
fn deserialize_motion_timeout_secs<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<u16>, D::Error> {
    let v: Option<u16> = Option::deserialize(d)?;
    Ok(v.filter(|&x| x != rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS))
}

impl StoredConfig {
    pub fn from_state(config: &CurveConfig, runtime_config: &RuntimeConfig) -> Self {
        Self {
            min_brightness: config.min_brightness,
            max_brightness: config.max_brightness,
            min_color_temp: config.min_color_temp,
            max_color_temp: config.max_color_temp,
            solar_noon_hour: runtime_config.solar_noon_hour,
            width_left_bri: config.width_left_bri,
            width_right_bri: config.width_right_bri,
            width_left_cct: config.width_left_cct,
            width_right_cct: config.width_right_cct,
            shape_p: config.shape_p,
            max_dim_steps: config.max_dim_steps,
            fade_ms: config.fade_ms,
            motion_timeout_secs: config.motion_timeout_secs,
        }
    }

    pub fn apply_to_state(&self, config: &mut CurveConfig, runtime_config: &mut RuntimeConfig) {
        config.min_brightness = self.min_brightness;
        config.max_brightness = self.max_brightness;
        config.min_color_temp = self.min_color_temp;
        config.max_color_temp = self.max_color_temp;
        config.width_left_bri = self.width_left_bri;
        config.width_right_bri = self.width_right_bri;
        config.width_left_cct = self.width_left_cct;
        config.width_right_cct = self.width_right_cct;
        config.shape_p = self.shape_p;
        config.max_dim_steps = self.max_dim_steps;
        config.fade_ms = self.fade_ms;
        config.motion_timeout_secs = self.motion_timeout_secs;
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
    pub rhythm_interval_secs: u64,
    pub power_save: bool,
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

    fn load_config(&self) -> Result<StoredConfig> {
        self.read_json("config.json")
    }

    fn save_config(&self, config: &StoredConfig) -> Result<()> {
        let data = serde_json::to_string_pretty(config)?;
        self.write_atomic("config.json", data.as_bytes())
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
    let storage = match s.storage.as_ref() {
        Some(st) => st,
        None => return,
    };

    if let Ok(config) = storage.load_config() {
        config.apply_to_state(&mut s.config, &mut s.runtime_config);
        // Sync curve-provided motion timeout as the baseline default.
        // StoredSettings load below may override this if the user set it explicitly.
        s.default_motion_timeout_secs =
            s.config.motion_timeout_secs.unwrap_or(rhythm_core::config::DEFAULT_MOTION_TIMEOUT_SECS) as u64;
        let c = &s.config;
        info!(target: "sys", "Loaded config: bri={}–{}%, cct={}–{}K, width_bri=L{}/R{}, width_cct=L{}/R{}, shape_p={}, solar_noon={}",
            c.min_brightness, c.max_brightness,
            c.min_color_temp, c.max_color_temp,
            c.width_left_bri, c.width_right_bri,
            c.width_left_cct, c.width_right_cct,
            c.shape_p, s.runtime_config.solar_noon_hour);
    }

    if let Ok(loc) = storage.load_location() {
        loc.apply_to_state(
            &mut s.latitude,
            &mut s.longitude,
            &mut s.utc_offset_hours,
            &mut s.runtime_config,
            &mut s.timezone_name,
        );
    }

    if let Ok(settings) = storage.load_settings() {
        s.runtime_config.update_interval_secs = settings.rhythm_interval_secs;
        s.power_save = settings.power_save;
        info!(target: "sys", "Loaded settings: interval={}s", s.runtime_config.update_interval_secs);
    }

    // Load hub credentials
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
            // Try legacy single-credential load
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

    info!(target: "sys", "Persisted state loaded");
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- StoredConfig tests (no feature gate needed) ----

    #[test]
    fn stored_config_from_state_roundtrip() {
        // Create CurveConfig and RuntimeConfig, convert to StoredConfig, apply back
        let config = rhythm_core::CurveConfig {
            min_brightness: 5,
            max_brightness: 95,
            min_color_temp: 2200,
            max_color_temp: 5500,
            width_left_bri: 0.7,
            width_right_bri: 1.3,
            width_left_cct: 0.8,
            width_right_cct: 1.1,
            shape_p: 4.0,
            max_dim_steps: 8,
            fade_ms: None,
            motion_timeout_secs: Some(300),
        };
        let runtime_config = rhythm_core::RuntimeConfig::default().with_solar_noon(12.8);

        let stored = StoredConfig::from_state(&config, &runtime_config);
        assert_eq!(stored.min_brightness, 5);
        assert_eq!(stored.max_brightness, 95);
        assert_eq!(stored.min_color_temp, 2200);
        assert_eq!(stored.max_color_temp, 5500);
        assert!((stored.solar_noon_hour - 12.8).abs() < 0.01);
        assert!((stored.width_left_bri - 0.7).abs() < 0.001);
        assert!((stored.width_right_bri - 1.3).abs() < 0.001);
        assert!((stored.shape_p - 4.0).abs() < 0.001);
        assert_eq!(stored.max_dim_steps, 8);
        assert_eq!(stored.fade_ms, None);
        assert_eq!(stored.motion_timeout_secs, Some(300));

        let mut config2 = rhythm_core::CurveConfig::default();
        let mut runtime_config2 = rhythm_core::RuntimeConfig::default();
        stored.apply_to_state(&mut config2, &mut runtime_config2);
        assert_eq!(config2.min_brightness, 5);
        assert_eq!(config2.max_brightness, 95);
        assert_eq!(config2.min_color_temp, 2200);
        assert_eq!(config2.max_color_temp, 5500);
        assert!((config2.width_left_bri - 0.7).abs() < 0.001);
        assert!((config2.width_right_bri - 1.3).abs() < 0.001);
        assert!((config2.width_left_cct - 0.8).abs() < 0.001);
        assert!((config2.width_right_cct - 1.1).abs() < 0.001);
        assert!((config2.shape_p - 4.0).abs() < 0.001);
        assert_eq!(config2.max_dim_steps, 8);
        assert!((runtime_config2.solar_noon_hour - 12.8).abs() < 0.01);
    }

    // ---- FileStorage tests (desktop only) ----

    #[cfg(feature = "desktop")]
    mod file_storage_tests {
        use super::*;
        use crate::hub::HubCredentials;

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
        fn config_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let config = StoredConfig {
                min_brightness: 10,
                max_brightness: 90,
                min_color_temp: 2000,
                max_color_temp: 6000,
                solar_noon_hour: 13.25,
                width_left_bri: 0.7,
                width_right_bri: 1.2,
                width_left_cct: 0.8,
                width_right_cct: 1.1,
                shape_p: 4.0,
                max_dim_steps: 8,
                fade_ms: Some(300),
                motion_timeout_secs: Some(300),
            };
            storage.save_config(&config).unwrap();
            let loaded = storage.load_config().unwrap();
            assert_eq!(loaded.min_brightness, 10);
            assert_eq!(loaded.max_brightness, 90);
            assert_eq!(loaded.min_color_temp, 2000);
            assert_eq!(loaded.max_color_temp, 6000);
            assert!((loaded.solar_noon_hour - 13.25).abs() < 0.01);
            assert!((loaded.width_left_bri - 0.7).abs() < 0.001);
            assert!((loaded.width_right_bri - 1.2).abs() < 0.001);
            assert!((loaded.shape_p - 4.0).abs() < 0.001);
            assert_eq!(loaded.fade_ms, Some(300));
            assert_eq!(loaded.motion_timeout_secs, Some(300));
            assert_eq!(loaded.max_dim_steps, 8);
            cleanup(&path);
        }

        #[test]
        fn config_legacy_json_uses_defaults_for_new_fields() {
            // Simulate loading a config.json from before width fields were added
            let (storage, path) = temp_storage();
            let legacy_json = r#"{"min_brightness":5,"max_brightness":90,"min_color_temp":2000,"max_color_temp":5500,"solar_noon_hour":12.5}"#;
            std::fs::write(path.join("config.json"), legacy_json).unwrap();
            let loaded = storage.load_config().unwrap();
            assert_eq!(loaded.min_brightness, 5);
            assert_eq!(loaded.max_brightness, 90);
            // New fields should get defaults
            assert!(
                (loaded.width_left_bri - rhythm_core::config::DEFAULT_WIDTH_LEFT_BRI).abs() < 0.001
            );
            assert!(
                (loaded.width_right_bri - rhythm_core::config::DEFAULT_WIDTH_RIGHT_BRI).abs()
                    < 0.001
            );
            assert!((loaded.shape_p - rhythm_core::config::DEFAULT_SHAPE_P).abs() < 0.001);
            assert_eq!(
                loaded.max_dim_steps,
                rhythm_core::config::DEFAULT_MAX_DIM_STEPS
            );
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
                rhythm_interval_secs: 120,
                power_save: true,
            };
            storage.save_settings(&settings).unwrap();
            let loaded = storage.load_settings().unwrap();
            assert_eq!(loaded.rhythm_interval_secs, 120);
            assert!(loaded.power_save);
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
