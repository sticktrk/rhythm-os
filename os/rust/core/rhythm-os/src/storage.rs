//! Abstract persistence interface for Rhythm OS.
//!
//! Platform crates implement this trait to provide concrete storage
//! (e.g., filesystem on Linux, SQLite on Raspberry Pi, or future custom backends).

use anyhow::Context;
use anyhow::Result;
use log::{debug, info, warn};
use rand::RngCore;
use rhythm_core::room::RoomManager;
use rhythm_core::RuntimeConfig;
use rhythm_core::{
    LightProfileConfig, ModeChangeCause, ModeConfig, ModeTransitionConfig, RhythmMode,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::canonical::identity::HubKey;
use crate::hub::HubCredentials;
use crate::scenes::{is_native_scene_id, StoredScenes};

const MAX_PAIRING_HISTORY_BYTES: u64 = 1024 * 1024;

/// Schema for the crash-atomic canonical-registry + topology authority state.
pub const STORED_AUTHORITY_STATE_SCHEMA_VERSION: u32 = 1;

/// One commit record for the two structures that jointly define Rhythm's
/// authoritative device placement and external-controller room projection.
///
/// The individual legacy files remain mirrored for downgrade compatibility,
/// but current runtimes load this snapshot first so a crash can never expose
/// one half of a room mutation without the other.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct StoredAuthorityState {
    pub schema_version: u32,
    pub canonical_registry: Value,
    pub topology: Value,
}

impl StoredAuthorityState {
    pub fn new(canonical_registry: Value, topology: Value) -> Self {
        Self {
            schema_version: STORED_AUTHORITY_STATE_SCHEMA_VERSION,
            canonical_registry,
            topology,
        }
    }
}

// This mirrors the serialized boundary owned by rhythm-hue without making the
// core storage crate depend on an integration crate. Deserializing the complete
// shape keeps destructive backup preflight aligned with the payload that Hue
// will consume after the current controller has been released.
#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum HueOwnershipValidationPhase {
    Captured,
    Clearing,
    ClearIncomplete,
    Active,
    Restoring,
    RestoreIncomplete,
    Restored,
    ReleasePending,
    SnapshotRetained,
}

#[allow(dead_code)]
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum HueOwnershipReceiptValidationStatus {
    Pending,
    Succeeded,
    Failed,
    Unsupported,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct HueOwnershipReceiptValidation {
    operation_id: String,
    api: String,
    action: String,
    resource_type: String,
    original_resource_id: String,
    #[serde(default)]
    replacement_resource_id: Option<String>,
    status: HueOwnershipReceiptValidationStatus,
    attempt: u32,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct HueManagedRoomValidation {
    rhythm_room_id: String,
    hue_room_id: String,
    grouped_light_id: String,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct HueManagedSceneValidation {
    rhythm_room_id: String,
    rhythm_scene_id: String,
    hue_room_id: String,
    hue_scene_id: String,
    fingerprint: String,
    #[serde(default)]
    ephemeral: bool,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct HueOwnershipBaselineValidation {
    schema_version: u32,
    capture_id: String,
    bridge_id: String,
    v2_resources: BTreeMap<String, Value>,
    v1_resources: BTreeMap<String, Value>,
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct HueOwnershipManifestValidation {
    schema_version: u32,
    phase: HueOwnershipValidationPhase,
    baseline: HueOwnershipBaselineValidation,
    #[serde(default)]
    managed_rooms: BTreeMap<String, HueManagedRoomValidation>,
    #[serde(default)]
    managed_scenes: BTreeMap<String, HueManagedSceneValidation>,
    #[serde(default)]
    restored_resource_ids: BTreeMap<String, String>,
    #[serde(default)]
    receipts: BTreeMap<String, HueOwnershipReceiptValidation>,
}

/// Namespaced durable state for plan-based light runtimes.
///
/// Shape: runtime id -> node id -> app-defined key -> JSON value.
pub type StoredLightRuntimeState = BTreeMap<String, BTreeMap<String, BTreeMap<String, Value>>>;

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
    fn load_light_runtime_state(&self) -> Result<Option<StoredLightRuntimeState>> {
        Ok(None)
    }
    fn save_light_runtime_state(&self, _state: &StoredLightRuntimeState) -> Result<()> {
        Ok(())
    }
    fn load_light_activity_history(&self) -> Result<Option<crate::activity::LightActivityHistory>> {
        Ok(None)
    }
    fn save_light_activity_history(
        &self,
        _history: &crate::activity::LightActivityHistory,
    ) -> Result<()> {
        Ok(())
    }
    fn load_light_usage_ledger(&self) -> Result<Option<crate::light_usage::LightUsageLedger>> {
        Ok(None)
    }
    fn save_light_usage_ledger(
        &self,
        _ledger: &crate::light_usage::LightUsageLedger,
    ) -> Result<()> {
        Ok(())
    }
    fn clear_light_usage_ledger(&self) -> Result<()> {
        Ok(())
    }
    fn load_activity_cloud_config(
        &self,
    ) -> Result<Option<crate::activity_cloud::StoredActivityCloudConfig>> {
        Ok(None)
    }
    fn save_activity_cloud_config(
        &self,
        _config: &crate::activity_cloud::StoredActivityCloudConfig,
    ) -> Result<()> {
        Ok(())
    }
    fn load_pairing_history(&self) -> Result<Option<crate::pairing::PairingHistory>> {
        Ok(None)
    }
    fn save_pairing_history(&self, _history: &crate::pairing::PairingHistory) -> Result<()> {
        Ok(())
    }
    fn clear_activity_cloud_config(&self) -> Result<()> {
        Ok(())
    }
    fn load_all_hub_credentials(&self) -> Result<Vec<HubCredentials>>;
    fn save_all_hub_credentials(&self, creds: &[HubCredentials]) -> Result<()>;
    fn load_hub_registry_for(&self, key: &HubKey) -> Result<Option<Value>>;
    fn save_hub_registry_for(&self, key: &HubKey, data: &Value) -> Result<()>;
    fn clear_hub_registry_for(&self, _key: &HubKey) -> Result<()> {
        Ok(())
    }
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

    /// Validate integration backup paths and payload shape without changing
    /// current installation state. Destructive restore workflows call this
    /// before releasing external controllers or deleting credentials.
    fn validate_integration_backup_files(
        &self,
        _files: &[crate::bundle::BackupIntegrationFile],
    ) -> Result<()> {
        Ok(())
    }

    /// Load one integration-owned secret state file.
    ///
    /// Paths are relative to the data directory and must belong to a
    /// registered integration backup directory (for example
    /// `hue/controller-ownership.json`). Implementations must reject path
    /// traversal. The default is intentionally empty for in-memory and test
    /// storage implementations that do not persist integration state.
    fn load_integration_state_file(&self, _path: &str) -> Result<Option<String>> {
        Ok(None)
    }

    /// Durably replace one integration-owned secret state file.
    ///
    /// A successful return is the durability boundary integrations may rely
    /// on before mutating an external controller.
    fn save_integration_state_file(&self, _path: &str, _content: &str) -> Result<()> {
        Ok(())
    }

    /// Durably remove one integration-owned secret state file.
    fn delete_integration_state_file(&self, _path: &str) -> Result<()> {
        Ok(())
    }

    /// Load the independently-written authority files used before the
    /// combined commit record existed.
    ///
    /// Production file storage overrides this so an existing malformed file
    /// remains distinguishable from a genuinely absent half during migration.
    fn load_legacy_authority_state(&self) -> Result<(Option<Value>, Option<Value>)> {
        Ok((self.load_canonical_registry()?, self.load_topology()?))
    }

    /// Load the coupled authority-state commit record.
    ///
    /// The compatibility default synthesizes a snapshot only when both legacy
    /// halves exist. File-backed production storage overrides this with the
    /// single crash-atomic commit file.
    fn load_authority_state(&self) -> Result<Option<StoredAuthorityState>> {
        let (Some(canonical_registry), Some(topology)) =
            (self.load_canonical_registry()?, self.load_topology()?)
        else {
            return Ok(None);
        };
        Ok(Some(StoredAuthorityState::new(
            canonical_registry,
            topology,
        )))
    }

    /// Durably commit canonical registry and topology as one authority state.
    ///
    /// Non-file test/platform implementations retain compatibility through
    /// their legacy methods. Production `FileStorage` overrides this with one
    /// atomic source-of-truth file plus legacy mirrors.
    fn save_authority_state(&self, state: &StoredAuthorityState) -> Result<()> {
        self.save_canonical_registry(&state.canonical_registry)?;
        self.save_topology(&state.topology)
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

    /// Load stable server metadata. Default: no configured identity.
    fn load_server_metadata(&self) -> Result<Option<StoredServerMetadata>> {
        Ok(None)
    }

    /// Persist stable server metadata. Default: no-op.
    fn save_server_metadata(&self, _metadata: &StoredServerMetadata) -> Result<()> {
        Ok(())
    }

    /// Clear stable server metadata. Default: no-op.
    fn clear_server_metadata(&self) -> Result<()> {
        Ok(())
    }

    /// Load private pairing metadata. Default: no configured pairing key.
    fn load_pairing_metadata(&self) -> Result<Option<StoredPairingMetadata>> {
        Ok(None)
    }

    /// Persist private pairing metadata. Default: no-op.
    fn save_pairing_metadata(&self, _metadata: &StoredPairingMetadata) -> Result<()> {
        Ok(())
    }

    /// Clear private pairing metadata. Default: no-op.
    fn clear_pairing_metadata(&self) -> Result<()> {
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

    /// Load remote access tunnel configuration. Default: no configured tunnel.
    fn load_remote_access_config(
        &self,
    ) -> Result<Option<crate::remote_access::StoredRemoteAccessConfig>> {
        Ok(None)
    }

    /// Persist remote access tunnel configuration. Default: no-op.
    fn save_remote_access_config(
        &self,
        _config: &crate::remote_access::StoredRemoteAccessConfig,
    ) -> Result<()> {
        Ok(())
    }

    /// Clear remote access tunnel configuration. Default: no-op.
    fn clear_remote_access_config(&self) -> Result<()> {
        Ok(())
    }

    /// Clear all persisted state that should not survive a full factory reset.
    ///
    /// Active desktop/server platforms use this to remove stale keyed hub
    /// registries and any other persisted artifacts that would otherwise be
    /// resurrected after a reset.
    fn clear_factory_reset_state(&self) -> Result<()> {
        self.clear_commissioning_wifi_credentials()?;
        self.clear_api_auth()?;
        self.clear_activity_cloud_config()?;
        self.clear_light_usage_ledger()?;
        self.clear_remote_access_config()
    }
}

const STORED_SERVER_METADATA_SCHEMA_VERSION: u32 = 2;

fn stored_server_metadata_schema_version() -> u32 {
    STORED_SERVER_METADATA_SCHEMA_VERSION
}

/// Stable server-installation metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredServerMetadata {
    #[serde(default = "stored_server_metadata_schema_version")]
    pub schema_version: u32,
    pub server_instance_id: String,
    /// Read compatibility for prerelease v2 metadata. New writes always omit
    /// this field so rollback-sensitive secrets live in a file an older binary
    /// never reads or rewrites.
    #[serde(default, rename = "pairing_hmac_key", skip_serializing)]
    legacy_pairing_hmac_key: Option<String>,
}

const STORED_PAIRING_METADATA_SCHEMA_VERSION: u32 = 1;

fn stored_pairing_metadata_schema_version() -> u32 {
    STORED_PAIRING_METADATA_SCHEMA_VERSION
}

/// Installation-secret metadata for privacy-safe pairing idempotency.
///
/// Kept separate from the stable public server identity so a rollback scrub
/// can rotate the pairing domain without changing `server_instance_id`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StoredPairingMetadata {
    #[serde(default = "stored_pairing_metadata_schema_version")]
    pub schema_version: u32,
    pub pairing_hmac_key: String,
}

/// Generate an opaque stable server-installation identifier.
pub fn generate_server_instance_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);

    let mut id = String::with_capacity(36);
    id.push_str("srv-");
    for byte in bytes {
        write!(&mut id, "{byte:02x}").expect("writing to String cannot fail");
    }
    id
}

/// Generate a 256-bit installation secret encoded as lowercase hex.
pub fn generate_pairing_hmac_key() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut key = String::with_capacity(64);
    for byte in bytes {
        write!(&mut key, "{byte:02x}").expect("writing to String cannot fail");
    }
    key
}

fn valid_pairing_hmac_key(key: &str) -> bool {
    key.len() == 64
        && key
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
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
            normalized.normalize_float_precision();
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
    #[serde(default)]
    pub light_runtime: crate::light_runtime::LightRuntimeKind,
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
    /// When true, the appliance applies OTA updates automatically during the
    /// daily update window. When false, updates only happen on an explicit
    /// `POST /api/ota/update`.
    #[serde(default = "default_auto_update")]
    pub auto_update: bool,
    /// Explicit OTA release channel. Deliberately not seeded from
    /// `auto_update`: absent stays `None`, which resolves to stable on
    /// appliances even for legacy `auto_update=false` devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub update_channel: Option<crate::state::UpdateChannel>,
}

fn default_auto_update() -> bool {
    true
}

fn default_light_breaker_enabled() -> bool {
    false
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
    write_lock: std::sync::Mutex<()>,
}

/// Integration state that is portable only together with its secret runtime
/// credentials and can therefore participate in full-secret backups.
const BACKUP_INTEGRATION_SUBDIRS: &[&str] = &["matter", "hue"];
/// Local integration state that must be removed by a full factory reset.
///
/// Appliance-local BLE metadata is deliberately not portable and is erased
/// together with the other integration state during factory reset.
const FACTORY_RESET_INTEGRATION_SUBDIRS: &[&str] = &[
    "matter",
    "hue",
    "hue_ble",
    "local_ble",
    // Securely erase state written by prerelease builds of the superseded
    // vendor-shaped implementation. It is reset-only and never migrated.
    "aidot_ble",
];

const LOCAL_BLE_ROLLBACK_DIRS: &[&str] = &["local_ble", "aidot_ble"];
const LOCAL_BLE_ROLLBACK_FILES: &[&str] = &[
    "pairing_history.json",
    "pairing_history.json.tmp",
    "pairing_metadata.json",
    "pairing_metadata.json.tmp",
    // A crashed prerelease combined-metadata write can leave the HMAC here.
    // The authoritative public identity file itself is deliberately retained.
    "server_metadata.json.tmp",
];

/// Durably erase state that a pre-local-BLE binary cannot safely own.
///
/// Supported appliance rollback paths call this before handing control to an
/// older binary. The stable public `server_metadata.json` is deliberately not
/// touched, so the server installation identity survives a rollback.
pub fn scrub_local_ble_rollback_state(data_dir: &std::path::Path) -> Result<()> {
    validate_rollback_data_dir(data_dir, "local BLE rollback scrub")?;

    for name in LOCAL_BLE_ROLLBACK_DIRS {
        remove_rollback_path(&data_dir.join(name))?;
    }
    for name in LOCAL_BLE_ROLLBACK_FILES {
        remove_rollback_path(&data_dir.join(name))?;
    }

    sync_rollback_data_dir(data_dir)
}

fn validate_rollback_data_dir(data_dir: &std::path::Path, operation: &str) -> Result<()> {
    use std::path::Component;

    if !data_dir.is_absolute()
        || data_dir.parent().is_none()
        || data_dir
            .components()
            .any(|component| matches!(component, Component::ParentDir))
    {
        anyhow::bail!(
            "{} requires a safe absolute data directory, got {}",
            operation,
            data_dir.display()
        );
    }

    let metadata = std::fs::symlink_metadata(data_dir)
        .with_context(|| format!("inspecting rollback data dir {}", data_dir.display()))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        anyhow::bail!(
            "{} data path must be a real directory: {}",
            operation,
            data_dir.display()
        );
    }

    Ok(())
}

fn sync_rollback_data_dir(data_dir: &std::path::Path) -> Result<()> {
    let directory = std::fs::File::open(data_dir)
        .with_context(|| format!("opening rollback data dir {}", data_dir.display()))?;
    directory
        .sync_all()
        .with_context(|| format!("fsyncing rollback data dir {}", data_dir.display()))
}

/// Prepare current authority state for a binary that only understands the
/// legacy registry and topology files.
///
/// Hue restore necessarily allocates replacement room, zone, and scene IDs.
/// Retire every Hue-native room route and registry cache before removing the
/// combined commit record so an older binary can only rediscover those
/// replacement resources. The combined file is removed last: before that
/// durable boundary current binaries continue to trust the complete snapshot;
/// afterwards older binaries see a matching pair of sanitized legacy files.
pub fn prepare_authority_state_for_binary_rollback(
    data_dir: &std::path::Path,
    configured_hue_hub_keys: &[HubKey],
) -> Result<()> {
    validate_rollback_data_dir(data_dir, "authority rollback handoff")?;
    let data_dir_str = data_dir
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("Rhythm data directory is not UTF-8"))?;
    let storage = FileStorage::new(data_dir_str)?;

    let mut hue_hub_keys = std::collections::BTreeMap::<String, HubKey>::new();
    for key in configured_hue_hub_keys
        .iter()
        .filter(|key| key.hub_type.as_str() == crate::hub::HubType::HUE)
    {
        hue_hub_keys.insert(key.to_string(), key.clone());
    }

    let Some(stored) = storage.load_authority_state()? else {
        // A retry after the commit-point removal must not reconstruct mirrors
        // from defaults or overwrite changes made by the older binary.
        for key in hue_hub_keys.values() {
            storage.clear_hub_registry_for(key)?;
        }
        return sync_rollback_data_dir(data_dir);
    };

    let (canonical_registry, mut topology) = decode_authority_state(stored)
        .context("Refusing binary rollback because authority state is invalid")?;
    for key in
        topology
            .referenced_hub_keys()
            .into_iter()
            .chain(canonical_registry.devices().flat_map(|device| {
                device
                    .endpoints
                    .iter()
                    .map(|endpoint| endpoint.hub_key.clone())
            }))
    {
        if key.hub_type.as_str() == crate::hub::HubType::HUE {
            hue_hub_keys.insert(key.to_string(), key);
        }
    }

    for key in hue_hub_keys.values() {
        topology.remove_room_bindings_for_hub_where(key, |_| true);
        topology.set_grouped_room_control_required(key, false);
    }
    topology.rebuild_indices();

    let canonical_registry = serde_json::to_value(canonical_registry)
        .context("Failed to serialize rollback canonical registry")?;
    let topology =
        serde_json::to_value(topology).context("Failed to serialize rollback topology")?;
    let canonical_mirror = serde_json::to_string_pretty(&canonical_registry)?;
    let topology_mirror = serde_json::to_string_pretty(&topology)?;

    storage.write_atomic_durable("canonical_registry.json", canonical_mirror.as_bytes())?;
    storage.write_atomic_durable("topology.json", topology_mirror.as_bytes())?;
    for key in hue_hub_keys.values() {
        storage.clear_hub_registry_for(key)?;
    }

    // This is the downgrade commit point. Never remove it before both mirrors
    // and every stale Hue discovery cache are durably retired.
    storage.remove_if_exists_durable("authority_state.json")?;
    Ok(())
}

fn remove_rollback_path(path: &std::path::Path) -> Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            return Err(anyhow::anyhow!(error))
                .with_context(|| format!("inspecting rollback state {}", path.display()));
        }
    };

    let result = if metadata.is_dir() && !metadata.file_type().is_symlink() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    };
    match result {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(anyhow::anyhow!(error))
            .with_context(|| format!("removing rollback state {}", path.display())),
    }
}

impl FileStorage {
    /// Create a new `FileStorage` rooted at `dir`.
    ///
    /// Creates the directory if it doesn't exist.
    pub fn new(dir: &str) -> Result<Self> {
        let dir = std::path::PathBuf::from(dir);
        std::fs::create_dir_all(&dir)
            .with_context(|| format!("Failed to create data dir: {}", dir.display()))?;
        Ok(Self {
            dir,
            write_lock: std::sync::Mutex::new(()),
        })
    }

    fn file_path(&self, name: &str) -> std::path::PathBuf {
        self.dir.join(name)
    }

    fn sync_directory_chain(&self, start: &std::path::Path, required: bool) -> Result<()> {
        if !start.starts_with(&self.dir) {
            anyhow::bail!("storage directory sync escaped the data root");
        }
        let stop = self.dir.parent().unwrap_or(&self.dir);
        let mut current = Some(start);
        while let Some(directory) = current {
            let result = std::fs::File::open(directory)
                .with_context(|| format!("Failed to open data dir {}", directory.display()))
                .and_then(|dir| {
                    dir.sync_all().with_context(|| {
                        format!("Failed to fsync data dir {}", directory.display())
                    })
                });
            if required {
                result?;
            }
            if directory == stop {
                break;
            }
            current = directory.parent();
        }
        Ok(())
    }

    /// Durable atomic write: write to `.tmp`, fsync the data, atomically rename
    /// into place, then fsync the parent directory so the rename survives a
    /// power loss. Removes any stale `.tmp` left behind by a crashed prior
    /// write before starting.
    fn write_atomic(&self, name: &str, data: &[u8]) -> Result<()> {
        self.write_atomic_with_mode(name, data, None, false)
    }

    fn write_atomic_durable(&self, name: &str, data: &[u8]) -> Result<()> {
        self.write_atomic_with_mode(name, data, None, true)
    }

    fn write_atomic_secret(&self, name: &str, data: &[u8]) -> Result<()> {
        self.write_atomic_with_mode(name, data, Some(0o600), false)
    }

    fn write_atomic_secret_durable(&self, name: &str, data: &[u8]) -> Result<()> {
        self.write_atomic_with_mode(name, data, Some(0o600), true)
    }

    fn write_atomic_with_mode(
        &self,
        name: &str,
        data: &[u8],
        unix_mode: Option<u32>,
        require_parent_sync: bool,
    ) -> Result<()> {
        use std::io::Write;

        // All atomic writes on a FileStorage instance share predictable `.tmp`
        // names. Serialize them so concurrent activity updates cannot remove or
        // rename another writer's temporary file.
        let _write_guard = self
            .write_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("File storage write lock is poisoned"))?;

        let path = self.file_path(name);
        let tmp = self.file_path(&format!("{}.tmp", name));
        let parent = path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("storage path has no parent"))?;
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;

        // A prior crashed write can leave a stale `.tmp`. Remove it so File::create
        // below doesn't silently inherit partial contents on platforms that
        // don't truncate on open.
        if tmp.exists() {
            let _ = std::fs::remove_file(&tmp);
        }

        {
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            if let Some(mode) = unix_mode {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(mode);
            }
            #[cfg(not(unix))]
            let _ = unix_mode;

            let mut file = options
                .open(&tmp)
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

        self.sync_directory_chain(parent, require_parent_sync)?;

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

    fn remove_if_exists_durable(&self, name: &str) -> Result<()> {
        let path = self.file_path(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {
                let parent = path
                    .parent()
                    .ok_or_else(|| anyhow::anyhow!("storage path has no parent"))?;
                let dir = std::fs::File::open(parent)
                    .with_context(|| format!("opening data dir {}", parent.display()))?;
                dir.sync_all()
                    .with_context(|| format!("fsyncing data dir {}", parent.display()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(anyhow::anyhow!(error)).with_context(|| format!("removing {}", path.display()))
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

    fn clear_integration_state_dirs(&self, directories: &[&str]) -> Result<()> {
        for dir in directories {
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

    fn write_file_atomic_path(&self, path: &std::path::Path, data: &[u8]) -> Result<()> {
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
            let mut options = std::fs::OpenOptions::new();
            options.create(true).truncate(true).write(true);
            #[cfg(unix)]
            {
                use std::os::unix::fs::OpenOptionsExt;
                options.mode(0o600);
            }
            let mut file = options
                .open(&tmp)
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
        self.sync_directory_chain(parent, true)?;
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
        Some(std::path::Component::Normal(first))
            if BACKUP_INTEGRATION_SUBDIRS
                .iter()
                .any(|supported| first == std::ffi::OsStr::new(supported)) => {}
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

    fn load_light_runtime_state(&self) -> Result<Option<StoredLightRuntimeState>> {
        let path = self.file_path("light_runtime_state.json");
        match self.read_json::<StoredLightRuntimeState>("light_runtime_state.json") {
            Ok(v) => Ok(Some(v)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load light runtime state {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    let legacy_path = self.file_path("app_runtime_state.json");
                    match self.read_json::<StoredLightRuntimeState>("app_runtime_state.json") {
                        Ok(v) => {
                            info!(
                                target: "sys",
                                "Loaded legacy runtime state from {}; future writes use light_runtime_state.json",
                                legacy_path.display()
                            );
                            return Ok(Some(v));
                        }
                        Err(legacy_error) => {
                            if legacy_path.exists() {
                                warn!(
                                    target: "sys",
                                    "Failed to load legacy runtime state {}: {}",
                                    legacy_path.display(),
                                    legacy_error
                                );
                            } else {
                                debug!(
                                    target: "sys",
                                    "No persisted light runtime state at {}",
                                    path.display()
                                );
                            }
                        }
                    }
                }
                Ok(None)
            }
        }
    }

    fn save_light_runtime_state(&self, state: &StoredLightRuntimeState) -> Result<()> {
        let data = serde_json::to_string_pretty(state)?;
        self.write_atomic("light_runtime_state.json", data.as_bytes())
    }

    fn load_light_activity_history(&self) -> Result<Option<crate::activity::LightActivityHistory>> {
        let path = self.file_path("activity_history.json");
        match self.read_json::<crate::activity::LightActivityHistory>("activity_history.json") {
            Ok(v) => Ok(Some(v.normalized())),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load activity history {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted activity history at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_light_activity_history(
        &self,
        history: &crate::activity::LightActivityHistory,
    ) -> Result<()> {
        let data = serde_json::to_string_pretty(history)?;
        self.write_atomic("activity_history.json", data.as_bytes())
    }

    fn load_light_usage_ledger(&self) -> Result<Option<crate::light_usage::LightUsageLedger>> {
        let path = self.file_path("light_usage_ledger.json");
        if !path.exists() {
            return Ok(None);
        }
        let bytes = std::fs::metadata(&path)
            .with_context(|| format!("Failed to inspect {}", path.display()))?
            .len();
        if bytes > crate::light_usage::LIGHT_USAGE_LEDGER_BYTES_LIMIT {
            anyhow::bail!(
                "light usage ledger exceeds the {} byte limit at {}",
                crate::light_usage::LIGHT_USAGE_LEDGER_BYTES_LIMIT,
                path.display()
            );
        }
        self.read_json::<crate::light_usage::LightUsageLedger>("light_usage_ledger.json")
            .map(Some)
            .with_context(|| format!("Failed to load light usage ledger at {}", path.display()))
    }

    fn save_light_usage_ledger(&self, ledger: &crate::light_usage::LightUsageLedger) -> Result<()> {
        let data = serde_json::to_vec_pretty(ledger)?;
        if data.len() as u64 > crate::light_usage::LIGHT_USAGE_LEDGER_BYTES_LIMIT {
            anyhow::bail!(
                "light usage ledger would exceed the {} byte limit",
                crate::light_usage::LIGHT_USAGE_LEDGER_BYTES_LIMIT
            );
        }
        self.write_atomic_durable("light_usage_ledger.json", &data)
    }

    fn clear_light_usage_ledger(&self) -> Result<()> {
        self.remove_if_exists_durable("light_usage_ledger.json")
    }

    fn load_pairing_history(&self) -> Result<Option<crate::pairing::PairingHistory>> {
        let path = self.file_path("pairing_history.json");
        if path.exists()
            && std::fs::metadata(&path)
                .with_context(|| format!("Failed to inspect {}", path.display()))?
                .len()
                > MAX_PAIRING_HISTORY_BYTES
        {
            anyhow::bail!(
                "pairing history exceeds the {} byte limit at {}",
                MAX_PAIRING_HISTORY_BYTES,
                path.display()
            );
        }
        match self.read_json::<crate::pairing::PairingHistory>("pairing_history.json") {
            Ok(history) => Ok(Some(history)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load pairing history {}: {}",
                        path.display(),
                        e
                    );
                    return Err(e).with_context(|| {
                        format!(
                            "pairing history exists but is unreadable at {}",
                            path.display()
                        )
                    });
                }
                Ok(None)
            }
        }
    }

    fn save_pairing_history(&self, history: &crate::pairing::PairingHistory) -> Result<()> {
        let data = serde_json::to_string_pretty(history)?;
        if data.len() as u64 > MAX_PAIRING_HISTORY_BYTES {
            anyhow::bail!(
                "pairing history would exceed the {} byte limit",
                MAX_PAIRING_HISTORY_BYTES
            );
        }
        self.write_atomic_durable("pairing_history.json", data.as_bytes())
    }

    fn load_activity_cloud_config(
        &self,
    ) -> Result<Option<crate::activity_cloud::StoredActivityCloudConfig>> {
        let path = self.file_path("activity_cloud.json");
        match self
            .read_json::<crate::activity_cloud::StoredActivityCloudConfig>("activity_cloud.json")
        {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load activity cloud config {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted activity cloud config at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_activity_cloud_config(
        &self,
        config: &crate::activity_cloud::StoredActivityCloudConfig,
    ) -> Result<()> {
        let data = serde_json::to_string_pretty(config)?;
        self.write_atomic("activity_cloud.json", data.as_bytes())
    }

    fn clear_activity_cloud_config(&self) -> Result<()> {
        self.remove_if_exists("activity_cloud.json")
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
        self.write_atomic_secret_durable("hub_credentials.json", data.as_bytes())
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

    fn clear_hub_registry_for(&self, key: &HubKey) -> Result<()> {
        let filename = format!("hub_registry_{}.json", sanitize_hub_key(key));
        self.remove_if_exists_durable(&filename)
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
        for dir in BACKUP_INTEGRATION_SUBDIRS {
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

        self.clear_integration_state_dirs(BACKUP_INTEGRATION_SUBDIRS)?;

        for (relative_path, file) in files {
            let absolute_path = self.dir.join(relative_path);
            self.write_file_atomic_path(&absolute_path, file.content.as_bytes())?;
        }

        Ok(())
    }

    fn validate_integration_backup_files(
        &self,
        files: &[crate::bundle::BackupIntegrationFile],
    ) -> Result<()> {
        let mut normalized_paths = std::collections::HashSet::new();
        let mut hue_bridge_ids = std::collections::HashSet::new();
        for file in files {
            let path = validate_integration_backup_path(&file.path)?;
            let normalized_path = normalized_relative_path(&path)?;
            if file.path != normalized_path {
                anyhow::bail!("integration backup path is not canonical");
            }
            if !normalized_paths.insert(path.clone()) {
                anyhow::bail!("duplicate integration backup path");
            }
            if path.extension() == Some(std::ffi::OsStr::new("json")) {
                let value: serde_json::Value = serde_json::from_str(&file.content)
                    .map_err(|_| anyhow::anyhow!("integration backup contains invalid JSON"))?;
                if normalized_path.starts_with("hue/controller-ownership/by-bridge/") {
                    let manifest = serde_json::from_value::<HueOwnershipManifestValidation>(value)
                        .map_err(|_| {
                            anyhow::anyhow!("invalid Hue controller ownership manifest")
                        })?;
                    let bridge_id = (!manifest.baseline.bridge_id.trim().is_empty())
                        .then_some(manifest.baseline.bridge_id.as_str())
                        .ok_or_else(|| {
                            anyhow::anyhow!("invalid Hue controller ownership manifest")
                        })?;
                    let safe_bridge_id = bridge_id
                        .chars()
                        .map(|character| {
                            if character.is_ascii_alphanumeric()
                                || matches!(character, '.' | '_' | '-')
                            {
                                character
                            } else {
                                '_'
                            }
                        })
                        .collect::<String>();
                    let expected_path =
                        format!("hue/controller-ownership/by-bridge/{safe_bridge_id}.json");
                    let valid_schema =
                        manifest.schema_version == 1 && manifest.baseline.schema_version == 1;
                    let valid_capture = !manifest.baseline.capture_id.trim().is_empty();
                    let valid_v2 = {
                        let resources = &manifest.baseline.v2_resources;
                        [
                            "bridge",
                            "device",
                            "light",
                            "behavior_instance",
                            "room",
                            "zone",
                            "scene",
                            "smart_scene",
                        ]
                        .iter()
                        .all(|resource_type| {
                            resources
                                .get(*resource_type)
                                .and_then(|resource| resource.get("data"))
                                .and_then(serde_json::Value::as_array)
                                .is_some()
                        })
                    };
                    let valid_v1 = {
                        let resources = &manifest.baseline.v1_resources;
                        ["rules", "schedules"].iter().all(|resource_type| {
                            resources
                                .get(*resource_type)
                                .and_then(serde_json::Value::as_object)
                                .is_some()
                        })
                    };
                    if !(valid_schema
                        && valid_capture
                        && valid_v2
                        && valid_v1
                        && normalized_path == expected_path
                        && hue_bridge_ids.insert(bridge_id.to_string()))
                    {
                        anyhow::bail!("invalid Hue controller ownership manifest");
                    }
                }
            }
        }
        Ok(())
    }

    fn load_integration_state_file(&self, path: &str) -> Result<Option<String>> {
        let relative_path = validate_integration_backup_path(path)?;
        let absolute_path = self.dir.join(relative_path);
        match std::fs::read_to_string(&absolute_path) {
            Ok(content) => Ok(Some(content)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(anyhow::anyhow!(error))
                .with_context(|| format!("reading {}", absolute_path.display())),
        }
    }

    fn save_integration_state_file(&self, path: &str, content: &str) -> Result<()> {
        let relative_path = validate_integration_backup_path(path)?;
        let relative_path = relative_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("integration state path is not UTF-8"))?;
        self.write_atomic_secret_durable(relative_path, content.as_bytes())
    }

    fn delete_integration_state_file(&self, path: &str) -> Result<()> {
        let relative_path = validate_integration_backup_path(path)?;
        let relative_path = relative_path
            .to_str()
            .ok_or_else(|| anyhow::anyhow!("integration state path is not UTF-8"))?;
        self.remove_if_exists_durable(relative_path)
    }

    fn load_legacy_authority_state(&self) -> Result<(Option<Value>, Option<Value>)> {
        let load_strict = |name: &str| -> Result<Option<Value>> {
            let path = self.file_path(name);
            match self.read_json::<Value>(name) {
                Ok(value) => Ok(Some(value)),
                Err(error) if path.exists() => Err(error).with_context(|| {
                    format!("legacy authority file is unreadable at {}", path.display())
                }),
                Err(_) => Ok(None),
            }
        };
        Ok((
            load_strict("canonical_registry.json")?,
            load_strict("topology.json")?,
        ))
    }

    fn load_authority_state(&self) -> Result<Option<StoredAuthorityState>> {
        let path = self.file_path("authority_state.json");
        if !path.exists() {
            debug!(
                target: "sys",
                "No persisted authority state at {}",
                path.display()
            );
            return Ok(None);
        }

        let state = self
            .read_json::<StoredAuthorityState>("authority_state.json")
            .context("Failed to load authority state")?;
        if state.schema_version != STORED_AUTHORITY_STATE_SCHEMA_VERSION {
            anyhow::bail!(
                "Unsupported authority-state schema version {} (expected {})",
                state.schema_version,
                STORED_AUTHORITY_STATE_SCHEMA_VERSION
            );
        }
        Ok(Some(state))
    }

    fn save_authority_state(&self, state: &StoredAuthorityState) -> Result<()> {
        if state.schema_version != STORED_AUTHORITY_STATE_SCHEMA_VERSION {
            anyhow::bail!(
                "Refusing to save authority-state schema version {} (expected {})",
                state.schema_version,
                STORED_AUTHORITY_STATE_SCHEMA_VERSION
            );
        }

        // This single file is the commit point. Once it is durable, a current
        // runtime will always recover the matching registry and topology even
        // if power is lost while refreshing the downgrade-compatible mirrors.
        let snapshot = serde_json::to_string_pretty(state)?;
        self.write_atomic_durable("authority_state.json", snapshot.as_bytes())?;

        for (name, value) in [
            ("canonical_registry.json", &state.canonical_registry),
            ("topology.json", &state.topology),
        ] {
            let mirror = serde_json::to_string_pretty(value)?;
            if let Err(error) = self.write_atomic_durable(name, mirror.as_bytes()) {
                // The authoritative commit is already durable. Returning an
                // error would make callers roll memory back behind that commit.
                warn!(
                    target: "sys",
                    "Authority state committed, but failed to refresh legacy mirror {}: {}",
                    name,
                    error
                );
            }
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

    fn load_server_metadata(&self) -> Result<Option<StoredServerMetadata>> {
        let path = self.file_path("server_metadata.json");
        if path.exists() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .open(&path)
                .with_context(|| format!("opening private metadata {}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .with_context(|| format!("securing private metadata {}", path.display()))?;
            }
        }
        match self.read_json::<StoredServerMetadata>("server_metadata.json") {
            Ok(metadata) => Ok(Some(metadata)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load server metadata {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted server metadata at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_server_metadata(&self, metadata: &StoredServerMetadata) -> Result<()> {
        let json = serde_json::to_string_pretty(metadata)?;
        // The public installation identity scopes pairing recovery across
        // reconnects, so commit its rename durably before pairing admission.
        self.write_atomic_secret_durable("server_metadata.json", json.as_bytes())
    }

    fn clear_server_metadata(&self) -> Result<()> {
        self.remove_if_exists_durable("server_metadata.json")?;
        self.remove_if_exists_durable("server_metadata.json.tmp")
    }

    fn load_pairing_metadata(&self) -> Result<Option<StoredPairingMetadata>> {
        let path = self.file_path("pairing_metadata.json");
        if path.exists() {
            let file = std::fs::OpenOptions::new()
                .read(true)
                .open(&path)
                .with_context(|| format!("opening private pairing metadata {}", path.display()))?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(std::fs::Permissions::from_mode(0o600))
                    .with_context(|| {
                        format!("securing private pairing metadata {}", path.display())
                    })?;
            }
        }
        match self.read_json::<StoredPairingMetadata>("pairing_metadata.json") {
            Ok(metadata) => Ok(Some(metadata)),
            Err(error) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load pairing metadata {}: {}",
                        path.display(),
                        error
                    );
                    return Err(error).with_context(|| {
                        format!(
                            "pairing metadata exists but is unreadable at {}",
                            path.display()
                        )
                    });
                } else {
                    debug!(
                        target: "sys",
                        "No persisted pairing metadata at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_pairing_metadata(&self, metadata: &StoredPairingMetadata) -> Result<()> {
        let json = serde_json::to_string_pretty(metadata)?;
        self.write_atomic_secret_durable("pairing_metadata.json", json.as_bytes())
    }

    fn clear_pairing_metadata(&self) -> Result<()> {
        self.remove_if_exists_durable("pairing_metadata.json")?;
        self.remove_if_exists_durable("pairing_metadata.json.tmp")
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

    fn load_remote_access_config(
        &self,
    ) -> Result<Option<crate::remote_access::StoredRemoteAccessConfig>> {
        let path = self.file_path("remote_access.json");
        match self.read_json::<crate::remote_access::StoredRemoteAccessConfig>("remote_access.json")
        {
            Ok(config) => Ok(Some(config)),
            Err(e) => {
                if path.exists() {
                    warn!(
                        target: "sys",
                        "Failed to load remote access config {}: {}",
                        path.display(),
                        e
                    );
                } else {
                    debug!(
                        target: "sys",
                        "No persisted remote access config at {}",
                        path.display()
                    );
                }
                Ok(None)
            }
        }
    }

    fn save_remote_access_config(
        &self,
        config: &crate::remote_access::StoredRemoteAccessConfig,
    ) -> Result<()> {
        let json = serde_json::to_string_pretty(config)?;
        self.write_atomic_secret("remote_access.json", json.as_bytes())
    }

    fn clear_remote_access_config(&self) -> Result<()> {
        self.remove_if_exists("remote_access.json")?;
        self.remove_if_exists("cloudflared/connector_token")?;
        self.remove_if_exists("cloudflared/hostname")
    }

    fn clear_factory_reset_state(&self) -> Result<()> {
        for name in [
            "rooms.json",
            "light_profiles.json",
            "location.json",
            "settings.json",
            "scenes.json",
            "motion_timers.json",
            "light_runtime_state.json",
            "activity_history.json",
            "light_usage_ledger.json",
            "pairing_history.json",
            "pairing_history.json.tmp",
            "activity_cloud.json",
            "hub_credentials.json",
            "canonical_registry.json",
            "topology.json",
            "commissioning_wifi.json",
            "auth.json",
            "support_bundle_jobs.json",
        ] {
            self.remove_if_exists(name)?;
        }
        // Delete the combined source of truth only after both legacy mirrors.
        // Its durable removal is the authority-state reset commit point: a
        // crash before it retains the prior complete snapshot, while a crash
        // after it cannot revive state from either compatibility half.
        self.remove_if_exists_durable("authority_state.json")?;
        self.clear_pairing_metadata()?;
        // Preserve the server's stable public installation ID while rotating
        // the confidential pairing HMAC domain across a factory reset.
        match self.load_server_metadata() {
            Ok(Some(metadata))
                if metadata.schema_version > STORED_SERVER_METADATA_SCHEMA_VERSION =>
            {
                // A reset must not retain unknown future secrets, and this
                // binary cannot safely rewrite a forward-owned schema while
                // preserving only the fields it understands.
                self.clear_server_metadata()?;
            }
            Ok(Some(mut metadata)) => {
                metadata.schema_version = STORED_SERVER_METADATA_SCHEMA_VERSION;
                metadata.legacy_pairing_hmac_key = None;
                if let Err(error) = self.save_server_metadata(&metadata) {
                    // If preserving the public installation ID cannot be made
                    // durable, fail closed by durably deleting its metadata.
                    // The next boot rotates the public ID as well as the key
                    // instead of silently retaining pre-reset identity state.
                    warn!(target: "sys", "Could not preserve public server identity while rotating pairing key: {error:#}");
                    self.clear_server_metadata()?;
                }
            }
            Ok(None) if self.file_path("server_metadata.json").exists() => {
                self.clear_server_metadata()?;
            }
            Ok(None) => {}
            Err(_) => self.clear_server_metadata()?,
        }
        self.clear_remote_access_config()?;
        self.clear_hub_registry_files()?;

        self.clear_integration_state_dirs(FACTORY_RESET_INTEGRATION_SUBDIRS)?;

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

fn decode_authority_state(
    stored: StoredAuthorityState,
) -> Result<(
    crate::canonical::registry::CanonicalRegistry,
    crate::topology::RoomTopologyStore,
)> {
    if stored.schema_version != STORED_AUTHORITY_STATE_SCHEMA_VERSION {
        anyhow::bail!(
            "Unsupported authority-state schema version {} (expected {})",
            stored.schema_version,
            STORED_AUTHORITY_STATE_SCHEMA_VERSION
        );
    }

    // Decode both halves before returning either. A malformed snapshot must
    // never partially replace the live authority state.
    let mut canonical_registry: crate::canonical::registry::CanonicalRegistry =
        serde_json::from_value(stored.canonical_registry)
            .context("Failed to parse authority-state canonical registry")?;
    let mut topology: crate::topology::RoomTopologyStore = serde_json::from_value(stored.topology)
        .context("Failed to parse authority-state topology")?;
    canonical_registry.rebuild_indices();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    canonical_registry.backfill_unassigned_triage(now);
    topology.rebuild_indices();
    Ok((canonical_registry, topology))
}

/// Load all persisted state from storage into AppState.
///
/// Loads config, location, settings, and hub credentials from the
/// configured [`Storage`] backend. Safe to call on any platform.
pub fn load_persisted_state(s: &mut crate::state::AppState) {
    if s.storage.is_none() {
        return;
    }

    ensure_server_instance_id(s);

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
                crate::light_runtime::reset_light_runtime_for_kind(s, settings.light_runtime);
                s.auto_update = settings.auto_update;
                s.update_channel = settings.update_channel;
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
                            light_runtime: s.light_runtime_kind.clone(),
                            active_mode: s.active_mode,
                            last_active_mode_cause: s.last_active_mode_cause,
                            last_active_mode_transition_id: s
                                .last_active_mode_transition_id
                                .clone(),
                            last_active_mode_change_utc_ms,
                            modes: normalized_modes.clone(),
                            mode_transitions: normalized_transitions,
                            auto_update: s.auto_update,
                            update_channel: s.update_channel,
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
        match storage.load_light_runtime_state() {
            Ok(Some(state)) => {
                let runtime_count = state.len();
                s.light_runtime_state = state;
                info!(
                    target: "sys",
                    "Loaded light runtime state: {} runtimes",
                    runtime_count
                );
            }
            Ok(None) => {}
            Err(e) => {
                debug!(target: "sys", "No persisted light runtime state loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_light_activity_history() {
            Ok(Some(history)) => {
                let history = history.normalized();
                let count = history.activities.len();
                s.light_activity = history.activities;
                info!(target: "sys", "Loaded light activity history: {} events", count);
            }
            Ok(None) => {}
            Err(e) => {
                debug!(target: "sys", "No persisted light activity history loaded: {}", e);
            }
        }
    }

    if let Some(storage) = s.storage.clone() {
        match storage.load_light_usage_ledger() {
            Ok(Some(ledger)) => match ledger.normalized(crate::state::current_epoch_ms()) {
                Ok(ledger) => {
                    let segment_count = ledger.segments.len();
                    s.light_usage = ledger;
                    info!(
                        target: "sys",
                        "Loaded light usage ledger: {} segments",
                        segment_count
                    );
                }
                Err(error) => warn!(
                    target: "sys",
                    "Rejected persisted light usage ledger: {error:#}"
                ),
            },
            Ok(None) => {}
            Err(error) => warn!(
                target: "sys",
                "Failed to load persisted light usage ledger: {error:#}"
            ),
        }
        s.light_usage.configure_shutdown_flush(storage);
    }

    if let Some(storage) = s.storage.as_ref() {
        match storage.load_scenes() {
            Ok(Some(stored)) => {
                s.scenes.clear();
                let mut removed_native_definitions = 0usize;
                for mut scene in stored.scenes {
                    scene.normalize();
                    if is_native_scene_id(&scene.id) {
                        removed_native_definitions += 1;
                        warn!(
                            target: "sys",
                            "Discarding persisted scene '{}' from the reserved native namespace",
                            scene.id
                        );
                        continue;
                    }
                    s.scenes.insert(scene.id.clone(), scene);
                }
                if removed_native_definitions > 0 {
                    if let Err(error) = storage.save_scenes(&s.stored_scenes()) {
                        warn!(
                            target: "sys",
                            "Failed to repair persisted native scene definitions: {}",
                            error
                        );
                    }
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

    let mut hub_credentials_loaded = false;
    if let Some(storage) = s.storage.as_ref() {
        match storage.load_all_hub_credentials() {
            Ok(all_creds) => {
                hub_credentials_loaded = true;
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

    let authority_storage = s.storage.as_ref().cloned();
    let mut loaded_authority_snapshot = false;
    let mut loaded_legacy_authority = false;
    let mut authority_sources_absent = false;
    s.authority_state_recovery_required = false;
    if let Some(storage) = authority_storage.as_ref() {
        let mut authority_snapshot_absent = false;
        match storage.load_authority_state() {
            Ok(Some(stored)) => match decode_authority_state(stored) {
                Ok((canonical_registry, topology)) => {
                    let device_count = canonical_registry.device_count();
                    let room_count = topology.room_count();
                    s.canonical_registry = canonical_registry;
                    s.topology = topology;
                    loaded_authority_snapshot = true;
                    info!(
                        target: "sys",
                        "Loaded authoritative registry + topology snapshot: {} devices, {} rooms",
                        device_count,
                        room_count
                    );
                }
                Err(error) => {
                    s.authority_state_recovery_required = true;
                    warn!(
                        target: "sys",
                        "Failed to decode authoritative registry + topology snapshot; refusing legacy mirror recovery: {}",
                        error
                    );
                }
            },
            Ok(None) => authority_snapshot_absent = true,
            Err(error) => {
                s.authority_state_recovery_required = true;
                warn!(
                    target: "sys",
                    "Failed to load authoritative registry + topology snapshot; refusing legacy mirror recovery: {}",
                    error
                );
            }
        }

        if !loaded_authority_snapshot && authority_snapshot_absent {
            // Legacy releases wrote these files independently, so a valid
            // installation may legitimately contain only one half. Pair a
            // present legacy half with the empty value for the missing domain,
            // then validate and commit both together. Once the combined commit
            // file has existed, any load/decode failure above is authoritative
            // evidence of corruption and must never be "repaired" from
            // independently refreshed, potentially mixed-generation mirrors.
            let legacy_values = match storage.load_legacy_authority_state() {
                Ok((Some(canonical_registry), Some(topology))) => {
                    Some((canonical_registry, topology))
                }
                Ok((None, None)) => {
                    authority_sources_absent = true;
                    if s.hub_credentials.is_empty() {
                        None
                    } else {
                        match (
                            serde_json::to_value(
                                crate::canonical::registry::CanonicalRegistry::new(),
                            ),
                            serde_json::to_value(crate::topology::RoomTopologyStore::legacy_empty()),
                        ) {
                            (Ok(canonical_registry), Ok(topology)) => {
                                Some((canonical_registry, topology))
                            }
                            (canonical_registry, topology) => {
                                s.authority_state_recovery_required = true;
                                warn!(
                                    target: "sys",
                                    "Failed to construct credentials-only legacy authority state: canonical_registry={:?}, topology={:?}",
                                    canonical_registry.err(),
                                    topology.err()
                                );
                                None
                            }
                        }
                    }
                }
                Ok((Some(canonical_registry), None)) => {
                    match serde_json::to_value(crate::topology::RoomTopologyStore::legacy_empty()) {
                        Ok(topology) => Some((canonical_registry, topology)),
                        Err(error) => {
                            s.authority_state_recovery_required = true;
                            warn!(
                                target: "sys",
                                "Failed to construct empty legacy topology for migration: {}",
                                error
                            );
                            None
                        }
                    }
                }
                Ok((None, Some(topology))) => {
                    match serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new())
                    {
                        Ok(canonical_registry) => Some((canonical_registry, topology)),
                        Err(error) => {
                            s.authority_state_recovery_required = true;
                            warn!(
                                target: "sys",
                                "Failed to construct empty legacy canonical registry for migration: {}",
                                error
                            );
                            None
                        }
                    }
                }
                Err(error) => {
                    s.authority_state_recovery_required = true;
                    warn!(
                        target: "sys",
                        "Failed to load legacy authority state; refusing partial authority recovery: {}",
                        error
                    );
                    None
                }
            };

            if let Some((canonical_registry, topology)) = legacy_values {
                match decode_authority_state(StoredAuthorityState::new(
                    canonical_registry,
                    topology,
                )) {
                    Ok((canonical_registry, topology)) => {
                        let device_count = canonical_registry.device_count();
                        let room_count = topology.room_count();
                        s.canonical_registry = canonical_registry;
                        s.topology = topology;
                        loaded_legacy_authority = true;
                        info!(
                            target: "sys",
                            "Loaded complete legacy authority state: {} devices, {} rooms",
                            device_count,
                            room_count
                        );
                    }
                    Err(error) => {
                        s.authority_state_recovery_required = true;
                        warn!(
                            target: "sys",
                            "Failed to decode complete legacy authority state; refusing partial authority recovery: {}",
                            error
                        );
                    }
                }
            }
        }
    }

    let configured_hub_keys = s.hub_credentials.keys().cloned().collect::<Vec<_>>();
    let external_automation_migration = s
        .topology
        .migrate_legacy_external_room_automation_policy(&configured_hub_keys);
    if external_automation_migration.changed() {
        info!(
            target: "sys",
            "Migrated legacy Hue room automation policy: grandfathered_hue_rooms={}",
            external_automation_migration.grandfathered_hue_rooms
        );
    }

    let topology_migration = s
        .topology
        .migrate_legacy_light_room_bindings(&mut s.canonical_registry);
    if topology_migration.changed() {
        info!(
            target: "sys",
            "Migrated legacy topology light bindings: filtered_light_device_ids={}, moved_light_devices={}",
            topology_migration.filtered_light_device_ids,
            topology_migration.moved_light_devices
        );
    }

    // A successful legacy load is migrated only after both files have been
    // considered and any legacy light-room bindings have moved. Likewise, a
    // migration of a valid combined snapshot is committed as one new record.
    // Invalid sources remain untouched rather than being replaced by defaults.
    // Persist an explicit version-1 empty authority snapshot for a genuinely
    // fresh installation. That commit must predate pairing credentials: if a
    // later pairing is interrupted after its credential write, the next boot
    // can distinguish the new unreviewed bridge from credentials left by a
    // pre-policy installation. Failed credential or authority reads are not
    // absence evidence and must never initialize over the files involved.
    let should_initialize_fresh_authority = authority_sources_absent
        && hub_credentials_loaded
        && s.hub_credentials.is_empty()
        && !s.authority_state_recovery_required;
    if should_initialize_fresh_authority {
        info!(target: "sys", "Initializing fresh versioned authority state");
    }

    let should_persist_authority_migration = loaded_legacy_authority
        || should_initialize_fresh_authority
        || (loaded_authority_snapshot
            && (topology_migration.changed() || external_automation_migration.changed()));
    if should_persist_authority_migration {
        if let Some(storage) = authority_storage.as_ref() {
            let migration_commit = (|| -> Result<()> {
                let canonical_registry = serde_json::to_value(&s.canonical_registry)
                    .context("Failed to serialize migrated canonical registry")?;
                let topology = serde_json::to_value(&s.topology)
                    .context("Failed to serialize migrated topology")?;
                storage
                    .save_authority_state(&StoredAuthorityState::new(canonical_registry, topology))
                    .context("Failed to persist migrated authority state")
            })();
            if let Err(error) = migration_commit {
                // The in-memory graph now reflects a migration that has no
                // crash-atomic durable counterpart. Keep all external
                // authority operations fenced until an explicit recovery path
                // successfully replaces the combined snapshot.
                s.authority_state_recovery_required = true;
                warn!(target: "sys", "{}", error);
            }
        }
    }

    if s.hub_credentials
        .values()
        .any(|credentials| credentials.backup_restore_pending)
    {
        // The imported Hue ownership manifest may already be durable, but the
        // rest of its backup graph was not committed. Preserve the staged
        // credentials for release/retry while preventing automatic takeover.
        s.authority_state_recovery_required = true;
        warn!(
            target: "sys",
            "Interrupted Hue backup restore requires retry or factory reset before controller startup"
        );
    }

    info!(target: "sys", "Persisted state loaded");
}

fn ensure_server_instance_id(s: &mut crate::state::AppState) {
    // Production targets install storage before calling this loader. Pairing
    // remains closed until both the stable public identity and the private key
    // are durable in their independently rollback-safe files.
    s.pairing_hmac_key_durable = s.storage.is_none();
    let Some(storage) = s.storage.as_ref() else {
        return;
    };

    let loaded_server = match storage.load_server_metadata() {
        Ok(metadata) => metadata,
        Err(error) => {
            warn!(target: "sys", "Failed to load server metadata: {}", error);
            None
        }
    };
    let pairing_load = storage.load_pairing_metadata();
    let loaded_pairing = match pairing_load.as_ref() {
        Ok(metadata) => metadata.as_ref(),
        Err(error) => {
            warn!(target: "sys", "Failed to load pairing metadata: {}", error);
            None
        }
    };

    if let Some(id) = loaded_server
        .as_ref()
        .map(|metadata| metadata.server_instance_id.trim())
        .filter(|id| !id.is_empty())
    {
        s.server_instance_id = id.to_string();
    } else if s.server_instance_id.trim().is_empty() {
        s.server_instance_id = generate_server_instance_id();
    }

    let server_schema_is_future = loaded_server
        .as_ref()
        .is_some_and(|metadata| metadata.schema_version > STORED_SERVER_METADATA_SCHEMA_VERSION);
    let server_metadata_is_current = loaded_server.as_ref().is_some_and(|metadata| {
        metadata.schema_version == STORED_SERVER_METADATA_SCHEMA_VERSION
            && metadata.server_instance_id == s.server_instance_id
            && metadata.legacy_pairing_hmac_key.is_none()
    });

    let pairing_schema_is_future = loaded_pairing
        .is_some_and(|metadata| metadata.schema_version > STORED_PAIRING_METADATA_SCHEMA_VERSION);
    let persisted_pairing_key = loaded_pairing
        .map(|metadata| metadata.pairing_hmac_key.as_str())
        .filter(|key| valid_pairing_hmac_key(key));
    let legacy_pairing_key = (!server_schema_is_future)
        .then(|| {
            loaded_server
                .as_ref()
                .and_then(|metadata| metadata.legacy_pairing_hmac_key.as_deref())
                .filter(|key| valid_pairing_hmac_key(key))
        })
        .flatten();

    if let Some(key) = persisted_pairing_key.or(legacy_pairing_key) {
        s.pairing_hmac_key = key.to_string();
    } else if !valid_pairing_hmac_key(&s.pairing_hmac_key) {
        s.pairing_hmac_key = generate_pairing_hmac_key();
    }

    let pairing_metadata_is_current = loaded_pairing.is_some_and(|metadata| {
        metadata.schema_version == STORED_PAIRING_METADATA_SCHEMA_VERSION
            && metadata.pairing_hmac_key == s.pairing_hmac_key
    });
    let pairing_metadata_durable = if pairing_load.is_err() {
        false
    } else if loaded_pairing.is_some() && persisted_pairing_key.is_none() {
        warn!(
            target: "sys",
            "Persisted pairing metadata has no valid key; preserving it and leaving pairing disabled"
        );
        false
    } else if server_schema_is_future && persisted_pairing_key.is_none() {
        warn!(
            target: "sys",
            "Future server metadata has no independently persisted pairing key; pairing remains disabled"
        );
        false
    } else if pairing_schema_is_future {
        if persisted_pairing_key.is_none() {
            warn!(
                target: "sys",
                "Future pairing metadata has no valid key; pairing remains disabled"
            );
            false
        } else {
            true
        }
    } else if pairing_metadata_is_current {
        true
    } else {
        let metadata = StoredPairingMetadata {
            schema_version: STORED_PAIRING_METADATA_SCHEMA_VERSION,
            pairing_hmac_key: s.pairing_hmac_key.clone(),
        };
        match storage.save_pairing_metadata(&metadata) {
            Ok(()) => {
                info!(target: "sys", "Generated or migrated private pairing metadata");
                true
            }
            Err(error) => {
                warn!(target: "sys", "Failed to persist private pairing metadata: {}", error);
                false
            }
        }
    };

    // During prerelease combined-file migration, commit the private key above
    // before rewriting server metadata without it. A failed second step leaves
    // pairing closed but never destroys the only durable copy of the key.
    let server_metadata_durable = if server_schema_is_future {
        let schema_version = loaded_server
            .as_ref()
            .map(|metadata| metadata.schema_version)
            .unwrap_or_default();
        warn!(
            target: "sys",
            "Server metadata schema {} is newer than supported {}; preserving it unchanged",
            schema_version,
            STORED_SERVER_METADATA_SCHEMA_VERSION
        );
        loaded_server
            .as_ref()
            .is_some_and(|metadata| !metadata.server_instance_id.trim().is_empty())
    } else if legacy_pairing_key.is_some() && !pairing_metadata_durable {
        warn!(
            target: "sys",
            "Private pairing metadata is not durable; preserving the prerelease combined metadata key"
        );
        false
    } else if server_metadata_is_current {
        true
    } else {
        let metadata = StoredServerMetadata {
            schema_version: STORED_SERVER_METADATA_SCHEMA_VERSION,
            server_instance_id: s.server_instance_id.clone(),
            legacy_pairing_hmac_key: None,
        };
        match storage.save_server_metadata(&metadata) {
            Ok(()) => {
                info!(target: "sys", "Generated or upgraded public server metadata");
                true
            }
            Err(error) => {
                warn!(target: "sys", "Failed to persist public server metadata: {}", error);
                false
            }
        }
    };

    s.pairing_hmac_key_durable = server_metadata_durable && pairing_metadata_durable;
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
        saved_canonical_registry: Arc<Mutex<Vec<Value>>>,
        topology: Option<Value>,
        saved_topology: Arc<Mutex<Vec<Value>>>,
        hub_credentials: Vec<HubCredentials>,
        authority_state_absent: bool,
        fail_save_authority_state: bool,
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
            Ok(self.hub_credentials.clone())
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

        fn load_authority_state(&self) -> Result<Option<StoredAuthorityState>> {
            if self.authority_state_absent {
                return Ok(None);
            }
            let (Some(canonical_registry), Some(topology)) =
                (self.canonical_registry.clone(), self.topology.clone())
            else {
                return Ok(None);
            };
            Ok(Some(StoredAuthorityState::new(
                canonical_registry,
                topology,
            )))
        }

        fn save_authority_state(&self, state: &StoredAuthorityState) -> Result<()> {
            if self.fail_save_authority_state {
                anyhow::bail!("injected authority-state commit failure");
            }
            self.save_canonical_registry(&state.canonical_registry)?;
            self.save_topology(&state.topology)
        }

        fn save_canonical_registry(&self, _data: &Value) -> Result<()> {
            self.saved_canonical_registry
                .lock()
                .unwrap()
                .push(_data.clone());
            Ok(())
        }

        fn load_topology(&self) -> Result<Option<Value>> {
            Ok(self.topology.clone())
        }

        fn save_topology(&self, _data: &Value) -> Result<()> {
            self.saved_topology.lock().unwrap().push(_data.clone());
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::default(),
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                auto_update: true,
                update_channel: None,
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
        assert!(storage.load_light_runtime_state().unwrap().is_none());
        storage
            .save_light_runtime_state(&StoredLightRuntimeState::default())
            .unwrap();
        assert!(storage.load_light_activity_history().unwrap().is_none());
        storage
            .save_light_activity_history(&crate::activity::LightActivityHistory::default())
            .unwrap();
        assert!(storage.load_activity_cloud_config().unwrap().is_none());
        storage
            .save_activity_cloud_config(&crate::activity_cloud::StoredActivityCloudConfig {
                schema_version: 1,
                enabled: false,
                ingest_url: String::new(),
                upload_token: String::new(),
                home_id: String::new(),
                hub_id: String::new(),
                token_id: None,
                server_instance_id: None,
                upload_status: None,
                last_upload_attempt_epoch_ms: None,
                last_upload_success_epoch_ms: None,
                last_upload_failure_epoch_ms: None,
                last_upload_http_status: None,
                last_upload_error: None,
                auth_failed_at_epoch_ms: None,
                updated_at_epoch_ms: 0,
            })
            .unwrap();
        storage.clear_activity_cloud_config().unwrap();
        storage.clear_hub_registries().unwrap();
        assert!(storage
            .load_integration_backup_files(true)
            .unwrap()
            .is_empty());
        storage.restore_integration_backup_files(&[]).unwrap();
        assert!(storage.load_authority_state().unwrap().is_none());
        storage
            .save_authority_state(&StoredAuthorityState::new(
                serde_json::json!({}),
                serde_json::json!({}),
            ))
            .unwrap();
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
        assert!(storage.load_remote_access_config().unwrap().is_none());
        storage
            .save_remote_access_config(&crate::remote_access::StoredRemoteAccessConfig {
                schema_version: 1,
                enabled: true,
                hostname: "hub.devices.rhythm.lighting".into(),
                connector_token: "secret".into(),
                tunnel_id: Some("tunnel-id".into()),
                tunnel_name: Some("tunnel-name".into()),
                updated_at_epoch_ms: 1,
            })
            .unwrap();
        storage.clear_remote_access_config().unwrap();
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
                update_channel: None,
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
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
                update_channel: None,
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
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
                update_channel: None,
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
                update_channel: None,
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
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
            topology: Some(
                serde_json::to_value(crate::topology::RoomTopologyStore::new()).unwrap(),
            ),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert!(!app.authority_state_recovery_required);
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
    fn load_persisted_state_grandfathers_hue_bridge_configured_before_consent() {
        use crate::topology::{DiscoveredTopologyRoom, ExternalRoomAutomationOwner, SyncAction};

        let address = "192.168.1.10";
        let hub_key = HubKey::new(crate::hub::HubType::new(crate::hub::HubType::HUE), address);
        let credentials = HubCredentials::new(
            crate::hub::HubType::HUE,
            address,
            serde_json::json!({"username": "existing-user"}),
        );
        let mut legacy_topology =
            serde_json::to_value(crate::topology::RoomTopologyStore::new()).unwrap();
        legacy_topology
            .as_object_mut()
            .unwrap()
            .remove("external_room_automation_policy_version");
        let storage = TestStorage {
            canonical_registry: Some(
                serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new()).unwrap(),
            ),
            topology: Some(legacy_topology),
            hub_credentials: vec![credentials],
            ..Default::default()
        };
        let mut app = crate::state::AppState {
            storage: Some(Arc::new(storage)),
            ..Default::default()
        };

        load_persisted_state(&mut app);

        let action = app.topology.sync_hub_room(
            &hub_key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-office".to_string(),
                name: "Office".to_string(),
                control_id: "grouped-office".to_string(),
                light_device_ids: Vec::new(),
                canonical_device_ids: Vec::new(),
                source_name_authoritative: true,
            },
        );
        let room_id = match action {
            SyncAction::Created { rhythm_room_id } => rhythm_room_id,
            other => panic!("unexpected sync action: {other:?}"),
        };
        assert_eq!(
            app.topology
                .external_room_automation_owner(&room_id, &hub_key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );
        assert!(app.topology.rhythm_automation_allowed_for_node(&room_id));
    }

    #[test]
    fn failed_legacy_authority_migration_commit_keeps_controller_writes_fenced() {
        let hub_key = HubKey::new(crate::hub::HubType::new("hue"), "bridge");
        let mut topology = crate::topology::RoomTopologyStore::new();
        topology.set_grouped_room_control_required(&hub_key, true);
        let storage = TestStorage {
            canonical_registry: Some(
                serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new()).unwrap(),
            ),
            topology: Some(serde_json::to_value(topology).unwrap()),
            authority_state_absent: true,
            fail_save_authority_state: true,
            ..Default::default()
        };
        let mut app = crate::state::AppState {
            storage: Some(Arc::new(storage)),
            ..Default::default()
        };

        load_persisted_state(&mut app);

        assert!(app.authority_state_recovery_required);
        assert!(!app.external_controller_authority_is_ready(&hub_key));
    }

    #[test]
    fn failed_fresh_authority_initialization_keeps_controller_writes_fenced() {
        let hub_key = HubKey::new(crate::hub::HubType::new("hue"), "new-bridge");
        let storage = TestStorage {
            fail_save_authority_state: true,
            ..Default::default()
        };
        let mut app = crate::state::AppState {
            storage: Some(Arc::new(storage)),
            ..Default::default()
        };

        load_persisted_state(&mut app);

        assert!(app.authority_state_recovery_required);
        assert!(!app.external_controller_authority_is_ready(&hub_key));
    }

    #[test]
    fn load_persisted_state_preserves_user_light_move_while_filtering_legacy_bindings() {
        use crate::canonical::identity::{DiscoveredIdentity, HardwareId};
        use crate::canonical::registry::{CanonicalRegistry, ResolveResult};
        use crate::hub::HubType;
        use crate::topology::{DevicePlacement, DiscoveredTopologyRoom, SyncAction};
        use rhythm_core::runtime::hub_registry::DeviceType;

        let hub_key = HubKey::new(HubType::new("hue"), "192.168.1.10");
        let mut registry = CanonicalRegistry::new();

        let stairwell_light_id = match registry.resolve(
            &DiscoveredIdentity {
                native_id: "stairwell-light-1".to_string(),
                room_id: Some("hue-stairwell".to_string()),
                room_name: Some("Stairwell".to_string()),
                name: "Stairwell light".to_string(),
                device_type: DeviceType::Light,
                hardware_ids: vec![HardwareId::mac("00:17:88:01:00:00:00:01")],
                manufacturer: Some("Signify".to_string()),
                model: Some("LCT001".to_string()),
            },
            &hub_key,
            1000,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {:?}", other),
        };
        let drop_zone_light_id = match registry.resolve(
            &DiscoveredIdentity {
                native_id: "drop-zone-light-1".to_string(),
                room_id: Some("hue-drop-zone".to_string()),
                room_name: Some("Drop Zone".to_string()),
                name: "Drop Zone light".to_string(),
                device_type: DeviceType::Light,
                hardware_ids: vec![HardwareId::mac("00:17:88:01:00:00:00:02")],
                manufacturer: Some("Signify".to_string()),
                model: Some("LCT001".to_string()),
            },
            &hub_key,
            1000,
        ) {
            ResolveResult::Created { canonical_id } => canonical_id,
            other => panic!("unexpected resolve result: {:?}", other),
        };

        let mut topology = crate::topology::RoomTopologyStore::new();
        let stairwell_room_id = match topology.sync_hub_room(
            &hub_key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-stairwell".to_string(),
                name: "Stairwell".to_string(),
                source_name_authoritative: true,
                control_id: "stairwell-grouped-light".to_string(),
                light_device_ids: vec![
                    "stairwell-light-1".to_string(),
                    "stale-motion-device-id".to_string(),
                ],
                canonical_device_ids: vec![stairwell_light_id.clone()],
            },
        ) {
            SyncAction::Created { rhythm_room_id } => rhythm_room_id,
            other => panic!("unexpected sync action: {:?}", other),
        };
        let drop_zone_room_id = match topology.sync_hub_room(
            &hub_key,
            &DiscoveredTopologyRoom {
                hub_room_id: "hue-drop-zone".to_string(),
                name: "Drop Zone".to_string(),
                source_name_authoritative: true,
                control_id: "drop-zone-grouped-light".to_string(),
                light_device_ids: vec!["drop-zone-light-1".to_string()],
                canonical_device_ids: vec![drop_zone_light_id.clone()],
            },
        ) {
            SyncAction::Created { rhythm_room_id } => rhythm_room_id,
            other => panic!("unexpected sync action: {:?}", other),
        };

        assert!(topology.move_device(&drop_zone_light_id, &drop_zone_room_id, &stairwell_room_id,));
        registry.assign_room(&stairwell_light_id, Some(&stairwell_room_id));
        registry.assign_room(&drop_zone_light_id, Some(&stairwell_room_id));

        let saved_canonical_registry = Arc::new(Mutex::new(Vec::new()));
        let saved_topology = Arc::new(Mutex::new(Vec::new()));
        let storage = TestStorage {
            canonical_registry: Some(serde_json::to_value(&registry).unwrap()),
            saved_canonical_registry: saved_canonical_registry.clone(),
            topology: Some(serde_json::to_value(&topology).unwrap()),
            saved_topology: saved_topology.clone(),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
            ..Default::default()
        };
        load_persisted_state(&mut app);

        assert_eq!(
            app.topology.device_parent_room_id(&drop_zone_light_id),
            Some(stairwell_room_id.as_str()),
            "startup migration must not replace an explicit user room move with the Hue default"
        );
        assert_eq!(
            app.topology
                .get_device_node(&drop_zone_light_id)
                .unwrap()
                .placement,
            DevicePlacement::UserOverride
        );
        assert_eq!(
            app.canonical_registry
                .get(&drop_zone_light_id)
                .unwrap()
                .room_id
                .as_deref(),
            Some(stairwell_room_id.as_str()),
            "canonical room authority must survive the same restart boundary as topology"
        );

        let stairwell_room = app
            .topology
            .find_by_hub_room(&hub_key, "hue-stairwell")
            .expect("stairwell room should still be bound");
        assert_eq!(
            stairwell_room
                .binding_for_hub_room(&hub_key, "hue-stairwell")
                .unwrap()
                .light_device_ids,
            vec!["stairwell-light-1".to_string()]
        );

        assert_eq!(saved_canonical_registry.lock().unwrap().len(), 1);
        assert_eq!(saved_topology.lock().unwrap().len(), 1);
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::Manual,
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: vec![],
                mode_transitions: vec![],
                auto_update: true,
                update_channel: None,
            }),
            ..Default::default()
        };

        let mut app = crate::state::AppState {
            storage: Some(std::sync::Arc::new(storage)),
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
        use std::sync::{Arc, Barrier};

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

        fn valid_hue_ownership_backup_file() -> crate::bundle::BackupIntegrationFile {
            crate::bundle::BackupIntegrationFile {
                path: "hue/controller-ownership/by-bridge/bridge-1.json".to_string(),
                content: serde_json::json!({
                    "schema_version": 1,
                    "phase": "active",
                    "baseline": {
                        "schema_version": 1,
                        "capture_id": "capture-1",
                        "bridge_id": "bridge-1",
                        "v2_resources": {
                            "bridge": {"data": []},
                            "device": {"data": []},
                            "light": {"data": []},
                            "behavior_instance": {"data": []},
                            "room": {"data": []},
                            "zone": {"data": []},
                            "scene": {"data": []},
                            "smart_scene": {"data": []}
                        },
                        "v1_resources": {
                            "rules": {},
                            "schedules": {}
                        }
                    },
                    "managed_rooms": {
                        "rhythm-room": {
                            "rhythm_room_id": "rhythm-room",
                            "hue_room_id": "hue-room",
                            "grouped_light_id": "grouped-light"
                        }
                    },
                    "managed_scenes": {
                        "rhythm-room:rhythm-scene": {
                            "rhythm_room_id": "rhythm-room",
                            "rhythm_scene_id": "rhythm-scene",
                            "hue_room_id": "hue-room",
                            "hue_scene_id": "hue-scene",
                            "fingerprint": "fingerprint",
                            "ephemeral": false
                        }
                    },
                    "restored_resource_ids": {"room:old": "new"},
                    "receipts": {
                        "operation-1": {
                            "operation_id": "operation-1",
                            "api": "v2",
                            "action": "delete",
                            "resource_type": "room",
                            "original_resource_id": "old",
                            "status": "succeeded",
                            "attempt": 1
                        }
                    }
                })
                .to_string(),
                secret: true,
            }
        }

        struct PairingDirectorySyncFailureStorage {
            server_metadata: StoredServerMetadata,
            pairing_saves: Arc<std::sync::Mutex<Vec<StoredPairingMetadata>>>,
            server_saves: Arc<std::sync::Mutex<Vec<StoredServerMetadata>>>,
        }

        impl Storage for PairingDirectorySyncFailureStorage {
            fn load_rooms(&self) -> Result<RoomManager> {
                Ok(RoomManager::new())
            }

            fn save_rooms(&self, _rooms: &RoomManager) -> Result<()> {
                Ok(())
            }

            fn load_light_profiles(&self) -> Result<StoredLightProfiles> {
                anyhow::bail!("unused")
            }

            fn save_light_profiles(&self, _config: &StoredLightProfiles) -> Result<()> {
                Ok(())
            }

            fn load_location(&self) -> Result<StoredLocation> {
                anyhow::bail!("unused")
            }

            fn save_location(&self, _loc: &StoredLocation) -> Result<()> {
                Ok(())
            }

            fn load_settings(&self) -> Result<StoredSettings> {
                anyhow::bail!("unused")
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

            fn load_server_metadata(&self) -> Result<Option<StoredServerMetadata>> {
                Ok(Some(self.server_metadata.clone()))
            }

            fn save_server_metadata(&self, metadata: &StoredServerMetadata) -> Result<()> {
                self.server_saves.lock().unwrap().push(metadata.clone());
                Ok(())
            }

            fn load_pairing_metadata(&self) -> Result<Option<StoredPairingMetadata>> {
                Ok(None)
            }

            fn save_pairing_metadata(&self, metadata: &StoredPairingMetadata) -> Result<()> {
                // Model rename success followed by a parent-directory fsync
                // failure: the attempted document is observable, but the
                // durability contract must still return an error.
                self.pairing_saves.lock().unwrap().push(metadata.clone());
                anyhow::bail!("pairing metadata parent-directory fsync failed")
            }
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
        fn concurrent_activity_history_writes_are_serialized() {
            let (storage, path) = temp_storage();
            let storage = Arc::new(storage);
            let writers = 24;
            let barrier = Arc::new(Barrier::new(writers));
            let mut handles = Vec::with_capacity(writers);

            for _ in 0..writers {
                let storage = Arc::clone(&storage);
                let barrier = Arc::clone(&barrier);
                handles.push(std::thread::spawn(move || {
                    barrier.wait();
                    storage.save_light_activity_history(
                        &crate::activity::LightActivityHistory::default(),
                    )
                }));
            }

            for handle in handles {
                handle.join().unwrap().unwrap();
            }

            assert!(storage.load_light_activity_history().unwrap().is_some());
            assert!(!path.join("activity_history.json.tmp").exists());
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
                storage: Some(std::sync::Arc::new(
                    FileStorage::new(path.to_str().unwrap()).unwrap(),
                )),
                ..Default::default()
            };
            load_persisted_state(&mut state);

            assert!(state.scenes.contains_key("icy-glow"));
            assert_eq!(state.scenes["icy-glow"].name, "Icy Glow");
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_repairs_reserved_native_scene_definitions() {
            let (storage, path) = temp_storage();
            let native_id = crate::scenes::native_scene_id("hue", "native-1");
            storage
                .save_scenes(&crate::scenes::StoredScenes {
                    schema_version: crate::scenes::LIGHT_SCENE_SCHEMA_VERSION,
                    scenes: vec![
                        crate::scenes::SceneDefinition {
                            id: "icy-glow".into(),
                            name: "Icy Glow".into(),
                            description: None,
                            source: crate::scenes::SceneSource::User,
                            light: None,
                            extensions: std::collections::BTreeMap::new(),
                        },
                        crate::scenes::SceneDefinition {
                            id: native_id.clone(),
                            name: "Persisted Hue shadow".into(),
                            description: None,
                            source: crate::scenes::SceneSource::User,
                            light: None,
                            extensions: std::collections::BTreeMap::new(),
                        },
                    ],
                })
                .unwrap();

            let storage = std::sync::Arc::new(storage);
            let mut state = crate::state::AppState {
                storage: Some(storage.clone()),
                ..Default::default()
            };
            load_persisted_state(&mut state);

            assert!(state.scenes.contains_key("icy-glow"));
            assert!(!state.scenes.contains_key(&native_id));
            let repaired = storage.load_scenes().unwrap().unwrap();
            assert_eq!(repaired.scenes.len(), 1);
            assert_eq!(repaired.scenes[0].id, "icy-glow");
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
                storage: Some(std::sync::Arc::new(
                    FileStorage::new(path.to_str().unwrap()).unwrap(),
                )),
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
                storage: Some(std::sync::Arc::new(
                    FileStorage::new(path.to_str().unwrap()).unwrap(),
                )),
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Sleep,
                last_active_mode_cause: ModeChangeCause::Schedule,
                last_active_mode_transition_id: Some("sleep_to_day".into()),
                last_active_mode_change_utc_ms: Some(1_234_567_890),
                modes: rhythm_core::default_mode_configs(),
                mode_transitions: rhythm_core::default_mode_transition_configs(),
                auto_update: false,
                update_channel: None,
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
        fn settings_update_channel_roundtrips_absent_beta_and_stable() {
            let (storage, path) = temp_storage();

            let mut settings = StoredSettings {
                power_save: false,
                light_breaker_enabled: true,
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::default(),
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                auto_update: true,
                update_channel: None,
            };
            storage.save_settings(&settings).unwrap();
            assert!(storage.load_settings().unwrap().update_channel.is_none());
            let raw = std::fs::read_to_string(path.join("settings.json")).unwrap();
            assert!(
                !raw.contains("update_channel"),
                "absent channel must not be serialized: {raw}"
            );

            settings.update_channel = Some(crate::state::UpdateChannel::Beta);
            storage.save_settings(&settings).unwrap();
            assert_eq!(
                storage.load_settings().unwrap().update_channel,
                Some(crate::state::UpdateChannel::Beta)
            );

            settings.update_channel = Some(crate::state::UpdateChannel::Stable);
            storage.save_settings(&settings).unwrap();
            assert_eq!(
                storage.load_settings().unwrap().update_channel,
                Some(crate::state::UpdateChannel::Stable)
            );
            let raw = std::fs::read_to_string(path.join("settings.json")).unwrap();
            assert!(
                raw.contains("\"update_channel\": \"stable\""),
                "channel must serialize lowercase: {raw}"
            );

            cleanup(&path);
        }

        #[test]
        fn settings_missing_light_breaker_defaults_false_and_auto_update_defaults_true() {
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
            assert!(!loaded.light_breaker_enabled);
            assert!(loaded.auto_update);
            cleanup(&path);
        }

        #[test]
        fn settings_legacy_mode_transitions_default_disabled() {
            let (storage, path) = temp_storage();
            let json = r#"{
              "power_save": false,
              "active_mode": "day",
              "modes": [],
              "mode_transitions": [
                {
                  "id": "sleep_to_day",
                  "label": "Sleep to Day",
                  "from_mode": "sleep",
                  "to_mode": "day",
                  "trigger": "civil_twilight"
                },
                {
                  "id": "day_to_sleep",
                  "label": "Day to Sleep",
                  "from_mode": "day",
                  "to_mode": "sleep",
                  "trigger": "nautical_twilight"
                }
              ]
            }"#;
            std::fs::write(path.join("settings.json"), json).unwrap();

            let loaded = storage.load_settings().unwrap();

            assert_eq!(loaded.mode_transitions.len(), 2);
            assert!(loaded
                .mode_transitions
                .iter()
                .all(|transition| !transition.trigger_enabled));
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
        fn authority_state_save_load_roundtrip_and_refreshes_legacy_mirrors() {
            let (storage, path) = temp_storage();
            let state = StoredAuthorityState::new(
                serde_json::json!({"devices": {"combined-device": {"name": "Combined"}}}),
                serde_json::json!({"rooms": {"combined-room": {"name": "Combined"}}}),
            );

            storage.save_authority_state(&state).unwrap();

            assert_eq!(storage.load_authority_state().unwrap(), Some(state.clone()));
            assert_eq!(
                storage.load_canonical_registry().unwrap(),
                Some(state.canonical_registry)
            );
            assert_eq!(storage.load_topology().unwrap(), Some(state.topology));
            assert!(path.join("authority_state.json").exists());
            cleanup(&path);
        }

        #[test]
        fn pending_backup_restore_credentials_fence_startup_after_restart() {
            let (storage, path) = temp_storage();
            let mut credentials = HubCredentials::new(
                crate::hub::HubType::HUE,
                "bridge.local",
                serde_json::json!({
                    "username": "recovery-user",
                    "bridge_id": "bridge-1"
                }),
            );
            credentials.backup_restore_pending = true;
            storage
                .save_all_hub_credentials(std::slice::from_ref(&credentials))
                .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            let loaded = app
                .hub_credentials
                .values()
                .next()
                .expect("pending recovery credentials should load");
            assert!(loaded.backup_restore_pending);
            assert!(loaded.can_restore_external_controller());
            assert!(!loaded.can_connect());
            assert!(app.authority_state_recovery_required);
            cleanup(&path);
        }

        fn load_file_backed_app(storage: Arc<FileStorage>) -> crate::state::AppState {
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);
            app
        }

        fn discover_hue_room(
            app: &mut crate::state::AppState,
            hub_key: &HubKey,
            native_room_id: &str,
        ) -> String {
            use crate::topology::{DiscoveredTopologyRoom, SyncAction};

            match app.topology.sync_hub_room(
                hub_key,
                &DiscoveredTopologyRoom {
                    hub_room_id: native_room_id.to_string(),
                    name: format!("Room {native_room_id}"),
                    control_id: format!("grouped-{native_room_id}"),
                    light_device_ids: Vec::new(),
                    canonical_device_ids: Vec::new(),
                    source_name_authoritative: true,
                },
            ) {
                SyncAction::Created { rhythm_room_id } => rhythm_room_id,
                other => panic!("unexpected sync action: {other:?}"),
            }
        }

        #[test]
        fn credentials_only_legacy_hue_state_is_grandfathered_across_two_restarts() {
            use crate::topology::ExternalRoomAutomationOwner;

            let (storage, path) = temp_storage();
            let address = "192.0.2.20";
            let hub_key = HubKey::new(crate::hub::HubType::new(crate::hub::HubType::HUE), address);
            storage
                .save_all_hub_credentials(&[HubCredentials::new(
                    crate::hub::HubType::HUE,
                    address,
                    serde_json::json!({"username": "existing-user"}),
                )])
                .unwrap();
            let storage = Arc::new(storage);

            let first_restart = load_file_backed_app(storage.clone());
            assert!(!first_restart.authority_state_recovery_required);
            assert_eq!(
                first_restart
                    .topology
                    .external_room_automation_owner("future-room", &hub_key),
                Some(ExternalRoomAutomationOwner::Rhythm)
            );
            assert!(path.join("authority_state.json").exists());
            drop(first_restart);

            let mut second_restart = load_file_backed_app(storage.clone());
            assert!(!second_restart.authority_state_recovery_required);
            let room_id = discover_hue_room(&mut second_restart, &hub_key, "legacy-office");
            assert_eq!(
                second_restart
                    .topology
                    .external_room_automation_owner(&room_id, &hub_key),
                Some(ExternalRoomAutomationOwner::Rhythm)
            );
            assert!(second_restart
                .topology
                .rhythm_automation_allowed_for_node(&room_id));
            cleanup(&path);
        }

        #[test]
        fn canonical_only_legacy_hue_state_is_grandfathered_across_two_restarts() {
            use crate::topology::ExternalRoomAutomationOwner;

            let (storage, path) = temp_storage();
            let address = "192.0.2.21";
            let hub_key = HubKey::new(crate::hub::HubType::new(crate::hub::HubType::HUE), address);
            storage
                .save_all_hub_credentials(&[HubCredentials::new(
                    crate::hub::HubType::HUE,
                    address,
                    serde_json::json!({"username": "existing-user"}),
                )])
                .unwrap();
            storage
                .save_canonical_registry(
                    &serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new())
                        .unwrap(),
                )
                .unwrap();
            let storage = Arc::new(storage);

            let first_restart = load_file_backed_app(storage.clone());
            assert!(!first_restart.authority_state_recovery_required);
            assert_eq!(
                first_restart
                    .topology
                    .external_room_automation_owner("future-room", &hub_key),
                Some(ExternalRoomAutomationOwner::Rhythm)
            );
            assert!(path.join("authority_state.json").exists());
            drop(first_restart);

            let mut second_restart = load_file_backed_app(storage.clone());
            assert!(!second_restart.authority_state_recovery_required);
            let room_id = discover_hue_room(&mut second_restart, &hub_key, "legacy-bedroom");
            assert_eq!(
                second_restart
                    .topology
                    .external_room_automation_owner(&room_id, &hub_key),
                Some(ExternalRoomAutomationOwner::Rhythm)
            );
            assert!(second_restart
                .topology
                .rhythm_automation_allowed_for_node(&room_id));
            cleanup(&path);
        }

        #[test]
        fn fresh_authority_snapshot_prevents_interrupted_new_pairing_from_being_grandfathered() {
            let (storage, path) = temp_storage();
            let storage = Arc::new(storage);
            let address = "192.0.2.22";
            let hub_key = HubKey::new(crate::hub::HubType::new(crate::hub::HubType::HUE), address);

            let fresh_start = load_file_backed_app(storage.clone());
            assert!(!fresh_start.authority_state_recovery_required);
            assert!(path.join("authority_state.json").exists());
            assert_eq!(
                fresh_start
                    .topology
                    .external_room_automation_owner("future-room", &hub_key),
                None
            );
            drop(fresh_start);

            // Model the durability window where a newly paired bridge's
            // credentials commit but discovery/topology never does.
            storage
                .save_all_hub_credentials(&[HubCredentials::new(
                    crate::hub::HubType::HUE,
                    address,
                    serde_json::json!({"username": "new-user"}),
                )])
                .unwrap();

            let first_restart = load_file_backed_app(storage.clone());
            assert!(!first_restart.authority_state_recovery_required);
            assert_eq!(
                first_restart
                    .topology
                    .external_room_automation_owner("future-room", &hub_key),
                None
            );
            drop(first_restart);

            let mut second_restart = load_file_backed_app(storage.clone());
            assert!(!second_restart.authority_state_recovery_required);
            let room_id = discover_hue_room(&mut second_restart, &hub_key, "new-office");
            assert_eq!(
                second_restart
                    .topology
                    .external_room_automation_owner(&room_id, &hub_key),
                None
            );
            assert!(!second_restart
                .topology
                .rhythm_automation_allowed_for_node(&room_id));
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_prefers_combined_authority_over_divergent_legacy_files() {
            let (storage, path) = temp_storage();
            let mut combined_topology = crate::topology::RoomTopologyStore::new();
            combined_topology.create_room("Combined Room");
            storage
                .save_authority_state(&StoredAuthorityState::new(
                    serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new())
                        .unwrap(),
                    serde_json::to_value(&combined_topology).unwrap(),
                ))
                .unwrap();

            let mut legacy_topology = crate::topology::RoomTopologyStore::new();
            legacy_topology.create_room("Stale Legacy Room");
            std::fs::write(
                path.join("topology.json"),
                serde_json::to_vec_pretty(&legacy_topology).unwrap(),
            )
            .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            let names = app
                .topology
                .rooms()
                .map(|room| room.name.as_str())
                .collect::<Vec<_>>();
            assert_eq!(names, vec!["Combined Room"]);
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_migrates_legacy_authority_files_to_combined_snapshot() {
            let (storage, path) = temp_storage();
            let registry = crate::canonical::registry::CanonicalRegistry::new();
            let mut topology = crate::topology::RoomTopologyStore::new();
            topology.create_room("Legacy Room");
            storage
                .save_canonical_registry(&serde_json::to_value(&registry).unwrap())
                .unwrap();
            storage
                .save_topology(&serde_json::to_value(&topology).unwrap())
                .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage.clone()),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            let combined = storage.load_authority_state().unwrap().unwrap();
            let combined_topology: crate::topology::RoomTopologyStore =
                serde_json::from_value(combined.topology).unwrap();
            assert_eq!(
                combined_topology
                    .rooms()
                    .map(|room| room.name.as_str())
                    .collect::<Vec<_>>(),
                vec!["Legacy Room"]
            );
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_fails_closed_when_combined_snapshot_is_corrupt() {
            let (storage, path) = temp_storage();
            let registry = crate::canonical::registry::CanonicalRegistry::new();
            let mut topology = crate::topology::RoomTopologyStore::new();
            topology.create_room("Stale Legacy Room");
            storage
                .save_canonical_registry(&serde_json::to_value(&registry).unwrap())
                .unwrap();
            storage
                .save_topology(&serde_json::to_value(&topology).unwrap())
                .unwrap();
            std::fs::write(path.join("authority_state.json"), b"{").unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage.clone()),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.topology.room_count(), 0);
            assert!(app.authority_state_recovery_required);
            assert_eq!(
                std::fs::read(path.join("authority_state.json")).unwrap(),
                b"{"
            );
            assert!(path.join("canonical_registry.json").exists());
            assert!(path.join("topology.json").exists());
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_fails_closed_when_combined_snapshot_cannot_decode() {
            let (storage, path) = temp_storage();
            let registry = crate::canonical::registry::CanonicalRegistry::new();
            let mut legacy_topology = crate::topology::RoomTopologyStore::new();
            legacy_topology.create_room("Stale Legacy Room");
            storage
                .save_canonical_registry(&serde_json::to_value(&registry).unwrap())
                .unwrap();
            storage
                .save_topology(&serde_json::to_value(&legacy_topology).unwrap())
                .unwrap();

            let invalid_combined = StoredAuthorityState::new(
                serde_json::json!({"not": "a canonical registry"}),
                serde_json::to_value(crate::topology::RoomTopologyStore::new()).unwrap(),
            );
            let invalid_combined_bytes = serde_json::to_vec_pretty(&invalid_combined).unwrap();
            std::fs::write(path.join("authority_state.json"), &invalid_combined_bytes).unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.canonical_registry.device_count(), 0);
            assert_eq!(app.topology.room_count(), 0);
            assert!(app.authority_state_recovery_required);
            assert_eq!(
                std::fs::read(path.join("authority_state.json")).unwrap(),
                invalid_combined_bytes
            );
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_migrates_canonical_only_legacy_authority() {
            let (storage, path) = temp_storage();
            let mut registry = crate::canonical::registry::CanonicalRegistry::new();
            let hub_key = HubKey::new(crate::hub::HubType::new("hue"), "192.0.2.10");
            let identity = crate::canonical::identity::DiscoveredIdentity {
                native_id: "legacy-light".to_string(),
                room_id: None,
                room_name: None,
                name: "Legacy Light".to_string(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: vec![crate::canonical::identity::HardwareId::mac(
                    "00:17:88:01:02:03:04:05",
                )],
                manufacturer: Some("Signify".to_string()),
                model: Some("LCT001".to_string()),
            };
            let canonical_id = match registry.resolve(&identity, &hub_key, 1000) {
                crate::canonical::registry::ResolveResult::Created { canonical_id } => canonical_id,
                other => panic!("unexpected resolve result: {other:?}"),
            };
            storage
                .save_canonical_registry(&serde_json::to_value(&registry).unwrap())
                .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.canonical_registry.device_count(), 1);
            assert!(app.canonical_registry.get(&canonical_id).is_some());
            assert_eq!(app.topology.room_count(), 0);
            assert!(!app.authority_state_recovery_required);
            assert!(path.join("authority_state.json").exists());
            assert!(path.join("canonical_registry.json").exists());
            assert!(path.join("topology.json").exists());
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_migrates_topology_only_legacy_authority() {
            let (storage, path) = temp_storage();
            let mut topology = crate::topology::RoomTopologyStore::new();
            topology.create_room("Unpaired Legacy Room");
            storage
                .save_topology(&serde_json::to_value(&topology).unwrap())
                .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.canonical_registry.device_count(), 0);
            assert_eq!(app.topology.room_count(), 1);
            assert!(!app.authority_state_recovery_required);
            assert!(path.join("authority_state.json").exists());
            assert!(path.join("canonical_registry.json").exists());
            assert!(path.join("topology.json").exists());
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_does_not_migrate_malformed_legacy_pair() {
            let (storage, path) = temp_storage();
            let registry = crate::canonical::registry::CanonicalRegistry::new();
            storage
                .save_canonical_registry(&serde_json::to_value(&registry).unwrap())
                .unwrap();
            std::fs::write(path.join("topology.json"), b"{").unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.canonical_registry.device_count(), 0);
            assert_eq!(app.topology.room_count(), 0);
            assert!(app.authority_state_recovery_required);
            assert!(!path.join("authority_state.json").exists());
            assert_eq!(std::fs::read(path.join("topology.json")).unwrap(), b"{");
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_does_not_migrate_malformed_canonical_legacy_state() {
            let (storage, path) = temp_storage();
            let mut topology = crate::topology::RoomTopologyStore::new();
            topology.create_room("Legacy Room");
            std::fs::write(path.join("canonical_registry.json"), b"{").unwrap();
            storage
                .save_topology(&serde_json::to_value(&topology).unwrap())
                .unwrap();

            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };
            load_persisted_state(&mut app);

            assert_eq!(app.canonical_registry.device_count(), 0);
            assert_eq!(app.topology.room_count(), 0);
            assert!(app.authority_state_recovery_required);
            assert!(!path.join("authority_state.json").exists());
            assert_eq!(
                std::fs::read(path.join("canonical_registry.json")).unwrap(),
                b"{"
            );
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_preserves_invalid_combined_snapshot_without_legacy_recovery() {
            let (storage, path) = temp_storage();
            std::fs::write(path.join("authority_state.json"), b"{").unwrap();
            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage),
                ..Default::default()
            };

            load_persisted_state(&mut app);

            assert!(app.authority_state_recovery_required);
            assert_eq!(
                std::fs::read(path.join("authority_state.json")).unwrap(),
                b"{"
            );
            assert!(!path.join("canonical_registry.json").exists());
            assert!(!path.join("topology.json").exists());
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
        fn server_metadata_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let metadata = StoredServerMetadata {
                schema_version: STORED_SERVER_METADATA_SCHEMA_VERSION,
                server_instance_id: "srv-test-instance".into(),
                legacy_pairing_hmac_key: None,
            };

            storage.save_server_metadata(&metadata).unwrap();
            let loaded = storage.load_server_metadata().unwrap();
            assert_eq!(loaded, Some(metadata));

            storage.clear_server_metadata().unwrap();
            assert!(storage.load_server_metadata().unwrap().is_none());
            cleanup(&path);
        }

        #[test]
        fn pairing_metadata_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let metadata = StoredPairingMetadata {
                schema_version: STORED_PAIRING_METADATA_SCHEMA_VERSION,
                pairing_hmac_key: "ab".repeat(32),
            };

            storage.save_pairing_metadata(&metadata).unwrap();
            assert_eq!(storage.load_pairing_metadata().unwrap(), Some(metadata));
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(path.join("pairing_metadata.json"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }

            storage.clear_pairing_metadata().unwrap();
            assert!(storage.load_pairing_metadata().unwrap().is_none());
            cleanup(&path);
        }

        #[test]
        fn pairing_admission_stays_closed_until_generated_key_is_durable() {
            let (storage, path) = temp_storage();
            // Deterministically make the atomic temp path unwritable without
            // relying on platform permission behavior (or whether CI is root).
            let blocked_tmp = path.join("server_metadata.json.tmp");
            std::fs::create_dir(&blocked_tmp).unwrap();
            let mut app = crate::state::AppState {
                storage: Some(Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);
            assert!(!app.pairing_hmac_key_durable);
            let generated_key = app.pairing_hmac_key.clone();
            let state = Arc::new(std::sync::Mutex::new(app));
            assert!(crate::pairing::pairing_request_fingerprint_for_state(
                &state,
                "local_ble",
                &serde_json::json!({})
            )
            .is_err());

            std::fs::remove_dir(&blocked_tmp).unwrap();
            {
                let mut app = state.lock().unwrap();
                ensure_server_instance_id(&mut app);
                assert!(app.pairing_hmac_key_durable);
                assert_eq!(app.pairing_hmac_key, generated_key);
            }
            assert!(crate::pairing::pairing_request_fingerprint_for_state(
                &state,
                "local_ble",
                &serde_json::json!({})
            )
            .is_ok());

            let mut restarted = crate::state::AppState {
                storage: Some(Arc::new(FileStorage::new(path.to_str().unwrap()).unwrap())),
                ..Default::default()
            };
            ensure_server_instance_id(&mut restarted);
            assert!(restarted.pairing_hmac_key_durable);
            assert_eq!(restarted.pairing_hmac_key, generated_key);
            cleanup(&path);
        }

        #[test]
        fn load_persisted_state_generates_and_reuses_server_instance_id() {
            let (storage, path) = temp_storage();
            let mut app = crate::state::AppState {
                storage: Some(std::sync::Arc::new(storage)),
                ..Default::default()
            };

            load_persisted_state(&mut app);

            let generated_id = app.server_instance_id.clone();
            let generated_pairing_key = app.pairing_hmac_key.clone();
            assert!(generated_id.starts_with("srv-"));
            assert!(valid_pairing_hmac_key(&generated_pairing_key));
            assert!(app.pairing_hmac_key_durable);
            let persisted = FileStorage::new(path.to_str().unwrap())
                .unwrap()
                .load_server_metadata()
                .unwrap()
                .unwrap();
            assert_eq!(persisted.server_instance_id, generated_id);
            assert_eq!(persisted.legacy_pairing_hmac_key, None);
            let persisted_pairing = FileStorage::new(path.to_str().unwrap())
                .unwrap()
                .load_pairing_metadata()
                .unwrap()
                .unwrap();
            assert_eq!(
                persisted_pairing.pairing_hmac_key,
                generated_pairing_key.clone()
            );

            let mut restarted = crate::state::AppState {
                storage: Some(std::sync::Arc::new(
                    FileStorage::new(path.to_str().unwrap()).unwrap(),
                )),
                ..Default::default()
            };
            load_persisted_state(&mut restarted);
            assert_eq!(restarted.server_instance_id, generated_id);
            assert_eq!(restarted.pairing_hmac_key, generated_pairing_key);
            assert!(restarted.pairing_hmac_key_durable);

            cleanup(&path);
        }

        #[test]
        fn prerelease_combined_metadata_migrates_key_without_rotating_public_id() {
            let (storage, path) = temp_storage();
            let legacy_key = "de".repeat(32);
            std::fs::write(
                path.join("server_metadata.json"),
                serde_json::json!({
                    "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION,
                    "server_instance_id": "srv-prerelease-instance",
                    "pairing_hmac_key": legacy_key,
                })
                .to_string(),
            )
            .unwrap();
            let mut app = crate::state::AppState {
                storage: Some(Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert_eq!(app.server_instance_id, "srv-prerelease-instance");
            assert_eq!(app.pairing_hmac_key, legacy_key);
            assert!(app.pairing_hmac_key_durable);
            let public: serde_json::Value =
                serde_json::from_slice(&std::fs::read(path.join("server_metadata.json")).unwrap())
                    .unwrap();
            assert_eq!(public["server_instance_id"], "srv-prerelease-instance");
            assert!(public.get("pairing_hmac_key").is_none());
            let private = FileStorage::new(path.to_str().unwrap())
                .unwrap()
                .load_pairing_metadata()
                .unwrap()
                .unwrap();
            assert_eq!(private.pairing_hmac_key, legacy_key);
            cleanup(&path);
        }

        #[test]
        fn combined_metadata_migration_stays_closed_until_public_rewrite_is_durable() {
            let (storage, path) = temp_storage();
            let legacy_key = "ad".repeat(32);
            std::fs::write(
                path.join("server_metadata.json"),
                serde_json::json!({
                    "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION,
                    "server_instance_id": "srv-migration-fence",
                    "pairing_hmac_key": legacy_key,
                })
                .to_string(),
            )
            .unwrap();
            let blocked_server_tmp = path.join("server_metadata.json.tmp");
            std::fs::create_dir(&blocked_server_tmp).unwrap();
            let mut app = crate::state::AppState {
                storage: Some(Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert!(!app.pairing_hmac_key_durable);
            assert_eq!(app.pairing_hmac_key, legacy_key);
            assert_eq!(
                FileStorage::new(path.to_str().unwrap())
                    .unwrap()
                    .load_pairing_metadata()
                    .unwrap()
                    .unwrap()
                    .pairing_hmac_key,
                legacy_key
            );
            assert!(std::fs::read_to_string(path.join("server_metadata.json"))
                .unwrap()
                .contains("pairing_hmac_key"));

            std::fs::remove_dir(&blocked_server_tmp).unwrap();
            ensure_server_instance_id(&mut app);
            assert!(app.pairing_hmac_key_durable);
            assert!(!std::fs::read_to_string(path.join("server_metadata.json"))
                .unwrap()
                .contains("pairing_hmac_key"));
            cleanup(&path);
        }

        #[test]
        fn combined_metadata_migration_preserves_only_key_when_private_write_fails() {
            let (storage, path) = temp_storage();
            let legacy_key = "ac".repeat(32);
            let combined = serde_json::json!({
                "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION,
                "server_instance_id": "srv-private-write-fence",
                "pairing_hmac_key": legacy_key,
            })
            .to_string();
            std::fs::write(path.join("server_metadata.json"), &combined).unwrap();
            let blocked_pairing_tmp = path.join("pairing_metadata.json.tmp");
            std::fs::create_dir(&blocked_pairing_tmp).unwrap();
            let mut app = crate::state::AppState {
                storage: Some(Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert!(!app.pairing_hmac_key_durable);
            assert_eq!(
                std::fs::read_to_string(path.join("server_metadata.json")).unwrap(),
                combined,
                "the legacy key remains durable until the private write succeeds"
            );
            assert!(!path.join("pairing_metadata.json").exists());

            std::fs::remove_dir(&blocked_pairing_tmp).unwrap();
            ensure_server_instance_id(&mut app);
            assert!(app.pairing_hmac_key_durable);
            assert_eq!(app.pairing_hmac_key, legacy_key);
            assert!(!std::fs::read_to_string(path.join("server_metadata.json"))
                .unwrap()
                .contains("pairing_hmac_key"));
            cleanup(&path);
        }

        #[test]
        fn combined_metadata_migration_does_not_rewrite_public_file_after_pairing_fsync_error() {
            let legacy_key = "ae".repeat(32);
            let pairing_saves = Arc::new(std::sync::Mutex::new(Vec::new()));
            let server_saves = Arc::new(std::sync::Mutex::new(Vec::new()));
            let storage = PairingDirectorySyncFailureStorage {
                server_metadata: StoredServerMetadata {
                    schema_version: STORED_SERVER_METADATA_SCHEMA_VERSION,
                    server_instance_id: "srv-post-rename-fence".to_string(),
                    legacy_pairing_hmac_key: Some(legacy_key.clone()),
                },
                pairing_saves: pairing_saves.clone(),
                server_saves: server_saves.clone(),
            };
            let mut app = crate::state::AppState {
                storage: Some(Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert!(!app.pairing_hmac_key_durable);
            assert_eq!(app.pairing_hmac_key, legacy_key);
            assert_eq!(pairing_saves.lock().unwrap().len(), 1);
            assert!(
                server_saves.lock().unwrap().is_empty(),
                "the combined file must retain the only definitely durable key"
            );
        }

        #[test]
        fn rollback_scrub_removes_only_local_ble_compatibility_state() {
            let (_storage, path) = temp_storage();
            for directory in LOCAL_BLE_ROLLBACK_DIRS {
                std::fs::create_dir_all(path.join(directory).join("nested")).unwrap();
                std::fs::write(path.join(directory).join("nested/state.json"), "{}").unwrap();
            }
            for file in LOCAL_BLE_ROLLBACK_FILES {
                std::fs::write(path.join(file), "sensitive").unwrap();
            }
            std::fs::write(
                path.join("server_metadata.json"),
                r#"{"schema_version":2,"server_instance_id":"srv-keep"}"#,
            )
            .unwrap();
            std::fs::write(path.join("settings.json"), "keep").unwrap();

            scrub_local_ble_rollback_state(&path).unwrap();
            scrub_local_ble_rollback_state(&path).unwrap();

            for directory in LOCAL_BLE_ROLLBACK_DIRS {
                assert!(!path.join(directory).exists());
            }
            for file in LOCAL_BLE_ROLLBACK_FILES {
                assert!(!path.join(file).exists());
            }
            assert!(path.join("server_metadata.json").exists());
            assert_eq!(
                std::fs::read_to_string(path.join("settings.json")).unwrap(),
                "keep"
            );
            cleanup(&path);
        }

        #[test]
        fn rollback_scrub_rejects_unsafe_data_directories() {
            assert!(scrub_local_ble_rollback_state(std::path::Path::new("relative-data")).is_err());
            assert!(scrub_local_ble_rollback_state(std::path::Path::new("/")).is_err());
        }

        #[test]
        fn authority_rollback_handoff_retires_hue_routes_before_legacy_migration() {
            let (storage, path) = temp_storage();
            let hue_key = HubKey::new(crate::hub::HubType::new("hue"), "192.0.2.10");
            let ha_key = HubKey::new(
                crate::hub::HubType::new("home_assistant"),
                "http://ha.local",
            );

            let mut registry = crate::canonical::registry::CanonicalRegistry::new();
            let identity = crate::canonical::identity::DiscoveredIdentity {
                native_id: "hue-light-1".to_string(),
                room_id: None,
                room_name: None,
                name: "Counter Light".to_string(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: vec![crate::canonical::identity::HardwareId::mac(
                    "00:17:88:01:02:03:04:06",
                )],
                manufacturer: Some("Signify".to_string()),
                model: Some("LCT001".to_string()),
            };
            let canonical_id = match registry.resolve(&identity, &hue_key, 1000) {
                crate::canonical::registry::ResolveResult::Created { canonical_id } => canonical_id,
                other => panic!("unexpected resolve result: {other:?}"),
            };
            registry.get_mut(&canonical_id).unwrap().upsert_endpoint(
                ha_key.clone(),
                "light.counter".to_string(),
                1001,
                None,
            );

            let mut topology = crate::topology::RoomTopologyStore::new();
            let room_id = topology.create_room("Kitchen");
            assert!(topology.attach_device_user_override(&room_id, &canonical_id));
            topology.set_grouped_room_control_required(&hue_key, true);
            assert!(topology.upsert_managed_room_binding(
                &room_id,
                crate::topology::HubRoomBinding {
                    hub_key: hue_key.clone(),
                    hub_room_id: "obsolete-hue-room".to_string(),
                    control_id: "obsolete-grouped-light".to_string(),
                    light_device_ids: vec!["hue-light-1".to_string()],
                },
            ));
            assert!(topology.upsert_room_binding(
                &room_id,
                crate::topology::HubRoomBinding {
                    hub_key: ha_key.clone(),
                    hub_room_id: "kitchen-area".to_string(),
                    control_id: "kitchen-area".to_string(),
                    light_device_ids: vec!["light.counter".to_string()],
                },
            ));
            storage
                .save_authority_state(&StoredAuthorityState::new(
                    serde_json::to_value(&registry).unwrap(),
                    serde_json::to_value(&topology).unwrap(),
                ))
                .unwrap();

            // Prove the handoff refreshes divergent legacy mirrors from the
            // combined source of truth instead of preserving a mixed generation.
            let mut stale_topology = crate::topology::RoomTopologyStore::new();
            stale_topology.create_room("Stale Mirror");
            storage
                .save_topology(&serde_json::to_value(stale_topology).unwrap())
                .unwrap();
            storage
                .save_hub_registry_for(
                    &hue_key,
                    &serde_json::json!({"rooms": [{"id": "obsolete-hue-room"}]}),
                )
                .unwrap();

            prepare_authority_state_for_binary_rollback(&path, std::slice::from_ref(&hue_key))
                .unwrap();
            prepare_authority_state_for_binary_rollback(&path, std::slice::from_ref(&hue_key))
                .unwrap();

            assert!(storage.load_authority_state().unwrap().is_none());
            assert!(storage.load_hub_registry_for(&hue_key).unwrap().is_none());
            let legacy_registry: crate::canonical::registry::CanonicalRegistry =
                serde_json::from_value(storage.load_canonical_registry().unwrap().unwrap())
                    .unwrap();
            assert!(legacy_registry.get(&canonical_id).is_some());
            let mut legacy_topology: crate::topology::RoomTopologyStore =
                serde_json::from_value(storage.load_topology().unwrap().unwrap()).unwrap();
            legacy_topology.rebuild_indices();
            assert!(!legacy_topology.grouped_room_control_is_required(&hue_key));
            assert!(!legacy_topology.references_hub_key(&hue_key));
            let room = legacy_topology.get(&room_id).unwrap();
            assert!(room
                .hub_room_bindings
                .iter()
                .any(|binding| binding.hub_key == ha_key));
            assert!(room
                .hub_room_bindings
                .iter()
                .all(|binding| binding.hub_key != hue_key));

            // Emulate the older binary changing a legacy mirror, then prove a
            // later current binary consumes that change instead of reviving the
            // retired combined generation.
            legacy_topology.get_mut(&room_id).unwrap().name = "Downgraded Kitchen".to_string();
            storage
                .save_topology(&serde_json::to_value(&legacy_topology).unwrap())
                .unwrap();
            let storage = Arc::new(storage);
            let mut app = crate::state::AppState {
                storage: Some(storage.clone()),
                ..Default::default()
            };
            load_persisted_state(&mut app);
            assert_eq!(
                app.topology.get(&room_id).unwrap().name,
                "Downgraded Kitchen"
            );
            assert!(storage.load_authority_state().unwrap().is_some());
            cleanup(&path);
        }

        #[test]
        fn authority_rollback_handoff_preserves_corrupt_combined_evidence() {
            let (storage, path) = temp_storage();
            let hue_key = HubKey::new(crate::hub::HubType::new("hue"), "192.0.2.10");
            storage
                .save_authority_state(&StoredAuthorityState::new(
                    serde_json::to_value(crate::canonical::registry::CanonicalRegistry::new())
                        .unwrap(),
                    serde_json::to_value(crate::topology::RoomTopologyStore::new()).unwrap(),
                ))
                .unwrap();
            storage
                .save_hub_registry_for(&hue_key, &serde_json::json!({"keep": true}))
                .unwrap();
            std::fs::write(path.join("authority_state.json"), b"{").unwrap();
            let canonical_before = std::fs::read(path.join("canonical_registry.json")).unwrap();
            let topology_before = std::fs::read(path.join("topology.json")).unwrap();

            assert!(prepare_authority_state_for_binary_rollback(
                &path,
                std::slice::from_ref(&hue_key)
            )
            .is_err());

            assert_eq!(
                std::fs::read(path.join("authority_state.json")).unwrap(),
                b"{"
            );
            assert_eq!(
                std::fs::read(path.join("canonical_registry.json")).unwrap(),
                canonical_before
            );
            assert_eq!(
                std::fs::read(path.join("topology.json")).unwrap(),
                topology_before
            );
            assert_eq!(
                storage.load_hub_registry_for(&hue_key).unwrap(),
                Some(serde_json::json!({"keep": true}))
            );
            cleanup(&path);
        }

        #[test]
        fn future_server_metadata_is_preserved_and_uses_separate_pairing_key() {
            let (storage, path) = temp_storage();
            storage
                .save_pairing_metadata(&StoredPairingMetadata {
                    schema_version: STORED_PAIRING_METADATA_SCHEMA_VERSION,
                    pairing_hmac_key: "cd".repeat(32),
                })
                .unwrap();
            let raw = serde_json::json!({
                "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION + 1,
                "server_instance_id": "srv-future-instance",
                "future_field": {"must": "survive"},
            })
            .to_string();
            std::fs::write(path.join("server_metadata.json"), &raw).unwrap();
            let mut app = crate::state::AppState {
                storage: Some(std::sync::Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert_eq!(app.server_instance_id, "srv-future-instance");
            assert_eq!(app.pairing_hmac_key, "cd".repeat(32));
            assert!(app.pairing_hmac_key_durable);
            assert_eq!(
                std::fs::read_to_string(path.join("server_metadata.json")).unwrap(),
                raw
            );
            cleanup(&path);
        }

        #[test]
        fn future_server_metadata_without_valid_key_fails_pairing_closed() {
            let (storage, path) = temp_storage();
            let raw = serde_json::json!({
                "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION + 1,
                "server_instance_id": "srv-future-without-key",
                "future_field": true,
            })
            .to_string();
            std::fs::write(path.join("server_metadata.json"), &raw).unwrap();
            let mut app = crate::state::AppState {
                storage: Some(std::sync::Arc::new(storage)),
                ..Default::default()
            };

            ensure_server_instance_id(&mut app);

            assert_eq!(app.server_instance_id, "srv-future-without-key");
            assert!(!app.pairing_hmac_key_durable);
            assert_eq!(
                std::fs::read_to_string(path.join("server_metadata.json")).unwrap(),
                raw
            );
            cleanup(&path);
        }

        #[test]
        fn remote_access_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let config = crate::remote_access::StoredRemoteAccessConfig {
                schema_version: 1,
                enabled: true,
                hostname: "hub.devices.rhythm.lighting".into(),
                connector_token: "connector-secret".into(),
                tunnel_id: Some("tunnel-id".into()),
                tunnel_name: Some("tunnel-name".into()),
                updated_at_epoch_ms: 123,
            };

            storage.save_remote_access_config(&config).unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mode = std::fs::metadata(path.join("remote_access.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(mode, 0o600);
            }
            let loaded = storage.load_remote_access_config().unwrap();
            assert_eq!(loaded, Some(config));

            std::fs::create_dir_all(path.join("cloudflared")).unwrap();
            std::fs::write(path.join("cloudflared").join("connector_token"), "secret").unwrap();
            std::fs::write(path.join("cloudflared").join("hostname"), "host").unwrap();
            std::fs::write(path.join("support_bundle_jobs.json"), "secret job").unwrap();

            storage.clear_remote_access_config().unwrap();
            assert!(storage.load_remote_access_config().unwrap().is_none());
            assert!(!path.join("cloudflared").join("connector_token").exists());
            assert!(!path.join("cloudflared").join("hostname").exists());
            cleanup(&path);
        }

        #[test]
        fn activity_cloud_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let config = crate::activity_cloud::StoredActivityCloudConfig {
                schema_version: 1,
                enabled: true,
                ingest_url: "https://example.test/functions/v1/server-activity-ingest".into(),
                upload_token: "device-secret".into(),
                home_id: "home-1".into(),
                hub_id: "hub-1".into(),
                token_id: Some("token-1".into()),
                server_instance_id: Some("srv-1".into()),
                upload_status: Some("ok".into()),
                last_upload_attempt_epoch_ms: Some(100),
                last_upload_success_epoch_ms: Some(101),
                last_upload_failure_epoch_ms: None,
                last_upload_http_status: Some(200),
                last_upload_error: None,
                auth_failed_at_epoch_ms: None,
                updated_at_epoch_ms: 123,
            };

            storage.save_activity_cloud_config(&config).unwrap();
            let loaded = storage.load_activity_cloud_config().unwrap();
            assert_eq!(loaded, Some(config));

            storage.clear_activity_cloud_config().unwrap();
            assert!(storage.load_activity_cloud_config().unwrap().is_none());
            cleanup(&path);
        }

        #[test]
        fn light_usage_ledger_save_load_and_clear_roundtrip() {
            let (storage, path) = temp_storage();
            let ledger = crate::light_usage::LightUsageLedger::default();

            storage.save_light_usage_ledger(&ledger).unwrap();
            let loaded = storage.load_light_usage_ledger().unwrap().unwrap();
            assert_eq!(
                loaded.schema_version,
                crate::light_usage::LIGHT_USAGE_SCHEMA_VERSION
            );
            assert!(loaded.segments.is_empty());

            storage.clear_light_usage_ledger().unwrap();
            assert!(storage.load_light_usage_ledger().unwrap().is_none());
            cleanup(&path);
        }

        #[test]
        fn maximum_light_usage_ledger_remains_persistable_across_checkpoints() {
            use crate::light_usage::{
                LightUsageLedger, LightUsageSegment, LightUsageSourceSummary,
                LightUsageSubjectKind, LIGHT_USAGE_LEDGER_BYTES_LIMIT, LIGHT_USAGE_SEGMENT_LIMIT,
            };

            let (storage, path) = temp_storage();
            let mut ledger = LightUsageLedger::default();
            for index in 0..LIGHT_USAGE_SEGMENT_LIMIT {
                let segment_id = format!("{index:032x}");
                let subject_id = format!("{index:08x}{}", "x".repeat(152));
                ledger.segment_order.push_back(segment_id.clone());
                ledger.segments.insert(
                    segment_id.clone(),
                    LightUsageSegment {
                        segment_id,
                        subject_id,
                        subject_kind: LightUsageSubjectKind::RoomAggregate,
                        usage_date: "9999-12-31".into(),
                        revision: u64::MAX,
                        synced_revision: u64::MAX - 1,
                        on_ms: u64::MAX,
                        covered_ms: u64::MAX,
                        transition_uncertainty_ms: u64::MAX,
                        observation_count: u64::MAX,
                        transition_count: u64::MAX,
                        first_observed_at_epoch_ms: u64::MAX,
                        last_observed_at_epoch_ms: u64::MAX,
                        source_summary: LightUsageSourceSummary {
                            periodic: u64::MAX,
                            sync_poll: u64::MAX,
                            live_subscription: u64::MAX,
                            authoritative_refresh: u64::MAX,
                        },
                    },
                );
            }

            storage.save_light_usage_ledger(&ledger).unwrap();
            let ledger_path = path.join("light_usage_ledger.json");
            assert!(
                std::fs::metadata(&ledger_path).unwrap().len() <= LIGHT_USAGE_LEDGER_BYTES_LIMIT
            );

            let mut loaded = storage.load_light_usage_ledger().unwrap().unwrap();
            loaded.last_checkpoint_epoch_ms = Some(u64::MAX);
            loaded.segments.values_mut().next().unwrap().revision = u64::MAX;
            storage.save_light_usage_ledger(&loaded).unwrap();
            assert!(
                std::fs::metadata(&ledger_path).unwrap().len() <= LIGHT_USAGE_LEDGER_BYTES_LIMIT
            );
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
                    light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                    active_mode: RhythmMode::Sleep,
                    last_active_mode_cause: ModeChangeCause::Manual,
                    last_active_mode_transition_id: None,
                    last_active_mode_change_utc_ms: Some(123),
                    modes: rhythm_core::default_mode_configs(),
                    mode_transitions: rhythm_core::default_mode_transition_configs(),
                    auto_update: true,
                    update_channel: None,
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
            storage.save_scenes(&StoredScenes::default()).unwrap();
            let mut runtime_state = StoredLightRuntimeState::default();
            runtime_state.insert(
                "room-1".into(),
                BTreeMap::from([(
                    "adaptive".into(),
                    BTreeMap::from([("last_plan".into(), serde_json::json!("off"))]),
                )]),
            );
            storage.save_light_runtime_state(&runtime_state).unwrap();
            storage
                .save_light_activity_history(&crate::activity::LightActivityHistory::default())
                .unwrap();
            storage
                .save_light_usage_ledger(&crate::light_usage::LightUsageLedger::default())
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
                .save_authority_state(&StoredAuthorityState::new(
                    serde_json::json!({"devices": {}, "triage": {"entries": []}}),
                    serde_json::json!({"rooms": {}}),
                ))
                .unwrap();
            storage
                .save_commissioning_wifi_credentials(&crate::provisioning::WifiCredentials {
                    ssid: "RhythmNet".into(),
                    password: "secret".into(),
                })
                .unwrap();
            storage
                .save_api_auth(&crate::auth::StoredApiAuth::default())
                .unwrap();
            storage
                .save_remote_access_config(&crate::remote_access::StoredRemoteAccessConfig {
                    schema_version: 1,
                    enabled: true,
                    hostname: "hub.devices.rhythm.lighting".into(),
                    connector_token: "connector-secret".into(),
                    tunnel_id: Some("tunnel-id".into()),
                    tunnel_name: Some("tunnel-name".into()),
                    updated_at_epoch_ms: 123,
                })
                .unwrap();
            storage
                .save_activity_cloud_config(&crate::activity_cloud::StoredActivityCloudConfig {
                    schema_version: 1,
                    enabled: true,
                    ingest_url: "https://example.test/functions/v1/server-activity-ingest".into(),
                    upload_token: "device-secret".into(),
                    home_id: "home-1".into(),
                    hub_id: "hub-1".into(),
                    token_id: Some("token-1".into()),
                    server_instance_id: Some("srv-1".into()),
                    upload_status: Some("ok".into()),
                    last_upload_attempt_epoch_ms: Some(100),
                    last_upload_success_epoch_ms: Some(101),
                    last_upload_failure_epoch_ms: None,
                    last_upload_http_status: Some(200),
                    last_upload_error: None,
                    auth_failed_at_epoch_ms: None,
                    updated_at_epoch_ms: 123,
                })
                .unwrap();
            storage
                .save_server_metadata(&StoredServerMetadata {
                    schema_version: STORED_SERVER_METADATA_SCHEMA_VERSION,
                    server_instance_id: "srv-reset-stable".to_string(),
                    legacy_pairing_hmac_key: None,
                })
                .unwrap();
            storage
                .save_pairing_metadata(&StoredPairingMetadata {
                    schema_version: STORED_PAIRING_METADATA_SCHEMA_VERSION,
                    pairing_hmac_key: "ab".repeat(32),
                })
                .unwrap();
            storage
                .save_pairing_history(&crate::pairing::PairingHistory {
                    schema_version: crate::pairing::PAIRING_HISTORY_SCHEMA_VERSION,
                    entries: Vec::new(),
                    pairing_results: Vec::new(),
                })
                .unwrap();
            std::fs::create_dir_all(path.join("cloudflared")).unwrap();
            std::fs::write(path.join("cloudflared").join("connector_token"), "secret").unwrap();
            std::fs::write(path.join("cloudflared").join("hostname"), "host").unwrap();
            std::fs::create_dir_all(path.join("matter").join("captures")).unwrap();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::create_dir_all(path.join("hue")).unwrap();
            std::fs::create_dir_all(path.join("hue_ble")).unwrap();
            std::fs::create_dir_all(path.join("local_ble")).unwrap();
            std::fs::create_dir_all(path.join("aidot_ble")).unwrap();
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
            std::fs::write(path.join("hue").join("controller-ownership-v1.json"), "{}").unwrap();
            std::fs::write(path.join("hue_ble").join("devices.json"), "{}").unwrap();
            std::fs::write(path.join("local_ble").join("devices.json"), "{}").unwrap();
            std::fs::write(path.join("aidot_ble").join("devices.json"), "{}").unwrap();

            storage.clear_factory_reset_state().unwrap();

            for name in [
                "rooms.json",
                "light_profiles.json",
                "location.json",
                "settings.json",
                "scenes.json",
                "motion_timers.json",
                "light_runtime_state.json",
                "activity_history.json",
                "light_usage_ledger.json",
                "pairing_history.json",
                "pairing_metadata.json",
                "hub_credentials.json",
                "authority_state.json",
                "canonical_registry.json",
                "topology.json",
                "commissioning_wifi.json",
                "auth.json",
                "activity_cloud.json",
                "remote_access.json",
                "support_bundle_jobs.json",
                "cloudflared/connector_token",
                "cloudflared/hostname",
                "hub_registry_hue_192_168_1_2.json",
            ] {
                assert!(!path.join(name).exists(), "{} should be removed", name);
            }
            let metadata = storage.load_server_metadata().unwrap().unwrap();
            assert_eq!(metadata.server_instance_id, "srv-reset-stable");
            assert_eq!(metadata.legacy_pairing_hmac_key, None);
            assert!(
                !path.join("matter").exists(),
                "integration runtime state should be removed"
            );
            assert!(
                !path.join("hue").exists(),
                "Hue controller recovery state should be removed after release"
            );
            assert!(
                !path.join("hue_ble").exists(),
                "adapter-bound Hue BLE metadata should be removed"
            );
            assert!(
                !path.join("local_ble").exists(),
                "appliance-local BLE profile metadata should be removed"
            );
            assert!(
                !path.join("aidot_ble").exists(),
                "prerelease vendor-shaped BLE metadata should be removed"
            );

            cleanup(&path);
        }

        #[test]
        fn factory_reset_deletes_future_server_metadata_instead_of_downgrading_it() {
            let (storage, path) = temp_storage();
            std::fs::write(
                path.join("server_metadata.json"),
                serde_json::json!({
                    "schema_version": STORED_SERVER_METADATA_SCHEMA_VERSION + 1,
                    "server_instance_id": "srv-future-reset",
                    "pairing_hmac_key": "ef".repeat(32),
                    "future_secret": "must-not-survive-reset",
                })
                .to_string(),
            )
            .unwrap();

            storage.clear_factory_reset_state().unwrap();

            assert!(!path.join("server_metadata.json").exists());
            cleanup(&path);
        }

        #[test]
        fn integration_backup_files_require_secrets_and_skip_runtime_noise() {
            let (storage, path) = temp_storage();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::create_dir_all(path.join("matter").join("captures")).unwrap();
            std::fs::write(path.join("matter").join("fabric-identity.json"), "fabric").unwrap();
            std::fs::create_dir_all(path.join("hue")).unwrap();
            std::fs::write(
                path.join("hue").join("controller-ownership-v1.json"),
                "hue-baseline",
            )
            .unwrap();
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
            std::fs::create_dir_all(path.join("hue_ble")).unwrap();
            std::fs::write(path.join("hue_ble").join("devices.json"), "local-only").unwrap();

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
                    ("hue/controller-ownership-v1.json", "hue-baseline", true),
                    ("matter/chip/controller-storage.json", "controller", true),
                    ("matter/chip/devices.json", "devices", true),
                    ("matter/fabric-identity.json", "fabric", true),
                ]
            );
            assert!(
                files.iter().all(|file| !file.path.starts_with("hue_ble/")),
                "Hue BLE metadata cannot be restored without adapter-bound BlueZ link keys"
            );

            cleanup(&path);
        }

        #[test]
        fn integration_state_file_round_trips_durably_and_rejects_escape() {
            let (storage, path) = temp_storage();
            let state_path = "hue/controller-ownership-v1.json";

            assert_eq!(
                storage.load_integration_state_file(state_path).unwrap(),
                None
            );
            storage
                .save_integration_state_file(state_path, r#"{"phase":"captured"}"#)
                .unwrap();
            assert_eq!(
                storage.load_integration_state_file(state_path).unwrap(),
                Some(r#"{"phase":"captured"}"#.to_string())
            );
            assert!(storage
                .save_integration_state_file("hue/../credentials.json", "escape")
                .is_err());
            assert!(storage
                .load_integration_state_file("unsupported/state.json")
                .is_err());

            storage.delete_integration_state_file(state_path).unwrap();
            assert_eq!(
                storage.load_integration_state_file(state_path).unwrap(),
                None
            );
            cleanup(&path);
        }

        #[test]
        fn restore_integration_backup_files_replaces_portable_integration_state() {
            let (storage, path) = temp_storage();
            std::fs::create_dir_all(path.join("matter").join("chip")).unwrap();
            std::fs::write(path.join("matter").join("stale.json"), "stale").unwrap();
            std::fs::create_dir_all(path.join("hue")).unwrap();
            std::fs::write(path.join("hue").join("stale.json"), "stale-hue").unwrap();

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
                    crate::bundle::BackupIntegrationFile {
                        path: "hue/controller-ownership-v1.json".to_string(),
                        content: "hue-baseline".to_string(),
                        secret: true,
                    },
                ])
                .unwrap();

            assert!(!path.join("matter").join("stale.json").exists());
            assert!(!path.join("hue").join("stale.json").exists());
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
            assert_eq!(
                std::fs::read_to_string(path.join("hue").join("controller-ownership-v1.json"))
                    .unwrap(),
                "hue-baseline"
            );

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(
                    std::fs::metadata(path.join("hue").join("controller-ownership-v1.json"))
                        .unwrap()
                        .permissions()
                        .mode()
                        & 0o777,
                    0o600
                );
            }

            storage.restore_integration_backup_files(&[]).unwrap();
            assert!(!path.join("matter").exists());
            assert!(!path.join("hue").exists());

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
        fn hue_integration_backup_validation_rejects_malformed_nested_state() {
            let (storage, path) = temp_storage();
            let valid = valid_hue_ownership_backup_file();
            storage
                .validate_integration_backup_files(std::slice::from_ref(&valid))
                .unwrap();

            for (field, invalid_value) in [
                ("managed_scenes", serde_json::json!([])),
                (
                    "receipts",
                    serde_json::json!({
                        "operation-1": {
                            "operation_id": "operation-1",
                            "api": "v2",
                            "action": "delete",
                            "resource_type": "room",
                            "original_resource_id": "old",
                            "status": "not-a-status",
                            "attempt": 1
                        }
                    }),
                ),
            ] {
                let mut content: serde_json::Value = serde_json::from_str(&valid.content).unwrap();
                content[field] = invalid_value;
                let invalid = crate::bundle::BackupIntegrationFile {
                    content: content.to_string(),
                    ..valid.clone()
                };
                let error = storage
                    .validate_integration_backup_files(&[invalid])
                    .unwrap_err();
                assert!(
                    error
                        .to_string()
                        .contains("invalid Hue controller ownership manifest"),
                    "unexpected validation error for {field}: {error:#}"
                );
            }
            cleanup(&path);
        }

        #[test]
        fn restore_integration_backup_files_rejects_empty_absolute_and_unsupported_paths() {
            let (storage, path) = temp_storage();
            for candidate in ["", "/matter/fabric.json", "hue_ble/fabric.json"] {
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

            assert!(storage.load_light_runtime_state().unwrap().is_none());
            std::fs::write(path.join("app_runtime_state.json"), "{}").unwrap();
            assert_eq!(
                storage.load_light_runtime_state().unwrap(),
                Some(StoredLightRuntimeState::default())
            );
            std::fs::write(path.join("light_runtime_state.json"), "{").unwrap();
            assert!(storage.load_light_runtime_state().unwrap().is_none());
            std::fs::remove_file(path.join("light_runtime_state.json")).unwrap();
            std::fs::write(path.join("app_runtime_state.json"), "{").unwrap();
            assert!(storage.load_light_runtime_state().unwrap().is_none());
            std::fs::remove_file(path.join("app_runtime_state.json")).unwrap();

            assert!(storage.load_light_activity_history().unwrap().is_none());
            std::fs::write(
                path.join("activity_history.json"),
                serde_json::to_string(&crate::activity::LightActivityHistory {
                    schema_version: 99,
                    activities: Vec::new(),
                })
                .unwrap(),
            )
            .unwrap();
            assert_eq!(
                storage
                    .load_light_activity_history()
                    .unwrap()
                    .unwrap()
                    .schema_version,
                1
            );
            std::fs::write(path.join("activity_history.json"), "{").unwrap();
            assert!(storage.load_light_activity_history().unwrap().is_none());

            assert!(storage.load_activity_cloud_config().unwrap().is_none());
            std::fs::write(path.join("activity_cloud.json"), "{").unwrap();
            assert!(storage.load_activity_cloud_config().unwrap().is_none());

            assert!(storage.load_remote_access_config().unwrap().is_none());
            std::fs::write(path.join("remote_access.json"), "{").unwrap();
            assert!(storage.load_remote_access_config().unwrap().is_none());

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
        fn corrupt_pairing_reconciliation_document_fails_closed() {
            let (storage, path) = temp_storage();
            assert!(storage.load_pairing_history().unwrap().is_none());
            std::fs::write(path.join("pairing_history.json"), "{").unwrap();

            let error = storage.load_pairing_history().unwrap_err();
            assert!(error
                .to_string()
                .contains("pairing history exists but is unreadable"));
            assert_eq!(
                std::fs::read_to_string(path.join("pairing_history.json")).unwrap(),
                "{"
            );
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
                light_runtime: crate::light_runtime::LightRuntimeKind::default(),
                active_mode: RhythmMode::Day,
                last_active_mode_cause: ModeChangeCause::default(),
                last_active_mode_transition_id: None,
                last_active_mode_change_utc_ms: None,
                modes: Vec::new(),
                mode_transitions: Vec::new(),
                auto_update: true,
                update_channel: None,
            }
        }

        #[test]
        fn stored_settings_without_light_runtime_defaults_to_rhythm_adaptive() {
            let settings: StoredSettings = serde_json::from_value(serde_json::json!({
                "power_save": false,
                "light_breaker_enabled": true,
                "active_mode": "day",
                "last_active_mode_cause": "manual",
                "modes": [],
                "mode_transitions": [],
                "auto_update": true
            }))
            .unwrap();

            assert_eq!(
                settings.light_runtime,
                crate::light_runtime::LightRuntimeKind::rhythm_adaptive()
            );
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
