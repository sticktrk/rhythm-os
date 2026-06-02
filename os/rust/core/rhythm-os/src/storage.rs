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
use crate::scenes::StoredScenes;

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
    fn load_scenes(&self) -> Result<Option<StoredScenes>> {
        Ok(None)
    }
    fn save_scenes(&self, _scenes: &StoredScenes) -> Result<()> {
        Ok(())
    }
    fn load_motion_timers(&self) -> Result<Option<StoredMotionTimers>> {
        Ok(None)
    }
    fn save_motion_timers(&self, _timers: &StoredMotionTimers) -> Result<()> {
        Ok(())
    }
    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>>;
    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()>;
    fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>>;
    fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()>;
    fn clear_hub_registries(&self) -> Result<()> {
        Ok(())
    }

    /// Load integration-owned durable files for a backup bundle.
    ///
    /// Implementations should only include secret-bearing files when
    /// `include_secrets` is true. Default: no integration files.
    fn load_integration_backup_files(
        &self,
        _include_secrets: bool,
    ) -> Result<Vec<crate::bundle::BackupIntegrationFile>> {
        Ok(Vec::new())
    }

    /// Replace integration-owned durable files during backup restore.
    ///
    /// A restore with an empty file list should clear known integration state,
    /// matching the restored backup's authoritative contents. Default: no-op.
    fn restore_integration_backup_files(
        &self,
        _files: &[crate::bundle::BackupIntegrationFile],
    ) -> Result<()> {
        Ok(())
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

    /// Load local API auth state. Default: no configured tokens.
    fn load_api_auth(&self) -> Result<Option<crate::auth::StoredApiAuth>> {
        Ok(None)
    }

    /// Persist local API auth state. Default: no-op.
    fn save_api_auth(&self, _auth: &crate::auth::StoredApiAuth) -> Result<()> {
        Ok(())
    }

    /// Clear local API auth state. Default: no-op.
    fn clear_api_auth(&self) -> Result<()> {
        Ok(())
    }

    /// Clear all persisted state that should not survive a full factory reset.
    ///
    /// Active desktop/server platforms use this to remove stale keyed hub
    /// registries and any other persisted artifacts that would otherwise be
    /// resurrected after a reset.
    fn clear_factory_reset_state(&self) -> Result<()> {
        self.clear_commissioning_wifi_credentials()?;
        self.clear_api_auth()
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
            normalize_legacy_persisted_builtin_state_profile(&mut normalized);
            rhythm_core::normalize_builtin_state_profile_config(&mut normalized);
            profiles.insert(normalized.id.clone(), normalized);
        }
        runtime_config.solar_noon_hour = self.solar_noon_hour;
    }
}

fn normalize_legacy_persisted_builtin_state_profile(config: &mut LightProfileConfig) {
    if !rhythm_core::is_builtin_state_profile_id(&config.id) {
        return;
    }

    let rhythm_core::LightCurveShape::Constant { brightness, .. } = &config.curve else {
        return;
    };

    if *brightness > 1.0
        && config.min_brightness == config.max_brightness
        && config.min_color_temp == 0
        && config.max_color_temp == 0
        && config.max_dim_steps == 1
        && config.fade_ms.is_auto()
        && config.motion_timeout_secs.is_auto()
        && config.rhythm_interval_secs.is_auto()
    {
        config.min_brightness = 1;
        config.max_brightness = 1;
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
        let has_location = self.latitude.is_some()
            || self.longitude.is_some()
            || self.timezone_name.is_some()
            || self.utc_offset_hours.abs() > f32::EPSILON;

        if let Some(ref tz_name) = self.timezone_name {
            let tz = rhythm_core::Timezone::new(tz_name);
            let (year, month, day, hour) =
                tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
            *utc_offset_hours = tz.utc_offset(year, month, day, hour);
            if let Some(lon) = self.longitude {
                runtime_config.solar_noon_hour =
                    rhythm_core::calculate_solar_noon(lon, year, month, day, &tz);
            }
            crate::logging::update_log_clock_from_location(
                self.timezone_name.as_deref(),
                *utc_offset_hours,
                has_location,
            );
            info!(target: "sys", "Loaded location: lat={:?}, lon={:?}, tz={}, utc_offset={}, solar_noon={:.2}",
                latitude, longitude, tz_name, utc_offset_hours, runtime_config.solar_noon_hour);
        } else {
            *utc_offset_hours = self.utc_offset_hours;
            crate::logging::update_log_clock_from_location(None, *utc_offset_hours, has_location);
            info!(target: "sys", "Loaded location: lat={:?}, lon={:?}, utc_offset={}",
                latitude, longitude, utc_offset_hours);
        }
    }
}

/// Global settings for persistence.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSettings {
    pub power_save: bool,
    #[serde(default = "default_light_breaker_enabled")]
    pub light_breaker_enabled: bool,
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
    /// When true, the appliance silently polls the curated "stable" OTA feed
    /// and applies updates during the overnight window. When false, it polls
    /// the "beta" feed and only updates on an explicit `POST /api/ota/update`.
    #[serde(default = "default_auto_update")]
    pub auto_update: bool,
}

fn default_auto_update() -> bool {
    true
}

fn default_light_breaker_enabled() -> bool {
    true
}

/// Runtime motion timer state persisted across process restarts.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredMotionTimers {
    #[serde(default = "stored_motion_timers_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub entries: Vec<StoredMotionTimerEntry>,
}

fn stored_motion_timers_schema_version() -> u32 {
    1
}

impl Default for StoredMotionTimers {
    fn default() -> Self {
        Self {
            schema_version: stored_motion_timers_schema_version(),
            entries: Vec::new(),
        }
    }
}

/// One persisted motion source timer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredMotionTimerEntry {
    pub source_node_id: String,
    pub target_node_id: String,
    /// `None` means the source was still active when persisted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped_at_epoch_ms: Option<u64>,
    #[serde(default)]
    pub motion_owned: bool,
    #[serde(default)]
    pub warning_active: bool,
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

const INTEGRATION_SUBDIRS: &[&str] = &["matter"];

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

    /// Durable atomic write: write to `.tmp`, fsync the data, atomically rename
    /// into place, then fsync the parent directory so the rename survives a
    /// power loss. Removes any stale `.tmp` left behind by a crashed prior
    /// write before starting.
    fn write_atomic(&self, name: &str, data: &[u8]) -> Result<()> {
        use std::io::Write;

        let path = self.file_path(name);
        let tmp = self.file_path(&format!("{}.tmp", name));

        // A prior crashed write can leave a stale `.tmp`. Remove it so File::create
        // below doesn't silently inherit partial contents on platforms that
        // don't truncate on open.
        if tmp.exists() {
            let _ = std::fs::remove_file(&tmp);
        }

        {
            let mut file = std::fs::File::create(&tmp)
                .with_context(|| format!("Failed to create {}", tmp.display()))?;
            file.write_all(data)
                .with_context(|| format!("Failed to write {}", tmp.display()))?;
            file.sync_all()
                .with_context(|| format!("Failed to fsync {}", tmp.display()))?;
        }

        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow::anyhow!(e)).with_context(|| {
                format!("Failed to rename {} -> {}", tmp.display(), path.display())
            });
        }

        if let Ok(dir) = std::fs::File::open(&self.dir) {
            let _ = dir.sync_all();
        }

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

    fn clear_hub_registry_files(&self) -> Result<()> {
        self.remove_if_exists("hub_registry.json")?;

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

        Ok(())
    }

    fn clear_integration_state_dirs(&self) -> Result<()> {
        for dir in INTEGRATION_SUBDIRS {
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

    fn collect_integration_backup_files(
        &self,
        relative_dir: &std::path::Path,
        files: &mut Vec<crate::bundle::BackupIntegrationFile>,
    ) -> Result<()> {
        let absolute_dir = self.dir.join(relative_dir);
        let entries = match std::fs::read_dir(&absolute_dir) {
            Ok(entries) => entries,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => {
                return Err(anyhow::anyhow!(e))
                    .with_context(|| format!("reading {}", absolute_dir.display()));
            }
        };

        for entry in entries {
            let entry = entry?;
            let path = entry.path();
            let file_type = entry.file_type()?;
            let relative_path = relative_dir.join(entry.file_name());
            if file_type.is_dir() {
                if should_skip_integration_backup_dir(&relative_path) {
                    continue;
                }
                self.collect_integration_backup_files(&relative_path, files)?;
            } else if file_type.is_file() {
                if should_skip_integration_backup_file(&relative_path) {
                    continue;
                }
                let path_string = normalized_relative_path(&relative_path)?;
                let content = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                files.push(crate::bundle::BackupIntegrationFile {
                    path: path_string,
                    content,
                    secret: true,
                });
            }
        }

        Ok(())
    }

    fn write_file_atomic_path(path: &std::path::Path, data: &[u8]) -> Result<()> {
        use std::io::Write;

        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("integration backup path has no parent"))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| anyhow::anyhow!("invalid integration backup file name"))?;
        let tmp = path.with_file_name(format!("{filename}.tmp"));

        {
            let mut file = std::fs::File::create(&tmp)
                .with_context(|| format!("creating {}", tmp.display()))?;
            file.write_all(data)
                .with_context(|| format!("writing {}", tmp.display()))?;
            file.sync_all()
                .with_context(|| format!("fsync {}", tmp.display()))?;
        }

        if let Err(e) = std::fs::rename(&tmp, path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(anyhow::anyhow!(e))
                .with_context(|| format!("renaming {} -> {}", tmp.display(), path.display()));
        }
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
        Ok(())
    }
}

fn should_skip_integration_backup_dir(relative_path: &std::path::Path) -> bool {
    relative_path.components().any(
        |component| matches!(component, std::path::Component::Normal(name) if name == "captures"),
    )
}

fn should_skip_integration_backup_file(relative_path: &std::path::Path) -> bool {
    let Some(name) = relative_path.file_name().and_then(|name| name.to_str()) else {
        return true;
    };
    name == "chip-controller.sock"
        || name.ends_with(".sock")
        || name.ends_with(".tmp")
        || name.ends_with(".log")
}

fn normalized_relative_path(path: &std::path::Path) -> Result<String> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            std::path::Component::Normal(part) => {
                let part = part
                    .to_str()
                    .ok_or_else(|| anyhow::anyhow!("integration backup path is not UTF-8"))?;
                parts.push(part.to_string());
            }
            _ => anyhow::bail!("invalid integration backup path {}", path.display()),
        }
    }
    if parts.is_empty() {
        anyhow::bail!("integration backup path must not be empty");
    }
    Ok(parts.join("/"))
}

fn validate_integration_backup_path(path: &str) -> Result<std::path::PathBuf> {
    if path.trim().is_empty() {
        anyhow::bail!("integration backup path must not be empty");
    }
    let relative = std::path::Path::new(path);
    if relative.is_absolute() {
        anyhow::bail!("integration backup path must be relative: {}", path);
    }

    let normalized = normalized_relative_path(relative)?;
    let mut components = std::path::Path::new(&normalized).components();
    match components.next() {
        Some(std::path::Component::Normal(first)) if first == "matter" => {}
        _ => anyhow::bail!("unsupported integration backup path: {}", path),
    }
    Ok(std::path::PathBuf::from(normalized))
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

    fn load_scenes(&self) -> Result<Option<StoredScenes>> {
        let path = self.file_path("scenes.json");
        match self.read_json::<StoredScenes>("scenes.json") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load scenes {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(target: "sys", "No persisted scenes at {}", path.display());
                }
                Ok(None)
            }
        }
    }

    fn save_scenes(&self, scenes: &StoredScenes) -> Result<()> {
        let data = serde_json::to_string_pretty(scenes)?;
        self.write_atomic("scenes.json", data.as_bytes())
    }

    fn load_motion_timers(&self) -> Result<Option<StoredMotionTimers>> {
        let path = self.file_path("motion_timers.json");
        match self.read_json::<StoredMotionTimers>("motion_timers.json") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load motion timers {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted motion timers at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_motion_timers(&self, timers: &StoredMotionTimers) -> Result<()> {
        let data = serde_json::to_string_pretty(timers)?;
        self.write_atomic("motion_timers.json", data.as_bytes())
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

    fn clear_hub_registries(&self) -> Result<()> {
        self.clear_hub_registry_files()
    }

    fn load_integration_backup_files(
        &self,
        include_secrets: bool,
    ) -> Result<Vec<crate::bundle::BackupIntegrationFile>> {
        if !include_secrets {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        for dir in INTEGRATION_SUBDIRS {
            self.collect_integration_backup_files(std::path::Path::new(dir), &mut files)?;
        }
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    }

    fn restore_integration_backup_files(
        &self,
        files: &[crate::bundle::BackupIntegrationFile],
    ) -> Result<()> {
        let files = files
            .iter()
            .map(|file| validate_integration_backup_path(&file.path).map(|path| (path, file)))
            .collect::<Result<Vec<_>>>()?;

        self.clear_integration_state_dirs()?;

        for (relative_path, file) in files {
            let absolute_path = self.dir.join(relative_path);
            Self::write_file_atomic_path(&absolute_path, file.content.as_bytes())?;
        }

        Ok(())
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

    fn load_api_auth(&self) -> Result<Option<crate::auth::StoredApiAuth>> {
        let path = self.file_path("auth.json");
        match self.read_json::<crate::auth::StoredApiAuth>("auth.json") {
            Ok(auth) => Ok(Some(auth)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load API auth state {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(target: "sys", "No persisted API auth state at {}", path.display());
                }
                Ok(None)
            }
        }
    }

    fn save_api_auth(&self, auth: &crate::auth::StoredApiAuth) -> Result<()> {
        let json = serde_json::to_string_pretty(auth)?;
        self.write_atomic("auth.json", json.as_bytes())
    }

    fn clear_api_auth(&self) -> Result<()> {
        self.remove_if_exists("auth.json")
    }

    fn clear_factory_reset_state(&self) -> Result<()> {
        for name in [
            "rooms.json",
            "light_profiles.json",
            "location.json",
            "settings.json",
            "scenes.json",
            "motion_timers.json",
            "hub_credentials.json",
            "canonical_registry.json",
            "topology.json",
            "commissioning_wifi.json",
            "auth.json",
        ] {
            self.remove_if_exists(name)?;
        }
        self.clear_hub_registry_files()?;

        self.clear_integration_state_dirs()?;

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
    crate::logging::update_log_clock_from_location(
        s.timezone_name.as_deref(),
        s.utc_offset_hours,
        s.latitude.is_some()
            || s.longitude.is_some()
            || s.timezone_name.is_some()
            || s.utc_offset_hours.abs() > f32::EPSILON,
    );

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
                s.light_breaker_enabled = settings.light_breaker_enabled;
                s.auto_update = settings.auto_update;
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
                            light_breaker_enabled: s.light_breaker_enabled,
                            active_mode: s.active_mode,
                            last_active_mode_cause: s.last_active_mode_cause,
                            last_active_mode_transition_id: s
                                .last_active_mode_transition_id
                                .clone(),
                            last_active_mode_change_utc_ms,
                            modes: normalized_modes.clone(),
                            mode_transitions: normalized_transitions,
                            auto_update: s.auto_update,
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
                    "Loaded settings: active_mode={:?}, light_breaker_enabled={}, modes={}, transitions={}",
                    s.active_mode,
                    s.light_breaker_enabled,
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
        match storage.load_motion_timers() {
            Ok(Some(timers)) => {
                let count = timers.entries.len();
                s.motion_timer_restores = timers
                    .entries
                    .into_iter()
                    .map(|entry| (entry.source_node_id.clone(), entry))
                    .collect();
                info!(target: "sys", "Loaded motion timers: {} sources", count);
            }
            Ok(None) => {}
            Err(e) => {
                debug!(target: "sys", "No persisted motion timers loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_scenes() {
            Ok(Some(stored)) => {
                s.scenes.clear();
                for mut scene in stored.scenes {
                    scene.normalize();
                    s.scenes.insert(scene.id.clone(), scene);
                }
                info!(target: "sys", "Loaded scenes: {}", s.scenes.len());
            }
            Ok(None) => {}
            Err(e) => {
                debug!(target: "sys", "No persisted scenes loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_api_auth() {
            Ok(Some(auth)) => {
                let count = auth.tokens.len();
                let owner_configured = auth.has_owner();
                if let Some(require_api_auth) = auth.require_api_auth {
                    s.require_api_auth = require_api_auth;
                }
                s.api_auth = auth;
                info!(
                    target: "sys",
                    "Loaded API auth state: tokens={}, owner_configured={}, require_api_auth={}",
                    count,
                    owner_configured,
                    s.require_api_auth
                );
            }
            Ok(None) => {}
            Err(e) => {
                warn!(target: "sys", "Failed to load API auth state: {}", e);
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

    struct DefaultOnlyStorage;

    impl Storage for DefaultOnlyStorage {
        fn load_rooms(&self) -> Result<RoomManager> {
            Ok(RoomManager::new())
        }

        fn save_rooms(&self, _rooms: &RoomManager) -> Result<()> {
            Ok(())
        }

        fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
            Ok(StoredLightProfiles {
                solar_noon_hour: 12.0,
                profiles: Vec::new(),
            })
        }

        fn save_light_profiles(&self, _config: &StoredLightProfiles) -> Result<()> {
            Ok(())
        }

        fn load_location(&self) -> Result<StoredLocation> {
            Ok(StoredLocation {
                latitude: None,
                longitude: None,
                utc_offset_hours: 0.0,
                timezone_name: None,
            })
        }

        fn save_location(&self, _loc: &StoredLocation) -> Result<()> {
            Ok(())
        }

        fn load_settings(&self) -> Result<StoredSettings> {
            Ok(StoredSettings {
                power_save: false,
                light_breaker_enabled: true,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::default(),
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                auto_update: true,
            })
        }

        fn save_settings(&self, _settings: &StoredSettings) -> Result<()> {
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
    }

    #[test]
    fn storage_default_extension_methods_are_noops() {
        let storage = DefaultOnlyStorage;
        let auth = crate::auth::StoredApiAuth::default();
        let wifi = crate::provisioning::WifiCredentials {
            ssid: "Test".to_string(),
            password: "secret".to_string(),
        };

        assert_eq!(StoredMotionTimers::default().schema_version, 1);
        assert!(storage.load_scenes().unwrap().is_none());
        storage.save_scenes(&StoredScenes::default()).unwrap();
        assert!(storage.load_motion_timers().unwrap().is_none());
        storage
            .save_motion_timers(&StoredMotionTimers::default())
            .unwrap();
        storage.clear_hub_registries().unwrap();
        assert!(storage
            .load_integration_backup_files(true)
            .unwrap()
            .is_empty());
        storage.restore_integration_backup_files(&[]).unwrap();
        assert!(storage.load_canonical_registry().unwrap().is_none());
        storage
            .save_canonical_registry(&serde_json::json!({}))
            .unwrap();
        assert!(storage.load_topology().unwrap().is_none());
        storage.save_topology(&serde_json::json!({})).unwrap();
        assert!(storage
            .load_commissioning_wifi_credentials()
            .unwrap()
            .is_none());
        storage.save_commissioning_wifi_credentials(&wifi).unwrap();
        storage.clear_commissioning_wifi_credentials().unwrap();
        assert!(storage.load_api_auth().unwrap().is_none());
        storage.save_api_auth(&auth).unwrap();
        storage.clear_api_auth().unwrap();
        storage.clear_factory_reset_state().unwrap();
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
    fn stored_light_profiles_preserve_normalized_day_idle_constant_override() {
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
                        brightness: 1.0,
                        color_temp: 0.0,
                        direct_color: Some(rhythm_core::LightDirectColor {
                            xy: rhythm_core::XyColor {
                                x: 0.2041,
                                y: 0.2444,
                            },
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
        assert_eq!(config.min_brightness, 15);
        assert_eq!(config.max_brightness, 15);
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
                light_breaker_enabled: true,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
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
                light_breaker_enabled: true,
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
                auto_update: true,
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
                light_breaker_enabled: true,
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
                auto_update: true,
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
                light_breaker_enabled: true,
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
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
            room_id: None,
            room_name: None,
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
                light_breaker_enabled: true,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
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
        fn scenes_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let stored = crate::scenes::StoredScenes {
                schema_version: crate::scenes::LIGHT_SCENE_SCHEMA_VERSION,
                scenes: vec![crate::scenes::SceneDefinition {
                    id: "icy-glow".into(),
                    name: "Icy Glow".into(),
                    description: None,
                    source: crate::scenes::SceneSource::Imported {
                        provider: "hue".into(),
                        external_id: Some("icy_glow".into()),
                    },
                    light: None,
                    extensions: std::collections::BTreeMap::new(),
                }],
            };

            storage.save_scenes(&stored).unwrap();
            let loaded = storage.load_scenes().unwrap().unwrap();

            assert_eq!(
                loaded.schema_version,
                crate::scenes::LIGHT_SCENE_SCHEMA_VERSION
            );
            assert_eq!(loaded.scenes.len(), 1);
            assert_eq!(loaded.scenes[0].id, "icy-glow");
            assert_eq!(loaded.scenes[0].name, "Icy Glow");
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_loads_and_normalizes_scenes() {
            let (storage, path) = temp_storage();
            storage
                .save_scenes(&crate::scenes::StoredScenes {
                    schema_version: crate::scenes::LIGHT_SCENE_SCHEMA_VERSION,
                    scenes: vec![crate::scenes::SceneDefinition {
                        id: String::new(),
                        name: "Icy Glow".into(),
                        description: None,
                        source: crate::scenes::SceneSource::User,
                        light: None,
                        extensions: std::collections::BTreeMap::new(),
                    }],
                })
                .unwrap();

            let mut state = crate::state::AppState {
                storage: Some(Box::new(FileStorage::new(path.to_str().unwrap()).unwrap())),
                ..Default::default()
            };
            load_persisted_state(&mut state);

            assert!(state.scenes.contains_key("icy-glow"));
            assert_eq!(state.scenes["icy-glow"].name, "Icy Glow");
            cleanup(&path);
        }

        #[test]
        fn api_auth_save_load_roundtrip_preserves_policy_override() {
            let (storage, path) = temp_storage();
            let auth = crate::auth::StoredApiAuth {
                require_api_auth: Some(false),
                ..crate::auth::StoredApiAuth::default()
            };

            storage.save_api_auth(&auth).unwrap();
            let loaded = storage.load_api_auth().unwrap().unwrap();

            assert_eq!(loaded.require_api_auth, Some(false));
            assert_eq!(loaded.tokens.len(), 0);
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_applies_api_auth_policy_override() {
            let (storage, path) = temp_storage();
            storage
                .save_api_auth(&crate::auth::StoredApiAuth {
                    require_api_auth: Some(false),
                    ..crate::auth::StoredApiAuth::default()
                })
                .unwrap();

            let mut app = crate::state::AppState {
                require_api_auth: true,
                storage: Some(Box::new(FileStorage::new(path.to_str().unwrap()).unwrap())),
                ..Default::default()
            };

            load_persisted_state(&mut app);

            assert!(!app.require_api_auth);
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_keeps_platform_default_when_policy_missing() {
            let (storage, path) = temp_storage();
            storage
                .save_api_auth(&crate::auth::StoredApiAuth::default())
                .unwrap();

            let mut app = crate::state::AppState {
                require_api_auth: true,
                storage: Some(Box::new(FileStorage::new(path.to_str().unwrap()).unwrap())),
                ..Default::default()
            };

            load_persisted_state(&mut app);

            assert!(app.require_api_auth);
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
                light_breaker_enabled: false,
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Schedule,
                last_active_mode_transition_id: Some("sleep_to_day".into()),
                last_active_mode_change_utc_ms: Some(1_234_567_890),
                modes: rhythm_core::default_mode_configs(),
                mode_transitions: rhythm_core::default_mode_transition_configs(),
                auto_update: false,
            };
            storage.save_settings(&settings).unwrap();
            let loaded = storage.load_settings().unwrap();
            assert!(loaded.power_save);
            assert!(!loaded.light_breaker_enabled);
            assert!(!loaded.auto_update);
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
        fn settings_missing_auto_update_defaults_true() {
            let (storage, path) = temp_storage();
            let json = r#"{
              "power_save": false,
              "active_mode": "day",
              "modes": [],
              "mode_transitions": []
            }"#;
            std::fs::write(path.join("settings.json"), json).unwrap();

            let loaded = storage.load_settings().unwrap();

            assert!(!loaded.power_save);
            assert!(loaded.light_breaker_enabled);
            assert!(loaded.auto_update);
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
        fn motion_timers_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let timers = StoredMotionTimers {
                schema_version: 1,
                entries: vec![StoredMotionTimerEntry {
                    source_node_id: "sensor-1".into(),
                    target_node_id: "room-1".into(),
                    stopped_at_epoch_ms: Some(1_700_000_000_000),
                    motion_owned: true,
                    warning_active: false,
                }],
            };

            storage.save_motion_timers(&timers).unwrap();
            let loaded = storage.load_motion_timers().unwrap();
            assert_eq!(loaded, Some(timers));
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
                    light_breaker_enabled: true,
                    active_mode: RhythmMode::Sleep,
                    last_active_mode_cause: ModeChangeCause::Manual,
                    last_active_mode_transition_id: None,
                    last_active_mode_change_utc_ms: Some(123),
                    modes: rhythm_core::default_mode_configs(),
                    mode_transitions: rhythm_core::default_mode_transition_configs(),
                    auto_update: true,
                })
                .unwrap();
            storage
                .save_motion_timers(&StoredMotionTimers {
                    schema_version: 1,
                    entries: vec![StoredMotionTimerEntry {
                        source_node_id: "sensor-1".into(),
                        target_node_id: "room-1".into(),
                        stopped_at_epoch_ms: Some(123),
                        motion_owned: true,
                        warning_active: false,
                    }],
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
                    .join("chip_tool_config.controller-storage.ini"),
                "{}",
            )
            .unwrap();

            storage.clear_factory_reset_state().unwrap();

            for name in [
                "rooms.json",
                "light_profiles.json",
                "location.json",
                "settings.json",
                "motion_timers.json",
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
        fn integration_backup_files_require_secrets_and_skip_runtime_noise() {
            let (storage, path) = temp_storage();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::create_dir_all(path.join("matter").join("captures")).unwrap();
            std::fs::write(path.join("matter").join("fabric-identity.json"), "fabric").unwrap();
            std::fs::write(
                path.join("matter")
                    .join("chip")
                    .join("controller-storage.json"),
                "controller",
            )
            .unwrap();
            std::fs::write(
                path.join("matter").join("chip").join("devices.json"),
                "devices",
            )
            .unwrap();
            std::fs::write(path.join("matter").join("chip").join("write.tmp"), "tmp").unwrap();
            std::fs::write(path.join("matter").join("chip").join("sidecar.log"), "log").unwrap();
            std::fs::write(
                path.join("matter").join("captures").join("pairing.json"),
                "capture",
            )
            .unwrap();

            assert!(storage
                .load_integration_backup_files(false)
                .unwrap()
                .is_empty());

            let files = storage.load_integration_backup_files(true).unwrap();
            assert_eq!(
                files
                    .iter()
                    .map(|file| (file.path.as_str(), file.content.as_str(), file.secret))
                    .collect::<Vec<_>>(),
                vec![
                    ("matter/chip/controller-storage.json", "controller", true),
                    ("matter/chip/devices.json", "devices", true),
                    ("matter/fabric-identity.json", "fabric", true),
                ]
            );

            cleanup(&path);
        }

        #[test]
        fn restore_integration_backup_files_replaces_matter_state() {
            let (storage, path) = temp_storage();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::write(path.join("matter").join("stale.json"), "stale").unwrap();

            storage
                .restore_integration_backup_files(&[
                    crate::bundle::BackupIntegrationFile {
                        path: "matter/fabric-identity.json".to_string(),
                        content: "fabric".to_string(),
                        secret: true,
                    },
                    crate::bundle::BackupIntegrationFile {
                        path: "matter/chip/controller-storage.json".to_string(),
                        content: "controller".to_string(),
                        secret: true,
                    },
                ])
                .unwrap();

            assert!(!path.join("matter").join("stale.json").exists());
            assert_eq!(
                std::fs::read_to_string(path.join("matter").join("fabric-identity.json")).unwrap(),
                "fabric"
            );
            assert_eq!(
                std::fs::read_to_string(
                    path.join("matter")
                        .join("chip")
                        .join("controller-storage.json")
                )
                .unwrap(),
                "controller"
            );

            storage.restore_integration_backup_files(&[]).unwrap();
            assert!(!path.join("matter").exists());

            cleanup(&path);
        }

        #[test]
        fn restore_integration_backup_files_rejects_path_escape() {
            let (storage, path) = temp_storage();
            std::fs::create_dir_all(path.join("matter")).unwrap();
            std::fs::write(path.join("matter").join("existing.json"), "existing").unwrap();
            let err = storage
                .restore_integration_backup_files(&[crate::bundle::BackupIntegrationFile {
                    path: "../matter/fabric-identity.json".to_string(),
                    content: "fabric".to_string(),
                    secret: true,
                }])
                .unwrap_err();
            assert!(err.to_string().contains("invalid integration backup path"));
            assert_eq!(
                std::fs::read_to_string(path.join("matter").join("existing.json")).unwrap(),
                "existing"
            );
            cleanup(&path);
        }

        #[test]
        fn restore_integration_backup_files_rejects_empty_absolute_and_unsupported_paths() {
            let (storage, path) = temp_storage();
            for candidate in ["", "/matter/fabric.json", "hue/fabric.json"] {
                let error = storage
                    .restore_integration_backup_files(&[crate::bundle::BackupIntegrationFile {
                        path: candidate.to_string(),
                        content: "data".to_string(),
                        secret: true,
                    }])
                    .unwrap_err();
                assert!(
                    error.to_string().contains("integration backup path"),
                    "unexpected error for {candidate:?}: {error:#}"
                );
            }
            cleanup(&path);
        }

        #[test]
        fn optional_json_files_return_none_when_missing_or_corrupt() {
            let (storage, path) = temp_storage();
            let hub_key = HubKey::new(crate::hub::HubType::new("hue"), "192.168.1.60");

            assert!(storage.load_scenes().unwrap().is_none());
            std::fs::write(path.join("scenes.json"), "{").unwrap();
            assert!(storage.load_scenes().unwrap().is_none());

            assert!(storage.load_motion_timers().unwrap().is_none());
            std::fs::write(path.join("motion_timers.json"), "{").unwrap();
            assert!(storage.load_motion_timers().unwrap().is_none());

            assert!(storage.load_hub_registry_for(&hub_key).unwrap().is_none());
            std::fs::write(path.join("hub_registry_hue_192_168_1_60.json"), "{").unwrap();
            assert!(storage.load_hub_registry_for(&hub_key).unwrap().is_none());

            assert!(storage.load_canonical_registry().unwrap().is_none());
            std::fs::write(path.join("canonical_registry.json"), "{").unwrap();
            assert!(storage.load_canonical_registry().unwrap().is_none());

            assert!(storage.load_topology().unwrap().is_none());
            std::fs::write(path.join("topology.json"), "{").unwrap();
            assert!(storage.load_topology().unwrap().is_none());

            assert!(storage
                .load_commissioning_wifi_credentials()
                .unwrap()
                .is_none());
            std::fs::write(path.join("commissioning_wifi.json"), "{").unwrap();
            assert!(storage
                .load_commissioning_wifi_credentials()
                .unwrap()
                .is_none());
            storage.clear_commissioning_wifi_credentials().unwrap();
            storage.clear_commissioning_wifi_credentials().unwrap();

            assert!(storage.load_api_auth().unwrap().is_none());
            std::fs::write(path.join("auth.json"), "{").unwrap();
            assert!(storage.load_api_auth().unwrap().is_none());
            storage.clear_api_auth().unwrap();
            storage.clear_api_auth().unwrap();

            cleanup(&path);
        }

        #[test]
        fn rooms_save_load_roundtrip() {
            let (storage, path) = temp_storage();
            let mut rooms = rhythm_core::room::RoomManager::new();
            rooms.get_or_create("living-room", "Living Room");
            rooms.get_or_create("bedroom", "Bedroom");
            rooms.get_mut("living-room").unwrap().mood_active = true;
            rooms.get_mut("bedroom").unwrap().soft_off = true;
            rooms.get_mut("bedroom").unwrap().standby_enabled = true;
            assert_eq!(rooms.len(), 2);

            storage.save_rooms(&rooms).unwrap();
            let loaded = storage.load_rooms().unwrap();
            assert_eq!(loaded.len(), 2);
            assert!(loaded.get("living-room").is_some());
            assert_eq!(loaded.get("living-room").unwrap().name, "Living Room");
            assert!(loaded.get("living-room").unwrap().mood_active);
            assert!(!loaded.get("living-room").unwrap().soft_off);
            assert!(loaded.get("bedroom").is_some());
            assert_eq!(loaded.get("bedroom").unwrap().name, "Bedroom");
            assert!(loaded.get("bedroom").unwrap().soft_off);
            assert!(loaded.get("bedroom").unwrap().standby_enabled);
            assert!(!loaded.get("bedroom").unwrap().mood_active);
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

        // ---- Durability / atomic-write tests ----

        fn sample_settings() -> StoredSettings {
            StoredSettings {
                power_save: false,
                light_breaker_enabled: true,
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::default(),
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                auto_update: true,
            }
        }

        #[test]
        fn save_cleans_up_tmp_file_on_success() {
            let (storage, path) = temp_storage();
            let settings = sample_settings();
            storage.save_settings(&settings).unwrap();

            assert!(path.join("settings.json").exists());
            assert!(
                !path.join("settings.json.tmp").exists(),
                "write_atomic must leave no .tmp sibling on success"
            );
            cleanup(&path);
        }

        #[test]
        fn save_reclaims_orphan_tmp_from_prior_crash() {
            // Simulate a crashed prior write by leaving a stale .tmp file
            // behind before the next save.
            let (storage, path) = temp_storage();
            std::fs::write(path.join("settings.json.tmp"), "partial garbage").unwrap();

            let settings = sample_settings();
            storage.save_settings(&settings).unwrap();

            assert!(path.join("settings.json").exists());
            assert!(
                !path.join("settings.json.tmp").exists(),
                "stale .tmp from a prior crashed write must be cleaned up"
            );

            // And loading must succeed with the fresh contents, not the stale tmp.
            let _loaded = storage.load_settings().unwrap();
            cleanup(&path);
        }

        #[test]
        fn partial_write_between_profiles_and_settings_keeps_profiles_loadable() {
            // Simulate the production crash window between save_light_profiles
            // and save_settings: profiles successfully persisted, settings call
            // never ran. Restoring must load profiles and fall back to default
            // settings without losing user data.
            let (storage, path) = temp_storage();

            let mut profile = rhythm_core::default_rhythm_profile();
            profile.min_brightness = 7;
            profile.max_brightness = 91;
            let profiles = StoredLightProfiles {
                solar_noon_hour: 12.5,
                profiles: vec![profile],
            };
            storage.save_light_profiles(&profiles).unwrap();
            // settings.save never called — simulates crash between writes.

            let reopened = FileStorage::new(path.to_str().unwrap()).unwrap();
            let loaded_profiles = reopened.load_light_profiles().unwrap();
            assert_eq!(loaded_profiles.profiles[0].min_brightness, 7);
            assert_eq!(loaded_profiles.profiles[0].max_brightness, 91);

            // No settings file → load_settings errors. Callers should treat
            // this as "use defaults" rather than crashing; assert the error
            // type is recoverable rather than a data-loss signal.
            let settings_err = reopened.load_settings();
            assert!(settings_err.is_err());

            cleanup(&path);
        }

        #[test]
        fn load_returns_error_on_truncated_json() {
            let (storage, path) = temp_storage();
            // Write a truncated JSON doc directly, simulating power loss
            // mid-write on a legacy non-atomic storage backend.
            std::fs::write(path.join("light_profiles.json"), "{\"profiles\":[").unwrap();

            let result = storage.load_light_profiles();
            assert!(
                result.is_err(),
                "truncated JSON must surface as a parse error, not panic"
            );
            cleanup(&path);
        }

        #[test]
        fn load_returns_error_on_empty_file() {
            let (storage, path) = temp_storage();
            std::fs::write(path.join("settings.json"), "").unwrap();

            let result = storage.load_settings();
            assert!(result.is_err(), "empty file must not deserialize silently");
            cleanup(&path);
        }

        #[test]
        fn successive_saves_do_not_accumulate_tmp_siblings() {
            let (storage, path) = temp_storage();
            for _ in 0..8 {
                storage.save_settings(&sample_settings()).unwrap();
            }
            let orphan_tmps: Vec<_> = std::fs::read_dir(&path)
                .unwrap()
                .filter_map(|entry| entry.ok())
                .filter(|entry| {
                    entry
                        .file_name()
                        .to_str()
                        .map(|n| n.ends_with(".tmp"))
                        .unwrap_or(false)
                })
                .collect();
            assert!(
                orphan_tmps.is_empty(),
                "no .tmp siblings should be left after repeated saves, found {} orphan(s)",
                orphan_tmps.len()
            );
            cleanup(&path);
        }
    }
}
