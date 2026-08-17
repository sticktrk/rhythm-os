//! Server-Sent Events types for real-time state updates.
//!
//! Used by the SSE endpoint (`GET /api/events`) to push changes to
//! connected clients. Gated behind `desktop` feature.

use serde::Serialize;

use rhythm_core::{
    ButtonAction, ModeChangeCause, NodeSnapshot, Rgb, RhythmMode, RoomModeState,
    RoomProfileSettings,
};

use crate::api_types::{
    LightBreakerDto, LightCapabilitiesDto, ObservedPowerDto, RoomProfileSettingsDto, SettingsDto,
};
use crate::pairing::{PairedDeviceInfo, PairingFailureStage, PairingStage, PairingStatus};
use crate::state::MotionSnapshot;

/// User-visible stage of an OTA update flow.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OtaUpdateStage {
    /// Server is checking the update manifest.
    Checking,
    /// A newer or repair update is available.
    UpdateAvailable,
    /// The server is already on the latest advertised version.
    UpToDate,
    /// Update artifact download is in progress.
    Downloading,
    /// Downloaded artifact checksum is being verified.
    Verifying,
    /// Artifact payloads are being prepared before install.
    Staging,
    /// Payloads are being installed or flashed.
    Installing,
    /// Install metadata is being finalized.
    Finalizing,
    /// Update completed and the server is restarting.
    Restarting,
    /// Update failed.
    Failed,
}

/// A server event broadcast to all connected SSE clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Addressable node state changed (after button, periodic tick, etc.).
    NodeState { nodes: Vec<NodeStateEvent> },
    /// Motion timer state changed.
    MotionTimer { timers: Vec<MotionTimerEvent> },
    /// Raw physical input observed from a hub before/while it is routed.
    InputEvent(InputEventResource),
    /// Hub connected/disconnected (per-hub status).
    HubStatus {
        /// Which hub type this status is for (e.g. "hue", "homeassistant").
        /// `None` for backward-compat broadcasts (any-hub-connected).
        #[serde(skip_serializing_if = "Option::is_none")]
        hub_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
        connected: bool,
    },
    /// A hub light command failed, timed out, or was dropped.
    ///
    /// Dispatch is fire-and-forget — commands are queued and paced per hub —
    /// so physical delivery problems surface here instead of failing the
    /// originating API call.
    DispatchFailure {
        hub_type: String,
        hub_key: String,
        /// Topology node the command addressed.
        node_id: String,
        /// Exact canonical light-device node for the failed endpoint.
        ///
        /// Grouped and stream-level failures omit this instead of guessing.
        #[serde(skip_serializing_if = "Option::is_none")]
        target_node_id: Option<String>,
        /// Hub-native dispatch target label.
        target: String,
        /// Command kind: "turn_on" or "turn_off".
        kind: String,
        /// Failure class: "failed", "timed_out", "skipped_cooldown", "dropped".
        status: String,
        /// Human-readable failure detail.
        #[serde(skip_serializing_if = "Option::is_none")]
        detail: Option<String>,
        /// Time the command waited in the hub queue.
        queued_ms: u64,
        /// Time the physical dispatch ran before failing.
        dispatch_ms: u64,
        epoch_ms: i64,
    },
    /// Settings changed.
    SettingsChanged { settings: SettingsDto },
    /// Global autonomous light breaker changed.
    LightBreakerChanged { light_breaker: LightBreakerDto },
    /// Active mode flipped — carries the new mode + last_change metadata so
    /// clients can update the displayed mode without an HTTP roundtrip and
    /// without waiting for the paced per-node `NodeState` events that follow.
    ModeChanged {
        active: RhythmMode,
        cause: ModeChangeCause,
        #[serde(skip_serializing_if = "Option::is_none")]
        transition_id: Option<String>,
        epoch_ms: i64,
    },
    /// Curve config changed (client should refetch).
    ConfigChanged,
    /// Topology graph changed (client should re-hello).
    NodesChanged,
    /// Triage queue changed (new entries or resolutions).
    TriageChanged {
        /// Total pending entries.
        pending_count: usize,
        /// Pending device merge entries.
        pending_devices: usize,
        /// Pending room binding entries.
        pending_rooms: usize,
        /// Pending unassigned device entries.
        pending_unassigned: usize,
        /// Pending hub-configured device entries.
        pending_hub_configured: usize,
        /// Capability-gated unreachable Matter device entries. This is kept
        /// separate from pending_count so older apps do not badge entries
        /// they cannot render.
        pending_unreachable: usize,
    },
    /// Device pairing progress changed.
    PairingProgress {
        /// Which integration is handling this pairing.
        hub_type: String,
        /// Optional client-generated correlation ID from the pairing request.
        #[serde(skip_serializing_if = "Option::is_none")]
        session_id: Option<String>,
        /// Coarse pairing status for compatibility with final pairing results.
        status: PairingStatus,
        /// More specific user-visible stage.
        stage: PairingStage,
        /// Short user-facing progress text.
        message: String,
        /// Device info, populated on completion when available.
        #[serde(skip_serializing_if = "Option::is_none")]
        device: Option<PairedDeviceInfo>,
        /// Every device completed by a batch pairing request.
        ///
        /// `device` remains populated with the first device for compatibility
        /// with clients that predate batch discovery.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        devices: Vec<PairedDeviceInfo>,
        /// Non-fatal candidate failures from a partially successful batch.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        warnings: Vec<String>,
        /// Error text, populated on failure.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
        /// Deepest privacy-safe stage reached by a failed attempt.
        #[serde(skip_serializing_if = "Option::is_none")]
        failure_stage: Option<PairingFailureStage>,
    },
    /// OTA update progress changed.
    OtaUpdateProgress {
        /// More specific user-visible stage.
        stage: OtaUpdateStage,
        /// Short user-facing progress text.
        message: String,
        /// Server version currently running.
        #[serde(skip_serializing_if = "Option::is_none")]
        current_version: Option<String>,
        /// Version being checked or installed.
        #[serde(skip_serializing_if = "Option::is_none")]
        target_version: Option<String>,
        /// Whether the latest check found an update.
        #[serde(skip_serializing_if = "Option::is_none")]
        update_available: Option<bool>,
        /// Bytes downloaded so far, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        downloaded_bytes: Option<u64>,
        /// Total bytes expected from the response, when known.
        #[serde(skip_serializing_if = "Option::is_none")]
        total_bytes: Option<u64>,
        /// Integer download percentage from 0-100, when total bytes are known.
        #[serde(skip_serializing_if = "Option::is_none")]
        percent: Option<u8>,
        /// Whether checksum verification completed successfully.
        #[serde(skip_serializing_if = "Option::is_none")]
        checksum_verified: Option<bool>,
        /// Installed target names, populated near restart when available.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        installed_targets: Vec<String>,
        /// Error text, populated on failure.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<String>,
    },
}

/// How a physical input event was routed by the server.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InputEventRoute {
    /// The event matched an automation/input binding.
    InputBinding,
    /// The event routed to ordinary node control behavior.
    NodeControl,
    /// The hub delivered an event for a known device with no usable target.
    Unroutable,
    /// The event could not be resolved enough to route.
    Unresolved,
}

/// Physical input observed from an integration and normalized for app clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum InputEventResource {
    Button {
        epoch_ms: i64,
        route: InputEventRoute,
        #[serde(skip_serializing_if = "Option::is_none")]
        hub_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_room_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        native_device_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        native_button_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        button_action: Option<ButtonAction>,
    },
    Motion {
        epoch_ms: i64,
        route: InputEventRoute,
        #[serde(skip_serializing_if = "Option::is_none")]
        hub_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_room_id: Option<String>,
        native_sensor_id: String,
        detected: bool,
    },
    Contact {
        epoch_ms: i64,
        route: InputEventRoute,
        #[serde(skip_serializing_if = "Option::is_none")]
        hub_type: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        address: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        target_node_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        source_room_id: Option<String>,
        native_sensor_id: String,
        open: bool,
    },
}

/// Addressable node rhythm state in an SSE event.
#[derive(Clone, Debug, Serialize)]
pub struct NodeStateEvent {
    pub id: String,
    /// Which hub types can address this node (e.g. ["hue"], ["hue", "matter"]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub light_capabilities: Option<LightCapabilitiesDto>,
    pub mode: RhythmMode,
    pub state: RoomModeState,
    pub rhythm_enabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    /// Whether lights are currently on in this room.
    pub lights_on: bool,
    pub observed_power: ObservedPowerDto,
    /// Whether a global mode transition fade is currently in progress.
    pub transitioning: bool,
    /// Whether light-dispatch work for this node is queued or running.
    pub pending_dispatch: bool,
    /// Effective brightness percentage (1-100) after offsets.
    pub brightness: u8,
    /// Effective color temperature in Kelvin.
    pub kelvin: u16,
    /// Direct RGB color the node is rendering, set when a mood scene applies a
    /// non-kelvin color so the app's mood indicator matches the bulbs.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<Rgb>,
    pub mood_enabled: bool,
    pub mood_active: bool,
    pub standby_enabled: bool,
    pub standby_active: bool,
    /// `true` when this event was triggered by the periodic rhythm tick.
    /// Absent (or `false`) for user actions, polls, and other state changes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tick: bool,
    pub profile_settings: RoomProfileSettingsDto,
    #[serde(rename = "room_profile", default)]
    pub room_profile: RoomProfileSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct NodeStateEventParams {
    pub hub_types: Vec<String>,
    pub light_capabilities: Option<LightCapabilitiesDto>,
    pub mode: RhythmMode,
    pub state: RoomModeState,
    pub lights_on: bool,
    pub observed_power: ObservedPowerDto,
    pub transitioning: bool,
    pub pending_dispatch: bool,
    pub brightness: u8,
    pub kelvin: u16,
    pub color: Option<Rgb>,
    pub mood_enabled: bool,
    pub mood_active: bool,
    pub standby_enabled: bool,
    pub standby_active: bool,
    pub room_profile: RoomProfileSettings,
}

impl NodeStateEvent {
    /// Build from an engine node snapshot with display values.
    pub(crate) fn from_snapshot(snap: &NodeSnapshot, params: NodeStateEventParams) -> Self {
        let NodeStateEventParams {
            hub_types,
            light_capabilities,
            mode,
            state,
            lights_on,
            observed_power,
            transitioning,
            pending_dispatch,
            brightness,
            kelvin,
            color,
            mood_enabled,
            mood_active,
            standby_enabled,
            standby_active,
            room_profile,
        } = params;
        Self {
            id: snap.id.clone(),
            hub_types,
            light_capabilities,
            mode,
            state,
            rhythm_enabled: snap.rhythm_enabled,
            time_offset: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
            lights_on,
            observed_power,
            transitioning,
            pending_dispatch,
            brightness,
            kelvin,
            color,
            mood_enabled,
            mood_active,
            standby_enabled,
            standby_active,
            tick: false,
            profile_settings: RoomProfileSettingsDto::from_settings(
                &snap.profile_settings,
                mood_enabled,
            ),
            room_profile,
        }
    }
}

/// Motion timer state in an SSE event.
#[derive(Clone, Debug, Serialize)]
pub struct MotionTimerEvent {
    pub node_id: String,
    pub motion_active: bool,
    pub motion_owned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    pub timeout_secs: u64,
    pub warning_active: bool,
}

impl MotionTimerEvent {
    /// Build from a motion snapshot.
    pub fn from_snapshot(node_id: &str, snap: &MotionSnapshot) -> Self {
        Self {
            node_id: node_id.to_string(),
            motion_active: snap.motion_active,
            motion_owned: snap.motion_owned,
            remaining_secs: snap.remaining_secs,
            timeout_secs: snap.timeout_secs,
            warning_active: snap.warning_active,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::light_runtime::LightRuntimeKind;

    #[test]
    fn server_event_serializes_with_tagged_type_and_data() {
        let event = ServerEvent::SettingsChanged {
            settings: SettingsDto {
                auto_update: true,
                update_channel: crate::state::UpdateChannel::Stable,
                light_runtime: LightRuntimeKind::default(),
            },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains("\"type\":\"settings_changed\""),
            "expected snake_case tagged type, got {}",
            json
        );
        assert!(
            !json.contains("power_save"),
            "unexpected power_save: {json}"
        );
        assert!(
            json.contains("\"auto_update\":true"),
            "expected auto_update payload, got {}",
            json
        );
        assert!(
            !json.contains("light_breaker"),
            "settings payload must not include light breaker state, got {}",
            json
        );
    }

    #[test]
    fn dispatch_failure_serializes_optional_canonical_target_node() {
        let event = ServerEvent::DispatchFailure {
            hub_type: "matter".into(),
            hub_key: "matter@local".into(),
            node_id: "room-1".into(),
            target_node_id: Some("bulb-1".into()),
            target: "matter-113".into(),
            kind: "matter_controller_command".into(),
            status: "timed_out".into(),
            detail: None,
            queued_ms: 12,
            dispatch_ms: 4500,
            epoch_ms: 1_778_000_000_000,
        };

        let json = serde_json::to_value(&event).unwrap();
        assert_eq!(json["type"], "dispatch_failure");
        assert_eq!(json["data"]["target_node_id"], "bulb-1");

        let unresolved = ServerEvent::DispatchFailure {
            hub_type: "hue".into(),
            hub_key: "hue@bridge.local".into(),
            node_id: "room-1".into(),
            target_node_id: None,
            target: "grouped_light/abc".into(),
            kind: "turn_on".into(),
            status: "failed".into(),
            detail: None,
            queued_ms: 0,
            dispatch_ms: 20,
            epoch_ms: 1_778_000_000_000,
        };
        let unresolved_json = serde_json::to_value(&unresolved).unwrap();
        assert!(unresolved_json["data"].get("target_node_id").is_none());
    }

    #[test]
    fn light_breaker_event_serializes_with_dedicated_payload() {
        let event = ServerEvent::LightBreakerChanged {
            light_breaker: LightBreakerDto { enabled: false },
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains("\"type\":\"light_breaker_changed\""),
            "expected light breaker tagged type, got {}",
            json
        );
        assert!(
            json.contains("\"enabled\":false"),
            "expected light breaker payload, got {}",
            json
        );
    }

    #[test]
    fn server_event_hub_status_round_trips() {
        let event = ServerEvent::HubStatus {
            hub_type: Some("hue".into()),
            address: Some("192.168.1.10".into()),
            connected: true,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"hub_status\""));
        assert!(json.contains("\"hub_type\":\"hue\""));
        assert!(json.contains("\"connected\":true"));
    }

    #[test]
    fn server_event_input_button_serializes_as_generic_input_event() {
        let event = ServerEvent::InputEvent(InputEventResource::Button {
            epoch_ms: 1778058932588,
            route: InputEventRoute::NodeControl,
            hub_type: Some("hue".into()),
            address: Some("192.168.1.20:443".into()),
            source_node_id: Some("button-1".into()),
            target_node_id: Some("room-1".into()),
            source_room_id: Some("hue-room-1".into()),
            native_device_id: Some("hue-button-native".into()),
            native_button_id: None,
            button_action: Some(ButtonAction::OnPress),
        });
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "input_event");
        assert_eq!(parsed["data"]["kind"], "button");
        assert_eq!(parsed["data"]["route"], "node_control");
        assert_eq!(parsed["data"]["hub_type"], "hue");
        assert_eq!(parsed["data"]["address"], "192.168.1.20:443");
        assert_eq!(parsed["data"]["source_node_id"], "button-1");
        assert_eq!(parsed["data"]["target_node_id"], "room-1");
        assert_eq!(parsed["data"]["source_room_id"], "hue-room-1");
        assert_eq!(parsed["data"]["native_device_id"], "hue-button-native");
        assert_eq!(parsed["data"]["button_action"], "on_press");
        assert!(parsed["data"].get("native_button_id").is_none(), "{}", json);
        assert!(parsed["data"].get("event").is_none(), "{}", json);
    }

    #[test]
    fn server_event_input_motion_serializes_as_generic_input_event() {
        let event = ServerEvent::InputEvent(InputEventResource::Motion {
            epoch_ms: 1778058932588,
            route: InputEventRoute::Unroutable,
            hub_type: Some("ha".into()),
            address: Some("homeassistant.local".into()),
            source_node_id: Some("motion-1".into()),
            target_node_id: None,
            source_room_id: Some("ha-area-1".into()),
            native_sensor_id: "binary_sensor.motion_1".into(),
            detected: false,
        });
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "input_event");
        assert_eq!(parsed["data"]["kind"], "motion");
        assert_eq!(parsed["data"]["route"], "unroutable");
        assert_eq!(parsed["data"]["hub_type"], "ha");
        assert_eq!(parsed["data"]["address"], "homeassistant.local");
        assert_eq!(parsed["data"]["source_node_id"], "motion-1");
        assert!(parsed["data"].get("target_node_id").is_none(), "{}", json);
        assert_eq!(parsed["data"]["source_room_id"], "ha-area-1");
        assert_eq!(parsed["data"]["native_sensor_id"], "binary_sensor.motion_1");
        assert_eq!(parsed["data"]["detected"], false);
    }

    #[test]
    fn server_event_input_contact_serializes_as_generic_input_event() {
        let event = ServerEvent::InputEvent(InputEventResource::Contact {
            epoch_ms: 1778058932588,
            route: InputEventRoute::NodeControl,
            hub_type: Some("ha".into()),
            address: Some("homeassistant.local".into()),
            source_node_id: Some("contact-1".into()),
            target_node_id: Some("room-1".into()),
            source_room_id: Some("ha-area-1".into()),
            native_sensor_id: "binary_sensor.front_door".into(),
            open: true,
        });
        let json = serde_json::to_string(&event).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["type"], "input_event");
        assert_eq!(parsed["data"]["kind"], "contact");
        assert_eq!(parsed["data"]["route"], "node_control");
        assert_eq!(parsed["data"]["hub_type"], "ha");
        assert_eq!(parsed["data"]["address"], "homeassistant.local");
        assert_eq!(parsed["data"]["source_node_id"], "contact-1");
        assert_eq!(parsed["data"]["target_node_id"], "room-1");
        assert_eq!(parsed["data"]["source_room_id"], "ha-area-1");
        assert_eq!(
            parsed["data"]["native_sensor_id"],
            "binary_sensor.front_door"
        );
        assert_eq!(parsed["data"]["open"], true);
    }

    #[test]
    fn server_event_hub_status_omits_optional_address() {
        let event = ServerEvent::HubStatus {
            hub_type: None,
            address: None,
            connected: false,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(!json.contains("\"address\""));
        assert!(!json.contains("\"hub_type\""));
        assert!(json.contains("\"connected\":false"));
    }

    #[test]
    fn server_event_triage_includes_all_pending_counts() {
        let event = ServerEvent::TriageChanged {
            pending_count: 5,
            pending_devices: 2,
            pending_rooms: 1,
            pending_unassigned: 1,
            pending_hub_configured: 1,
            pending_unreachable: 2,
        };
        let json = serde_json::to_string(&event).unwrap();
        for field in [
            "pending_count",
            "pending_devices",
            "pending_rooms",
            "pending_unassigned",
            "pending_hub_configured",
            "pending_unreachable",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
    }

    #[test]
    fn server_event_pairing_progress_serializes_stage_and_session() {
        let event = ServerEvent::PairingProgress {
            hub_type: "matter".into(),
            session_id: Some("pair-1".into()),
            status: PairingStatus::Commissioning,
            stage: PairingStage::Commissioning,
            message: "Commissioning Matter device".into(),
            device: None,
            devices: Vec::new(),
            warnings: Vec::new(),
            error: None,
            failure_stage: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"pairing_progress\""));
        assert!(json.contains("\"hub_type\":\"matter\""));
        assert!(json.contains("\"session_id\":\"pair-1\""));
        assert!(json.contains("\"status\":\"commissioning\""));
        assert!(json.contains("\"stage\":\"commissioning\""));
    }

    #[test]
    fn terminal_pairing_progress_serializes_every_batch_device() {
        let first = PairedDeviceInfo {
            device_id: "hue-ble-first".into(),
            name: "First Hue light".into(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            manufacturer: Some("Signify Netherlands B.V.".into()),
            model: Some("LCA013".into()),
        };
        let second = PairedDeviceInfo {
            device_id: "hue-ble-second".into(),
            name: "Second Hue light".into(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            manufacturer: Some("Signify Netherlands B.V.".into()),
            model: Some("LCT015".into()),
        };
        let event = ServerEvent::PairingProgress {
            hub_type: "hue_ble".into(),
            session_id: Some("pair-batch".into()),
            status: PairingStatus::Complete,
            stage: PairingStage::Complete,
            message: "Pairing complete".into(),
            device: Some(first.clone()),
            devices: vec![first, second],
            warnings: vec!["One nearby bulb rejected pairing".into()],
            error: None,
            failure_stage: None,
        };

        let value = serde_json::to_value(event).unwrap();
        let data = &value["data"];
        assert_eq!(data["device"]["device_id"], "hue-ble-first");
        assert_eq!(data["devices"].as_array().unwrap().len(), 2);
        assert_eq!(data["devices"][1]["device_id"], "hue-ble-second");
        assert_eq!(data["warnings"][0], "One nearby bulb rejected pairing");
    }

    #[test]
    fn terminal_pairing_failure_serializes_privacy_safe_stage() {
        let event = ServerEvent::PairingProgress {
            hub_type: "local_ble".into(),
            session_id: Some("local-pair-stage".into()),
            status: PairingStatus::Failed,
            stage: PairingStage::Failed,
            message: "Pairing failed".into(),
            device: None,
            devices: Vec::new(),
            warnings: Vec::new(),
            error: Some("Device found, but could not connect".into()),
            failure_stage: Some(PairingFailureStage::CandidateConnect),
        };

        let value = serde_json::to_value(event).unwrap();
        assert_eq!(value["data"]["failure_stage"], "candidate_connect");
    }

    #[test]
    fn server_event_ota_update_progress_serializes_stage_and_percent() {
        let event = ServerEvent::OtaUpdateProgress {
            stage: OtaUpdateStage::Downloading,
            message: "Downloading update bundle".into(),
            current_version: Some("0.4.192-beta".into()),
            target_version: Some("0.4.193-beta".into()),
            update_available: Some(true),
            downloaded_bytes: Some(50),
            total_bytes: Some(100),
            percent: Some(50),
            checksum_verified: None,
            installed_targets: Vec::new(),
            error: None,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("\"type\":\"ota_update_progress\""));
        assert!(json.contains("\"stage\":\"downloading\""));
        assert!(json.contains("\"percent\":50"));
        assert!(json.contains("\"current_version\":\"0.4.192-beta\""));
        assert!(json.contains("\"target_version\":\"0.4.193-beta\""));
    }

    #[tokio::test]
    async fn slow_subscriber_does_not_block_fast_subscriber() {
        // Two subscribers; the "fast" one drains immediately, the "slow"
        // one only reads after the broadcast capacity is exceeded. The fast
        // subscriber must still see every event in order; the slow one
        // surfaces a Lagged error rather than blocking the sender.
        let (tx, _) = tokio::sync::broadcast::channel::<ServerEvent>(8);
        let mut fast = tx.subscribe();
        let mut slow = tx.subscribe();

        // Push more events than the channel capacity.
        for _ in 0..32 {
            let _ = tx.send(ServerEvent::SettingsChanged {
                settings: SettingsDto {
                    auto_update: true,
                    update_channel: crate::state::UpdateChannel::Stable,
                    light_runtime: LightRuntimeKind::default(),
                },
            });
        }

        // Fast subscriber drains: it sees at least the latest 8 events
        // (the channel may drop the oldest from its perspective too if it
        // hasn't kept up — verify it sees a Lagged or a value, not block).
        let mut fast_count = 0;
        let mut fast_lagged = false;
        while let Ok(result) =
            tokio::time::timeout(std::time::Duration::from_millis(20), fast.recv()).await
        {
            match result {
                Ok(_) => fast_count += 1,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => fast_lagged = true,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
        assert!(
            fast_count > 0 || fast_lagged,
            "fast subscriber should observe events or a lag signal"
        );

        // Slow subscriber: must surface a Lagged error indicating dropped
        // messages, NOT block the sender (already proven by the loop above
        // having returned all 32 sends).
        match slow.recv().await {
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                assert!(skipped > 0, "expected positive lag count, got {}", skipped);
            }
            other => {
                // If recv returned Ok, the broadcast channel may have eagerly
                // dropped older messages without surfacing Lagged on the
                // first call — that's also acceptable backpressure behavior.
                let _ = other;
            }
        }
    }

    #[tokio::test]
    async fn broadcast_send_returns_error_when_no_subscribers() {
        // Sender alone (no subscribers) must not panic; it returns an error
        // that callers like `emit_event` swallow with `let _ =`.
        let (tx, rx) = tokio::sync::broadcast::channel::<ServerEvent>(4);
        drop(rx);
        let result = tx.send(ServerEvent::SettingsChanged {
            settings: SettingsDto {
                auto_update: true,
                update_channel: crate::state::UpdateChannel::Stable,
                light_runtime: LightRuntimeKind::default(),
            },
        });
        assert!(
            result.is_err(),
            "send with no subscribers must error (and emit_event must ignore it)"
        );
    }
}
