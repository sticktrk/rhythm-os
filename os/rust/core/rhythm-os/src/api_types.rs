//! Serde response structs for all API endpoints.
//!
//! Centralizes JSON shapes so handlers use typed structs instead of
//! `format!()` string concatenation. Field names match the SSE types
//! in `server_event.rs` — one canonical naming convention.

use rhythm_core::{
    runtime::hub_registry::DeviceType, LightNodeKind, LightProfileConfig, LightProfileNodeOverride,
    ModeChangeCause, ModeConfig, ModeTransitionConfig, RhythmMode, RoomModeState,
    RoomProfileSettings, TimerSetting,
};
use serde::Serialize;
use std::collections::BTreeMap;

use crate::app_runtime::LightingRuntimeKind;
use crate::canonical::triage::{TriageKind, TriageStatus};
use crate::scenes::SceneDefinition;
use crate::topology::{DevicePlacement, HubRoomBinding, InputBinding, NodeControlKind};

// ---------------------------------------------------------------------------
// Room state structs
// ---------------------------------------------------------------------------

/// Observed power metadata for a room or node.
#[derive(Clone, Debug, Serialize)]
pub struct ObservedPowerDto {
    pub lights_on: bool,
    pub fresh: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_at_epoch_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// API representation of room profile settings with legacy mood fields resolved.
#[derive(Clone, Debug, Serialize)]
pub struct RoomProfileSettingsDto {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub profile_id: Option<String>,
    pub mood_enabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mood_profile_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mood_scene_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fade_ms: Option<TimerSetting>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_timeout_secs: Option<TimerSetting>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub profile_overrides: BTreeMap<String, LightProfileNodeOverride>,
}

impl RoomProfileSettingsDto {
    pub fn from_settings(settings: &RoomProfileSettings, mood_enabled: bool) -> Self {
        Self {
            profile_id: settings.profile_id.clone(),
            mood_enabled,
            mood_profile_id: settings.mood_profile_id.clone(),
            mood_scene_id: settings.mood_scene_id.clone(),
            fade_ms: settings.fade_ms.clone(),
            motion_timeout_secs: settings.motion_timeout_secs.clone(),
            profile_overrides: settings.profile_overrides.clone(),
        }
    }
}

/// Core room rhythm state — used by mutation responses and as the base
/// for poll and full-state variants.
#[derive(Clone, Debug, Serialize)]
pub struct RoomRhythmState {
    pub id: String,
    /// Which hub types have lights in this room (e.g. ["hue"], ["hue", "matter"]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    pub state: RoomModeState,
    pub rhythm_enabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    pub lights_on: bool,
    pub observed_power: ObservedPowerDto,
    pub transitioning: bool,
    pub brightness: u8,
    pub kelvin: u16,
    pub mood_enabled: bool,
    pub mood_active: bool,
    pub standby_enabled: bool,
    pub standby_active: bool,
    pub profile_settings: RoomProfileSettingsDto,
    #[serde(
        rename = "room_profile",
        default,
        skip_serializing_if = "RoomProfileSettings::is_empty"
    )]
    pub room_profile: RoomProfileSettings,
}

/// Room state for poll payloads.
///
/// Extends `RoomRhythmState` with optional motion fields (using SSE names).
#[derive(Clone, Debug, Serialize)]
pub struct RoomPollState {
    #[serde(flatten)]
    pub rhythm: RoomRhythmState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_owned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_active: Option<bool>,
}

/// Full room state for `GET /api/state` snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct RoomFullState {
    #[serde(flatten)]
    pub rhythm: RoomRhythmState,
    pub name: String,
    pub grouped_light_id: String,
    pub disabled: bool,
    pub device_ids: Vec<String>,
    pub devices: Vec<TypedDeviceDto>,
}

// ---------------------------------------------------------------------------
// Wrapper responses
// ---------------------------------------------------------------------------

/// Batch room response — `{"rooms": [...]}`.
#[derive(Debug, Serialize)]
pub struct RoomsResponse {
    pub rooms: Vec<RoomRhythmState>,
}

/// Poll response — `{"hub_connected": bool, "rooms": [...]}`.
#[derive(Debug, Serialize)]
pub struct RoomsPollResponse {
    pub hub_connected: bool,
    pub rooms: Vec<RoomPollState>,
}

/// Full state snapshot for `GET /api/state`.
#[derive(Debug, Serialize)]
pub struct StateSnapshot {
    /// Epoch milliseconds of the most recent periodic tick (for client bootstrap).
    pub last_tick_epoch_ms: u64,
    pub version: String,
    pub server_instance_id: String,
    pub platform: String,
    pub context: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub listen_port: Option<u16>,
    /// All configured hubs with their current live connection state.
    pub hubs: Vec<HubDto>,
    /// Capability metadata for integrations available on this platform.
    pub capabilities: ApiCapabilitiesDto,
    pub active_profile: ActiveProfileDto,
    pub location: LocationDto,
    pub settings: SettingsDto,
    pub light_breaker: LightBreakerDto,
    pub mode: ModeSettingsDto,
    pub transitions: Vec<ModeTransitionConfig>,
    pub scenes: Vec<SceneDefinition>,
    pub input_bindings: Vec<InputBinding>,
    pub profiles: Vec<LightProfileConfig>,
    pub review: ReviewSummaryDto,
    pub nodes: Vec<NodeStateDto>,
}

/// Review-focused metadata for restore/triage/admin workflows.
#[derive(Clone, Debug, Default, Serialize)]
pub struct ReviewSummaryDto {
    pub pending: ReviewCountsDto,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub disconnected_hubs: Vec<ReviewHubDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub preferred_endpoints: Vec<PreferredEndpointDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_configured_conflicts: Vec<ReviewEntryDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub triage_entries: Vec<ReviewEntryDto>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ReviewCountsDto {
    pub devices: usize,
    pub rooms: usize,
    pub unassigned: usize,
    pub hub_configured: usize,
    pub total: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewHubDto {
    #[serde(rename = "type")]
    pub hub_type: String,
    pub address: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct PreferredEndpointDto {
    pub canonical_id: String,
    pub name: String,
    #[serde(rename = "type")]
    pub hub_type: String,
    pub hub_address: String,
    pub native_id: String,
    pub endpoint_count: usize,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReviewEntryDto {
    pub id: String,
    pub kind: TriageKind,
    pub status: TriageStatus,
    #[serde(rename = "type")]
    pub hub_type: String,
    pub hub_address: String,
    pub native_id: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub name: String,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
}

// ---------------------------------------------------------------------------
// Component DTOs
// ---------------------------------------------------------------------------

/// Hub status in state snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct HubDto {
    #[serde(rename = "type")]
    pub hub_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub address: Option<String>,
    pub connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_retry: Option<HubStartupRetryDto>,
}

/// Startup bootstrap retry metadata for a configured-but-not-active hub.
#[derive(Clone, Debug, Serialize)]
pub struct HubStartupRetryDto {
    /// `"scheduled"` while automatic retrying is still active,
    /// `"manual_retry_required"` once the 24h retry budget is exhausted.
    pub status: String,
    pub attempt_count: u32,
    pub first_failure_epoch_ms: i64,
    pub last_failure_epoch_ms: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_retry_epoch_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_error: Option<String>,
}

/// API capability metadata in state snapshot.
#[derive(Clone, Debug, Serialize)]
pub struct ApiCapabilitiesDto {
    pub hubs: Vec<HubCapabilityDto>,
}

/// Capabilities for one available hub integration.
#[derive(Clone, Debug, Serialize)]
pub struct HubCapabilityDto {
    #[serde(rename = "type")]
    pub hub_type: String,
    pub configurable: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub device_onboarding_methods: Vec<String>,
    pub supports_unpairing: bool,
    pub supports_roomless_devices: bool,
}

/// Location in state snapshot.
#[derive(Debug, Serialize)]
pub struct LocationDto {
    /// Current wall-clock time in local time.
    pub current_local_time: String,
    /// Current wall-clock hour in local time as a decimal hour.
    pub current_local_hour: f32,
    pub latitude: Option<f32>,
    pub longitude: Option<f32>,
    pub utc_offset_hours: f32,
    /// Solar noon expressed as a local wall-clock decimal hour.
    pub solar_noon: f32,
    /// Solar noon expressed as a local wall-clock time string (`HH:MM:SS`).
    pub solar_noon_local_time: String,
    /// Solar midnight expressed as a local wall-clock decimal hour.
    pub solar_midnight: f32,
    /// Solar midnight expressed as a local wall-clock time string (`HH:MM:SS`).
    pub solar_midnight_local_time: String,
    /// Current solar clock hour, where 12 = solar noon.
    pub current_solar_time: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timezone_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twilight: Option<TwilightResponse>,
}

/// App-level settings in state snapshot and `GET /api/settings`.
#[derive(Debug, Clone, Serialize)]
pub struct SettingsDto {
    pub auto_update: bool,
    pub lighting_runtime: LightingRuntimeKind,
}

/// Global Rhythm light-breaker switch in `GET/PUT /api/light-breaker`.
#[derive(Debug, Clone, Serialize)]
pub struct LightBreakerDto {
    pub enabled: bool,
}

/// Mode state and policy in `GET /api/mode` and `GET /api/state`.
#[derive(Debug, Serialize)]
pub struct ModeSettingsDto {
    pub active: RhythmMode,
    pub last_change: ModeLastChangeDto,
    pub configs: Vec<ModeConfig>,
}

/// Metadata about the most recent mode change.
#[derive(Debug, Serialize)]
pub struct ModeLastChangeDto {
    pub cause: ModeChangeCause,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub transition_id: Option<String>,
    pub epoch_ms: i64,
}

/// Available light profiles in `GET /api/profiles`.
#[derive(Debug, Serialize)]
pub struct ProfilesDto {
    pub profiles: Vec<LightProfileConfig>,
}

/// Mode transition policy in `GET /api/transitions`.
#[derive(Debug, Serialize)]
pub struct ModeTransitionsDto {
    pub transitions: Vec<ModeTransitionConfig>,
}

/// Physical input bindings in `GET /api/input-bindings`.
#[derive(Debug, Serialize)]
pub struct InputBindingsDto {
    pub bindings: Vec<InputBinding>,
}

/// Active profile in `GET /api/state`, including persisted config plus
/// runtime-resolved effective values.
#[derive(Debug, Serialize)]
pub struct ActiveProfileDto {
    pub config: LightProfileConfig,
    pub effective: ActiveProfileEffectiveDto,
}

/// Runtime-resolved values for the active profile.
#[derive(Debug, Serialize)]
pub struct ActiveProfileEffectiveDto {
    pub fade_ms: u32,
    pub motion_timeout_secs: u64,
    pub rhythm_interval_secs: u64,
}

/// A typed device entry.
#[derive(Clone, Debug, Serialize)]
pub struct TypedDeviceDto {
    pub id: String,
    #[serde(rename = "type")]
    pub device_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TypedDeviceDto {
    pub fn new(id: String, dt: &DeviceType) -> Self {
        Self {
            id,
            device_type: Self::type_str(dt).to_string(),
            name: None,
            manufacturer: None,
            model: None,
        }
    }

    pub fn enriched(
        id: String,
        dt: &DeviceType,
        name: Option<String>,
        manufacturer: Option<String>,
        model: Option<String>,
    ) -> Self {
        Self {
            id,
            device_type: Self::type_str(dt).to_string(),
            name,
            manufacturer,
            model,
        }
    }

    fn type_str(dt: &DeviceType) -> &'static str {
        match dt {
            DeviceType::Light => "light",
            DeviceType::Button => "button",
            DeviceType::Motion => "motion",
        }
    }
}

/// Public node state for any addressable topology/runtime node.
///
/// This is the node-first contract used by `/api/state` and `/api/nodes/*`.
/// Root rooms and standalone/child devices share one shape; hierarchy is
/// expressed by `kind` and optional `parent_id`.
#[derive(Clone, Debug, Serialize)]
pub struct NodeStateDto {
    pub id: String,
    pub name: String,
    pub kind: LightNodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<DevicePlacement>,
    /// Which hub types can address this node (e.g. ["hue"], ["matter"]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    pub state: RoomModeState,
    pub rhythm_enabled: bool,
    pub disabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    pub curve_modifier: CurveModifierDto,
    pub lights_on: bool,
    pub observed_power: ObservedPowerDto,
    pub transitioning: bool,
    pub brightness: u8,
    pub kelvin: u16,
    pub mood_enabled: bool,
    pub mood_active: bool,
    pub standby_enabled: bool,
    pub standby_active: bool,
    pub profile_settings: RoomProfileSettingsDto,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub motion_owned: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeout_secs: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub warning_active: Option<bool>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CurveModifierDto {
    pub time_offset_minutes: f32,
    pub brightness_offset: f32,
    pub brightness: u8,
    pub kelvin: u16,
}

/// Batch node response — `{"nodes": [...]}`.
#[derive(Debug, Serialize)]
pub struct NodesResponse {
    pub nodes: Vec<NodeStateDto>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub queued: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_count: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dispatch_spacing_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub estimated_dispatch_ms: Option<u64>,
}

/// Poll response — `{"hub_connected": bool, "nodes": [...]}`.
#[derive(Debug, Serialize)]
pub struct NodesPollResponse {
    pub hub_connected: bool,
    pub nodes: Vec<NodeStateDto>,
}

/// Public topology graph node for `/api/topology/nodes`.
#[derive(Clone, Debug, Serialize)]
pub struct TopologyNodeDto {
    pub id: String,
    pub name: String,
    pub kind: LightNodeKind,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub placement: Option<DevicePlacement>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<TopologyNodeControlDto>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hub_room_bindings: Vec<HubRoomBinding>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_customized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bootstrap_name: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct TopologyNodeControlDto {
    pub kind: NodeControlKind,
    pub target_id: String,
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub inherited: bool,
}

// ---------------------------------------------------------------------------
// Mutation responses
// ---------------------------------------------------------------------------

/// Response for `POST /api/sync`.
#[derive(Debug, Serialize)]
pub struct SyncResponse {
    pub rooms_added: usize,
    pub rooms_updated: usize,
    pub rooms_removed: usize,
    pub devices_synced: usize,
}

/// Response for `PUT /api/hub/credentials`.
#[derive(Debug, Serialize)]
pub struct HubCredentialsResponse {
    pub hub_connected: bool,
}

// ---------------------------------------------------------------------------
// Curve visualization responses
// ---------------------------------------------------------------------------

/// Full curve response for `GET /api/curve` and `POST /api/curve`.
#[derive(Debug, Serialize)]
pub struct CurveResponse {
    pub config: serde_json::Value,
    pub solar: SolarResponse,
    pub curve: rhythm_profile::CurveData,
    pub steps: rhythm_profile::StepSequences,
}

/// Solar times and twilight data.
#[derive(Debug, Serialize)]
pub struct SolarResponse {
    /// Sunrise expressed as a local wall-clock decimal hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunrise: Option<f32>,
    /// Sunrise expressed as a local wall-clock time string (`HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunrise_local_time: Option<String>,
    /// Sunset expressed as a local wall-clock decimal hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunset: Option<f32>,
    /// Sunset expressed as a local wall-clock time string (`HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sunset_local_time: Option<String>,
    /// Solar noon expressed as a local wall-clock decimal hour.
    pub solar_noon: f32,
    /// Solar noon expressed as a local wall-clock time string (`HH:MM:SS`).
    pub solar_noon_local_time: String,
    /// Solar midnight expressed as a local wall-clock decimal hour.
    pub solar_midnight: f32,
    /// Solar midnight expressed as a local wall-clock time string (`HH:MM:SS`).
    pub solar_midnight_local_time: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub day_length: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub twilight: Option<TwilightResponse>,
}

/// Twilight dawn/dusk phases.
#[derive(Debug, Serialize)]
pub struct TwilightResponse {
    pub dawn: TwilightPhaseResponse,
    pub dusk: TwilightPhaseResponse,
}

/// A single twilight phase (civil/nautical/astronomical).
#[derive(Debug, Serialize)]
pub struct TwilightPhaseResponse {
    /// Civil twilight expressed as a local wall-clock decimal hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub civil: Option<f32>,
    /// Civil twilight expressed as a local wall-clock time string (`HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub civil_local_time: Option<String>,
    /// Nautical twilight expressed as a local wall-clock decimal hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nautical: Option<f32>,
    /// Nautical twilight expressed as a local wall-clock time string (`HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nautical_local_time: Option<String>,
    /// Astronomical twilight expressed as a local wall-clock decimal hour.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astronomical: Option<f32>,
    /// Astronomical twilight expressed as a local wall-clock time string (`HH:MM:SS`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub astronomical_local_time: Option<String>,
}

/// Current lighting values for `GET /api/curve/now`.
#[derive(Debug, Serialize)]
pub struct LightingNowResponse {
    pub hour: f32,
    pub brightness: u8,
    pub kelvin: u16,
    pub mireds: u16,
    pub rgb: rhythm_profile::Rgb,
    pub xy: rhythm_profile::XyColor,
    pub solar_time: f32,
    pub sun_position: f32,
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    fn sample_rhythm_state() -> RoomRhythmState {
        RoomRhythmState {
            id: "room1".into(),
            hub_types: vec!["hue".into()],
            state: rhythm_core::RoomModeState::Active,
            rhythm_enabled: true,
            time_offset: 5.0,
            brightness_offset: -10.0,
            lights_on: true,
            observed_power: ObservedPowerDto {
                lights_on: true,
                fresh: true,
                observed_at_epoch_ms: Some(1_700_000_000_000),
                source: Some("periodic".into()),
            },
            transitioning: true,
            brightness: 80,
            kelvin: 4000,
            mood_enabled: false,
            mood_active: false,
            standby_enabled: false,
            standby_active: false,
            profile_settings: RoomProfileSettingsDto::from_settings(
                &rhythm_core::RoomProfileSettings::default(),
                false,
            ),
            room_profile: rhythm_core::RoomProfileSettings::default(),
        }
    }

    fn sample_node_state() -> NodeStateDto {
        NodeStateDto {
            id: "room1".into(),
            name: "Office".into(),
            kind: rhythm_core::LightNodeKind::Room,
            parent_id: None,
            placement: None,
            hub_types: vec!["hue".into()],
            manufacturer: None,
            model: None,
            state: rhythm_core::RoomModeState::Active,
            rhythm_enabled: true,
            disabled: false,
            time_offset: 5.0,
            brightness_offset: -10.0,
            curve_modifier: CurveModifierDto {
                time_offset_minutes: 5.0,
                brightness_offset: -10.0,
                brightness: 80,
                kelvin: 4000,
            },
            lights_on: true,
            observed_power: ObservedPowerDto {
                lights_on: true,
                fresh: true,
                observed_at_epoch_ms: Some(1_700_000_000_000),
                source: Some("periodic".into()),
            },
            transitioning: true,
            brightness: 80,
            kelvin: 4000,
            mood_enabled: false,
            mood_active: false,
            standby_enabled: false,
            standby_active: false,
            profile_settings: RoomProfileSettingsDto::from_settings(
                &rhythm_core::RoomProfileSettings::default(),
                false,
            ),
            motion_active: None,
            motion_owned: None,
            remaining_secs: None,
            timeout_secs: None,
            warning_active: None,
        }
    }

    #[test]
    fn node_state_uses_profile_settings_key() {
        let node = sample_node_state();
        let json: Value = serde_json::to_value(node).unwrap();
        assert!(json.get("profile_settings").is_some());
        assert_eq!(json["profile_settings"]["mood_enabled"], false);
        assert_eq!(json["mood_enabled"], false);
        assert_eq!(json["mood_active"], false);
        assert_eq!(json["standby_enabled"], false);
        assert_eq!(json["standby_active"], false);
        assert_eq!(json["curve_modifier"]["time_offset_minutes"], 5.0);
        assert_eq!(json["curve_modifier"]["brightness_offset"], -10.0);
        assert_eq!(json["curve_modifier"]["brightness"], 80);
        assert_eq!(json["curve_modifier"]["kelvin"], 4000);
        assert!(json.get("room_profile").is_none());
    }

    // ---- RoomRhythmState ----

    #[test]
    fn room_rhythm_state_serializes_all_fields() {
        let state = sample_rhythm_state();
        let json: Value = serde_json::to_value(&state).unwrap();
        assert_eq!(json["id"], "room1");
        assert_eq!(json["state"], "active");
        assert_eq!(json["rhythm_enabled"], true);
        assert_eq!(json["time_offset"], 5.0);
        assert_eq!(json["brightness_offset"], -10.0);
        assert_eq!(json["lights_on"], true);
        assert_eq!(json["observed_power"]["lights_on"], true);
        assert_eq!(json["observed_power"]["fresh"], true);
        assert_eq!(
            json["observed_power"]["observed_at_epoch_ms"],
            1_700_000_000_000u64
        );
        assert_eq!(json["observed_power"]["source"], "periodic");
        assert_eq!(json["transitioning"], true);
        assert_eq!(json["brightness"], 80);
        assert_eq!(json["kelvin"], 4000);
        assert_eq!(json["profile_settings"]["mood_enabled"], false);
        assert_eq!(json["mood_enabled"], false);
        assert_eq!(json["mood_active"], false);
        assert_eq!(json["standby_enabled"], false);
        assert_eq!(json["standby_active"], false);
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    // ---- RoomPollState ----

    #[test]
    fn room_poll_state_flattens_rhythm_fields() {
        let poll = RoomPollState {
            rhythm: sample_rhythm_state(),
            motion_active: None,
            motion_owned: None,
            remaining_secs: None,
            timeout_secs: None,
            warning_active: None,
        };
        let json: Value = serde_json::to_value(&poll).unwrap();
        // Flattened rhythm fields appear at top level
        assert_eq!(json["id"], "room1");
        assert_eq!(json["brightness"], 80);
        // Motion fields omitted when None
        assert!(json.get("motion_active").is_none());
        assert!(json.get("motion_owned").is_none());
        assert!(json.get("remaining_secs").is_none());
        assert!(json.get("timeout_secs").is_none());
        assert!(json.get("warning_active").is_none());
    }

    #[test]
    fn room_poll_state_includes_motion_when_present() {
        let poll = RoomPollState {
            rhythm: sample_rhythm_state(),
            motion_active: Some(true),
            motion_owned: Some(false),
            remaining_secs: Some(120),
            timeout_secs: Some(300),
            warning_active: Some(true),
        };
        let json: Value = serde_json::to_value(&poll).unwrap();
        assert_eq!(json["motion_active"], true);
        assert_eq!(json["motion_owned"], false);
        assert_eq!(json["remaining_secs"], 120);
        assert_eq!(json["timeout_secs"], 300);
        assert_eq!(json["warning_active"], true);
        // SSE-aligned names — old names must NOT appear
        assert!(json.get("motion_remaining").is_none());
        assert!(json.get("motion_timeout").is_none());
        assert!(json.get("motion_warning").is_none());
    }

    // ---- RoomFullState ----

    #[test]
    fn room_full_state_flattens_with_extra_fields() {
        let full = RoomFullState {
            rhythm: sample_rhythm_state(),
            name: "Living Room".into(),
            grouped_light_id: "gl_abc".into(),
            disabled: false,
            device_ids: vec!["d1".into(), "d2".into()],
            devices: vec![
                TypedDeviceDto::new("d1".into(), &DeviceType::Light),
                TypedDeviceDto::new("d2".into(), &DeviceType::Button),
            ],
        };
        let json: Value = serde_json::to_value(&full).unwrap();
        // Flattened from rhythm
        assert_eq!(json["id"], "room1");
        assert_eq!(json["kelvin"], 4000);
        // Full-state only fields
        assert_eq!(json["name"], "Living Room");
        assert_eq!(json["grouped_light_id"], "gl_abc");
        assert_eq!(json["disabled"], false);
        assert_eq!(json["device_ids"], serde_json::json!(["d1", "d2"]));
        assert_eq!(json["devices"][0]["id"], "d1");
        assert_eq!(json["devices"][0]["type"], "light");
        assert_eq!(json["devices"][1]["type"], "button");
    }

    // ---- RoomsResponse ----

    #[test]
    fn rooms_response_wraps_in_rooms_array() {
        let resp = RoomsResponse {
            rooms: vec![sample_rhythm_state()],
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert!(json["rooms"].is_array());
        assert_eq!(json["rooms"].as_array().unwrap().len(), 1);
        assert_eq!(json["rooms"][0]["id"], "room1");
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    #[test]
    fn rooms_response_empty_array() {
        let resp = RoomsResponse { rooms: vec![] };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["rooms"].as_array().unwrap().len(), 0);
    }

    // ---- RoomsPollResponse ----

    #[test]
    fn rooms_poll_response_shape() {
        let resp = RoomsPollResponse {
            hub_connected: true,
            rooms: vec![RoomPollState {
                rhythm: sample_rhythm_state(),
                motion_active: Some(true),
                motion_owned: Some(true),
                remaining_secs: Some(60),
                timeout_secs: Some(300),
                warning_active: Some(false),
            }],
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["hub_connected"], true);
        assert_eq!(json["rooms"][0]["id"], "room1");
        assert_eq!(json["rooms"][0]["remaining_secs"], 60);
        // No status wrapper
        assert!(json.get("status").is_none());
    }

    // ---- SettingsDto ----

    #[test]
    fn settings_dto_serializes() {
        let dto = SettingsDto {
            auto_update: true,
            lighting_runtime: LightingRuntimeKind::default(),
        };
        let json: Value = serde_json::to_value(&dto).unwrap();
        assert!(json.get("power_save").is_none());
        assert_eq!(json["auto_update"], true);
        assert_eq!(json["lighting_runtime"], "rhythm-adaptive");
        assert!(json.get("light_breaker_enabled").is_none());
        assert!(json.get("mode").is_none());
        assert!(json.get("profiles").is_none());
    }

    #[test]
    fn mode_settings_dto_includes_last_mode_change_timestamp() {
        let dto = ModeSettingsDto {
            active: rhythm_core::RhythmMode::Day,
            last_change: ModeLastChangeDto {
                cause: rhythm_core::ModeChangeCause::Manual,
                transition_id: None,
                epoch_ms: 1_700_000_000_000,
            },
            configs: rhythm_core::default_mode_configs(),
        };
        let json: Value = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["last_change"]["cause"], "manual");
        assert!(json["last_change"].get("transition_id").is_none());
        assert_eq!(json["last_change"]["epoch_ms"], 1_700_000_000_000i64);
        assert!(json.get("transitions").is_none());
    }

    #[test]
    fn profiles_dto_serializes() {
        let dto = ProfilesDto {
            profiles: vec![
                rhythm_core::default_rhythm_profile(),
                rhythm_core::default_sleep_profile(),
            ],
        };
        let json: Value = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["profiles"].as_array().unwrap().len(), 2);
        assert_eq!(json["profiles"][0]["id"], "rhythm");
    }

    #[test]
    fn mode_transitions_dto_serializes() {
        let dto = ModeTransitionsDto {
            transitions: rhythm_core::default_mode_transition_configs(),
        };
        let json: Value = serde_json::to_value(&dto).unwrap();
        assert_eq!(json["transitions"].as_array().unwrap().len(), 2);
    }

    // ---- HubDto ----

    #[test]
    fn hub_dto_type_field_renamed() {
        let hub = HubDto {
            hub_type: "hue".into(),
            address: Some("192.168.1.100".into()),
            connected: true,
            startup_retry: None,
        };
        let json: Value = serde_json::to_value(&hub).unwrap();
        // Serialized as "type", not "hub_type"
        assert_eq!(json["type"], "hue");
        assert!(json.get("hub_type").is_none());
        assert_eq!(json["address"], "192.168.1.100");
        assert_eq!(json["connected"], true);
    }

    #[test]
    fn hub_dto_address_omitted_when_none() {
        let hub = HubDto {
            hub_type: "none".into(),
            address: None,
            connected: false,
            startup_retry: None,
        };
        let json: Value = serde_json::to_value(&hub).unwrap();
        assert!(json.get("address").is_none());
        assert_eq!(json["type"], "none");
        assert_eq!(json["connected"], false);
    }

    #[test]
    fn hub_dto_serializes_startup_retry_when_present() {
        let hub = HubDto {
            hub_type: "hue".into(),
            address: Some("192.168.1.100".into()),
            connected: false,
            startup_retry: Some(HubStartupRetryDto {
                status: "scheduled".into(),
                attempt_count: 3,
                first_failure_epoch_ms: 1_700_000_000_000,
                last_failure_epoch_ms: 1_700_000_000_500,
                next_retry_epoch_ms: Some(1_700_000_010_500),
                last_error: Some("timeout".into()),
            }),
        };
        let json: Value = serde_json::to_value(&hub).unwrap();
        assert_eq!(json["startup_retry"]["status"], "scheduled");
        assert_eq!(json["startup_retry"]["attempt_count"], 3);
        assert_eq!(
            json["startup_retry"]["next_retry_epoch_ms"],
            1_700_000_010_500i64
        );
        assert_eq!(json["startup_retry"]["last_error"], "timeout");
    }

    #[test]
    fn hub_capability_dto_type_field_renamed() {
        let capability = HubCapabilityDto {
            hub_type: "matter".into(),
            configurable: true,
            device_onboarding_methods: vec!["matter_on_network_setup_code".into()],
            supports_unpairing: true,
            supports_roomless_devices: true,
        };
        let json: Value = serde_json::to_value(&capability).unwrap();
        assert_eq!(json["type"], "matter");
        assert!(json.get("hub_type").is_none());
        assert_eq!(
            json["device_onboarding_methods"][0],
            "matter_on_network_setup_code"
        );
        assert_eq!(json["supports_roomless_devices"], true);
    }

    // ---- LocationDto ----

    #[test]
    fn location_dto_with_timezone() {
        let loc = LocationDto {
            current_local_time: "2024-01-01T06:00:00-06:00".into(),
            current_local_hour: 6.0,
            latitude: Some(35.6),
            longitude: Some(-97.5),
            utc_offset_hours: -6.0,
            solar_noon: 12.3,
            solar_noon_local_time: "12:18:00".into(),
            solar_midnight: 0.3,
            solar_midnight_local_time: "00:18:00".into(),
            current_solar_time: 5.7,
            timezone_name: Some("America/Chicago".into()),
            twilight: Some(TwilightResponse {
                dawn: TwilightPhaseResponse {
                    civil: Some(6.0),
                    civil_local_time: Some("06:00:00".into()),
                    nautical: Some(5.5),
                    nautical_local_time: Some("05:30:00".into()),
                    astronomical: Some(5.0),
                    astronomical_local_time: Some("05:00:00".into()),
                },
                dusk: TwilightPhaseResponse {
                    civil: Some(18.0),
                    civil_local_time: Some("18:00:00".into()),
                    nautical: Some(18.5),
                    nautical_local_time: Some("18:30:00".into()),
                    astronomical: Some(19.0),
                    astronomical_local_time: Some("19:00:00".into()),
                },
            }),
        };
        let json: Value = serde_json::to_value(&loc).unwrap();
        assert_eq!(json["current_local_hour"], 6.0);
        assert_eq!(json["latitude"], 35.6_f32 as f64);
        assert_eq!(json["longitude"], -97.5_f32 as f64);
        assert_eq!(json["utc_offset_hours"], -6.0_f32 as f64);
        assert_eq!(json["solar_noon"], 12.3_f32 as f64);
        assert_eq!(json["solar_noon_local_time"], "12:18:00");
        assert_eq!(json["solar_midnight"], 0.3_f32 as f64);
        assert_eq!(json["solar_midnight_local_time"], "00:18:00");
        assert_eq!(json["current_solar_time"], 5.7_f32 as f64);
        assert_eq!(json["timezone_name"], "America/Chicago");
        assert_eq!(json["twilight"]["dawn"]["civil"], 6.0);
        assert_eq!(json["twilight"]["dawn"]["civil_local_time"], "06:00:00");
        assert_eq!(json["twilight"]["dusk"]["nautical"], 18.5);
        assert_eq!(json["twilight"]["dusk"]["nautical_local_time"], "18:30:00");
    }

    #[test]
    fn location_dto_null_optionals() {
        let loc = LocationDto {
            current_local_time: "2024-01-01T00:00:00Z".into(),
            current_local_hour: 0.0,
            latitude: None,
            longitude: None,
            utc_offset_hours: 0.0,
            solar_noon: 12.0,
            solar_noon_local_time: "12:00:00".into(),
            solar_midnight: 0.0,
            solar_midnight_local_time: "00:00:00".into(),
            current_solar_time: 18.0,
            timezone_name: None,
            twilight: None,
        };
        let json: Value = serde_json::to_value(&loc).unwrap();
        assert!(json["latitude"].is_null());
        assert!(json["longitude"].is_null());
        assert_eq!(json["current_local_hour"], 0.0);
        assert_eq!(json["solar_noon"], 12.0);
        assert_eq!(json["solar_noon_local_time"], "12:00:00");
        assert_eq!(json["solar_midnight"], 0.0);
        assert_eq!(json["solar_midnight_local_time"], "00:00:00");
        assert_eq!(json["current_solar_time"], 18.0);
        assert!(json.get("timezone_name").is_none());
        assert!(json.get("twilight").is_none());
    }

    // ---- TypedDeviceDto ----

    #[test]
    fn typed_device_dto_type_field_renamed() {
        let d = TypedDeviceDto::new("dev1".into(), &DeviceType::Motion);
        let json: Value = serde_json::to_value(&d).unwrap();
        assert_eq!(json["id"], "dev1");
        assert_eq!(json["type"], "motion");
        assert!(json.get("device_type").is_none());
    }

    // ---- SyncResponse ----

    #[test]
    fn sync_response_no_status_wrapper() {
        let resp = SyncResponse {
            rooms_added: 3,
            rooms_updated: 1,
            rooms_removed: 0,
            devices_synced: 8,
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["rooms_added"], 3);
        assert_eq!(json["rooms_updated"], 1);
        assert_eq!(json["rooms_removed"], 0);
        assert_eq!(json["devices_synced"], 8);
        assert!(json.get("status").is_none());
    }

    // ---- HubCredentialsResponse ----

    #[test]
    fn hub_credentials_response_no_status_wrapper() {
        let resp = HubCredentialsResponse {
            hub_connected: true,
        };
        let json: Value = serde_json::to_value(&resp).unwrap();
        assert_eq!(json["hub_connected"], true);
        assert!(json.get("status").is_none());
    }

    // ---- StateSnapshot ----

    #[test]
    fn state_snapshot_omits_listen_port_when_none() {
        let snap = StateSnapshot {
            version: "1.0.0".into(),
            server_instance_id: "srv-test".into(),
            platform: "desktop".into(),
            context: "server".into(),
            listen_port: None,
            hubs: vec![],
            capabilities: ApiCapabilitiesDto { hubs: vec![] },
            active_profile: ActiveProfileDto {
                config: rhythm_core::default_rhythm_profile(),
                effective: ActiveProfileEffectiveDto {
                    fade_ms: 500,
                    motion_timeout_secs: 1200,
                    rhythm_interval_secs: 60,
                },
            },
            location: LocationDto {
                current_local_time: "2024-01-01T00:00:00Z".into(),
                current_local_hour: 0.0,
                latitude: None,
                longitude: None,
                utc_offset_hours: 0.0,
                solar_noon: 12.0,
                solar_noon_local_time: "12:00:00".into(),
                solar_midnight: 0.0,
                solar_midnight_local_time: "00:00:00".into(),
                current_solar_time: 18.0,
                timezone_name: None,
                twilight: None,
            },
            settings: SettingsDto {
                auto_update: true,
                lighting_runtime: LightingRuntimeKind::default(),
            },
            light_breaker: LightBreakerDto { enabled: true },
            mode: ModeSettingsDto {
                active: rhythm_core::RhythmMode::Day,
                last_change: ModeLastChangeDto {
                    cause: rhythm_core::ModeChangeCause::Manual,
                    transition_id: None,
                    epoch_ms: 1_700_000_000_000,
                },
                configs: rhythm_core::default_mode_configs(),
            },
            transitions: rhythm_core::default_mode_transition_configs(),
            scenes: vec![],
            input_bindings: vec![],
            profiles: vec![
                rhythm_core::default_rhythm_profile(),
                rhythm_core::default_sleep_profile(),
            ],
            review: ReviewSummaryDto::default(),
            nodes: vec![],
            last_tick_epoch_ms: 1700000000000,
        };
        let json: Value = serde_json::to_value(&snap).unwrap();
        assert!(json.get("listen_port").is_none());
        assert_eq!(json["last_tick_epoch_ms"], 1700000000000u64);
        assert_eq!(json["version"], "1.0.0");
        assert_eq!(json["server_instance_id"], "srv-test");
        assert_eq!(json["platform"], "desktop");
        assert_eq!(json["context"], "server");
        assert_eq!(json["active_profile"]["effective"]["fade_ms"], 500);
        assert_eq!(
            json["active_profile"]["effective"]["motion_timeout_secs"],
            1200
        );
        assert!(json["nodes"].as_array().unwrap().is_empty());
        assert!(json["location"]["current_local_time"].is_string());
        assert_eq!(json["location"]["current_local_hour"], 0.0);
        assert_eq!(json["location"]["solar_noon"], 12.0);
        assert_eq!(json["location"]["solar_noon_local_time"], "12:00:00");
        assert_eq!(json["location"]["solar_midnight"], 0.0);
        assert_eq!(json["location"]["solar_midnight_local_time"], "00:00:00");
        assert_eq!(json["location"]["current_solar_time"], 18.0);
        assert_eq!(json["transitions"].as_array().unwrap().len(), 2);
        assert!(json["input_bindings"].as_array().unwrap().is_empty());
        assert!(json["review"].is_object());
    }

    #[test]
    fn state_snapshot_includes_listen_port_when_set() {
        let snap = StateSnapshot {
            version: "1.0.0".into(),
            server_instance_id: "srv-test".into(),
            platform: "desktop".into(),
            context: "ha_addon".into(),
            listen_port: Some(8099),
            hubs: vec![HubDto {
                hub_type: "hue".into(),
                address: Some("192.168.1.2".into()),
                connected: true,
                startup_retry: None,
            }],
            capabilities: ApiCapabilitiesDto {
                hubs: vec![HubCapabilityDto {
                    hub_type: "matter".into(),
                    configurable: true,
                    device_onboarding_methods: vec!["matter_on_network_setup_code".into()],
                    supports_unpairing: true,
                    supports_roomless_devices: true,
                }],
            },
            active_profile: ActiveProfileDto {
                config: rhythm_core::default_rhythm_profile(),
                effective: ActiveProfileEffectiveDto {
                    fade_ms: 500,
                    motion_timeout_secs: 300,
                    rhythm_interval_secs: 60,
                },
            },
            location: LocationDto {
                current_local_time: "2024-01-01T00:00:00Z".into(),
                current_local_hour: 0.0,
                latitude: Some(35.0),
                longitude: Some(-97.0),
                utc_offset_hours: -6.0,
                solar_noon: 12.4,
                solar_noon_local_time: "12:24:00".into(),
                solar_midnight: 0.4,
                solar_midnight_local_time: "00:24:00".into(),
                current_solar_time: 8.1,
                timezone_name: Some("America/Chicago".into()),
                twilight: Some(TwilightResponse {
                    dawn: TwilightPhaseResponse {
                        civil: Some(6.0),
                        civil_local_time: Some("06:00:00".into()),
                        nautical: Some(5.5),
                        nautical_local_time: Some("05:30:00".into()),
                        astronomical: Some(5.0),
                        astronomical_local_time: Some("05:00:00".into()),
                    },
                    dusk: TwilightPhaseResponse {
                        civil: Some(18.0),
                        civil_local_time: Some("18:00:00".into()),
                        nautical: Some(18.5),
                        nautical_local_time: Some("18:30:00".into()),
                        astronomical: Some(19.0),
                        astronomical_local_time: Some("19:00:00".into()),
                    },
                }),
            },
            settings: SettingsDto {
                auto_update: true,
                lighting_runtime: LightingRuntimeKind::default(),
            },
            light_breaker: LightBreakerDto { enabled: true },
            mode: ModeSettingsDto {
                active: rhythm_core::RhythmMode::Day,
                last_change: ModeLastChangeDto {
                    cause: rhythm_core::ModeChangeCause::Schedule,
                    transition_id: Some("day_to_sleep".into()),
                    epoch_ms: 1_700_000_000_000,
                },
                configs: rhythm_core::default_mode_configs(),
            },
            transitions: rhythm_core::default_mode_transition_configs(),
            scenes: vec![],
            input_bindings: vec![],
            profiles: vec![
                rhythm_core::default_rhythm_profile(),
                rhythm_core::default_sleep_profile(),
            ],
            review: ReviewSummaryDto::default(),
            nodes: vec![sample_node_state()],
            last_tick_epoch_ms: 1700000000000,
        };
        let json: Value = serde_json::to_value(&snap).unwrap();
        assert_eq!(json["listen_port"], 8099);
        assert_eq!(json["nodes"].as_array().unwrap().len(), 1);
        assert_eq!(json["nodes"][0]["name"], "Office");
        assert_eq!(json["hubs"][0]["type"], "hue");
        assert_eq!(json["capabilities"]["hubs"][0]["type"], "matter");
        assert_eq!(
            json["capabilities"]["hubs"][0]["device_onboarding_methods"][0],
            "matter_on_network_setup_code"
        );
        assert_eq!(json["last_tick_epoch_ms"], 1700000000000u64);
        assert_eq!(json["location"]["solar_noon"], 12.4_f32 as f64);
        assert_eq!(json["location"]["solar_noon_local_time"], "12:24:00");
        assert_eq!(json["location"]["solar_midnight"], 0.4_f32 as f64);
        assert_eq!(json["location"]["solar_midnight_local_time"], "00:24:00");
        assert_eq!(json["location"]["current_solar_time"], 8.1_f32 as f64);
        assert_eq!(json["location"]["twilight"]["dusk"]["astronomical"], 19.0);
        assert_eq!(
            json["location"]["twilight"]["dusk"]["astronomical_local_time"],
            "19:00:00"
        );
        assert!(json["settings"].get("power_save").is_none());
        assert_eq!(json["mode"]["active"], "day");
        assert_eq!(json["mode"]["last_change"]["cause"], "schedule");
        assert_eq!(json["mode"]["last_change"]["transition_id"], "day_to_sleep");
        assert_eq!(
            json["mode"]["last_change"]["epoch_ms"],
            1_700_000_000_000i64
        );
        assert_eq!(json["profiles"].as_array().unwrap().len(), 2);
    }
}
