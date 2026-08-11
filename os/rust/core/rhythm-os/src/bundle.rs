//! Profile-bundle and backup bundle schemas.
//!
//! `ProfileBundle` is the portable format for a user's active portable profile
//! settings.
//! `BackupBundle` captures installation-specific state for restore workflows.

use rhythm_core::{
    LightProfileConfig, ModeChangeCause, ModeConfig, ModeTransitionConfig, RhythmMode, Room,
    RoomManager, RoomModeState, RoomProfileSettings,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::canonical::identity::HubKey;
use crate::canonical::registry::CanonicalRegistry;
use crate::hub::HubType;
use crate::scenes::SceneDefinition;
use crate::storage::StoredLocation;
use crate::topology::RoomTopologyStore;

pub const PROFILE_BUNDLE_SCHEMA_VERSION: u32 = 1;
pub const LEGACY_BACKUP_SCHEMA_VERSION: u32 = 1;
/// Backup v2 added explicit per-room motion admission state.
pub const MOTION_ADMISSION_BACKUP_SCHEMA_VERSION: u32 = 2;
/// Backup v3 additionally carries the immutable Hue controller takeover
/// record in secret integration files. Older appliances must reject it: they
/// cannot safely release a bridge or preserve its recovery baseline.
pub const BACKUP_BUNDLE_SCHEMA_VERSION: u32 = 3;

fn default_profile_schema_version() -> u32 {
    PROFILE_BUNDLE_SCHEMA_VERSION
}

fn default_backup_schema_version() -> u32 {
    LEGACY_BACKUP_SCHEMA_VERSION
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleKind {
    #[serde(alias = "share_bundle", alias = "configuration_bundle")]
    ProfileBundle,
    BackupBundle,
}

fn default_profile_bundle_kind() -> BundleKind {
    BundleKind::ProfileBundle
}

fn default_backup_bundle_kind() -> BundleKind {
    BundleKind::BackupBundle
}

fn default_legacy_backup_topology() -> RoomTopologyStore {
    // Current backup generation always writes `installation.topology`. Its
    // absence therefore identifies an older compatibility payload rather than
    // a newly created topology, and must cross the one-time Hue policy
    // migration when restored.
    RoomTopologyStore::legacy_empty()
}

/// Portable lighting behavior captured in the profile bundle.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfileBundleData {
    #[serde(default)]
    pub power_save: bool,
    #[serde(default)]
    pub profiles: Vec<LightProfileConfig>,
    #[serde(default)]
    pub scenes: Vec<SceneDefinition>,
    #[serde(default)]
    pub mode_transitions: Vec<ModeTransitionConfig>,
}

/// Portable top-level profile bundle.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProfileBundle {
    #[serde(default = "default_profile_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_profile_bundle_kind")]
    pub kind: BundleKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(alias = "share", alias = "configuration")]
    pub profile: ProfileBundleData,
}

/// Import accepts either a full profile bundle or a bare profile payload.
#[derive(Clone, Debug, Deserialize)]
#[serde(untagged)]
pub enum ProfileBundleImportPayload {
    Bundle(ProfileBundle),
    Profile(ProfileBundleData),
}

impl ProfileBundleImportPayload {
    pub fn into_bundle(self) -> ProfileBundle {
        match self {
            Self::Bundle(bundle) => bundle,
            Self::Profile(profile) => ProfileBundle {
                schema_version: PROFILE_BUNDLE_SCHEMA_VERSION,
                kind: BundleKind::ProfileBundle,
                name: None,
                description: None,
                profile,
            },
        }
    }
}

/// Portable per-room preferences included in backup configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupConfigurationRoom {
    pub id: String,
    pub name: String,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub state: RoomModeState,
    #[serde(
        rename = "room_profile",
        default,
        skip_serializing_if = "RoomProfileSettings::is_empty"
    )]
    pub room_profile: RoomProfileSettings,
}

impl From<&Room> for BackupConfigurationRoom {
    fn from(room: &Room) -> Self {
        Self {
            id: room.id.clone(),
            name: room.name.clone(),
            rhythm_enabled: room.rhythm_enabled,
            disabled: room.disabled,
            state: RoomModeState::from_flags(room.hard_off, room.soft_off, room.mood_active, false),
            room_profile: room.profile_settings.clone(),
        }
    }
}

/// Backed-up lighting and room behavior stored with installation restores.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupConfiguration {
    #[serde(default)]
    pub power_save: bool,
    #[serde(default = "default_light_breaker_enabled")]
    pub light_breaker_enabled: bool,
    #[serde(default)]
    pub active_mode: RhythmMode,
    #[serde(default)]
    pub profiles: Vec<LightProfileConfig>,
    #[serde(default)]
    pub mode_configs: Vec<ModeConfig>,
    #[serde(default)]
    pub mode_transitions: Vec<ModeTransitionConfig>,
    #[serde(default)]
    pub scenes: Vec<SceneDefinition>,
    #[serde(default)]
    pub rooms: Vec<BackupConfigurationRoom>,
}

fn default_light_breaker_enabled() -> bool {
    false
}

impl Default for BackupConfiguration {
    fn default() -> Self {
        Self {
            power_save: false,
            light_breaker_enabled: default_light_breaker_enabled(),
            active_mode: RhythmMode::default(),
            profiles: Vec::new(),
            mode_configs: Vec::new(),
            mode_transitions: Vec::new(),
            scenes: Vec::new(),
            rooms: Vec::new(),
        }
    }
}

/// Backup copy of per-hub credentials.
///
/// The credential payload can be omitted for redacted backups.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupHubCredentials {
    #[serde(default)]
    pub hub_type: Option<HubType>,
    #[serde(default)]
    pub address: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// Serialized hub registry snapshot for one hub instance.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupHubRegistry {
    pub hub_key: HubKey,
    pub snapshot: Value,
}

/// Integration-owned durable file included in full-secret backups.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BackupIntegrationFile {
    /// Normalized path relative to the storage root, e.g.
    /// `matter/fabric-identity.json`.
    pub path: String,
    pub content: String,
    #[serde(default)]
    pub secret: bool,
}

/// Installation-specific data for restore workflows.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupInstallation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<StoredLocation>,
    #[serde(default)]
    pub rooms: RoomManager,
    #[serde(default = "default_legacy_backup_topology")]
    pub topology: RoomTopologyStore,
    #[serde(default)]
    pub canonical_registry: CanonicalRegistry,
    #[serde(default)]
    pub hub_credentials: Vec<BackupHubCredentials>,
    #[serde(default)]
    pub hub_registries: Vec<BackupHubRegistry>,
    #[serde(default)]
    pub integration_files: Vec<BackupIntegrationFile>,
}

/// Restorable runtime mode state captured at backup time.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupRuntimeState {
    #[serde(default)]
    pub active_mode: RhythmMode,
    #[serde(default)]
    pub last_change_cause: ModeChangeCause,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_transition_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_change_epoch_ms: Option<i64>,
}

/// Full installation backup.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct BackupBundle {
    #[serde(default = "default_backup_schema_version")]
    pub schema_version: u32,
    #[serde(default = "default_backup_bundle_kind")]
    pub kind: BundleKind,
    pub created_at: String,
    #[serde(default)]
    pub secrets_included: bool,
    pub configuration: BackupConfiguration,
    pub installation: BackupInstallation,
    pub runtime_state: BackupRuntimeState,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::topology::ExternalRoomAutomationOwner;

    #[test]
    fn omitted_backup_topology_defaults_to_legacy_hue_policy() {
        let mut installation: BackupInstallation = serde_json::from_value(serde_json::json!({}))
            .expect("legacy installation should deserialize without topology");
        let hub_key = HubKey::new(HubType::new(HubType::HUE), "192.0.2.30");

        let migration = installation
            .topology
            .migrate_legacy_external_room_automation_policy(std::slice::from_ref(&hub_key));

        assert!(migration.external_automation_policy_version_advanced);
        assert_eq!(
            installation
                .topology
                .external_room_automation_owner("future-room", &hub_key),
            Some(ExternalRoomAutomationOwner::Rhythm)
        );
    }
}
