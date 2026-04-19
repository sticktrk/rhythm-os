//! Server-Sent Events types for real-time state updates.
//!
//! Used by the SSE endpoint (`GET /api/events`) to push changes to
//! connected clients. Gated behind `desktop` feature.

use serde::Serialize;

use rhythm_core::{NodeSnapshot, RhythmMode, RoomModeState, RoomProfileSettings};

use crate::state::MotionSnapshot;

/// A server event broadcast to all connected SSE clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Addressable node state changed (after button, periodic tick, etc.).
    NodeState { nodes: Vec<NodeStateEvent> },
    /// Motion timer state changed.
    MotionTimer { timers: Vec<MotionTimerEvent> },
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
    /// Settings changed (client should refetch).
    SettingsChanged,
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
    },
}

/// Addressable node rhythm state in an SSE event.
#[derive(Clone, Debug, Serialize)]
pub struct NodeStateEvent {
    pub id: String,
    /// Which hub types can address this node (e.g. ["hue"], ["hue", "matter"]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub hub_types: Vec<String>,
    pub mode: RhythmMode,
    pub state: RoomModeState,
    pub rhythm_enabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    /// Whether lights are currently on in this room.
    pub lights_on: bool,
    /// Whether a global mode transition fade is currently in progress.
    pub transitioning: bool,
    /// Effective brightness percentage (1-100) after offsets.
    pub brightness: u8,
    /// Effective color temperature in Kelvin.
    pub kelvin: u16,
    /// `true` when this event was triggered by the periodic rhythm tick.
    /// Absent (or `false`) for user actions, polls, and other state changes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tick: bool,
    #[serde(default, skip_serializing_if = "RoomProfileSettings::is_empty")]
    pub profile_settings: RoomProfileSettings,
}

#[derive(Clone, Debug)]
pub(crate) struct NodeStateEventParams {
    pub hub_types: Vec<String>,
    pub mode: RhythmMode,
    pub state: RoomModeState,
    pub lights_on: bool,
    pub transitioning: bool,
    pub brightness: u8,
    pub kelvin: u16,
}

impl NodeStateEvent {
    /// Build from an engine node snapshot with display values.
    pub(crate) fn from_snapshot(snap: &NodeSnapshot, params: NodeStateEventParams) -> Self {
        let NodeStateEventParams {
            hub_types,
            mode,
            state,
            lights_on,
            transitioning,
            brightness,
            kelvin,
        } = params;
        Self {
            id: snap.id.clone(),
            hub_types,
            mode,
            state,
            rhythm_enabled: snap.rhythm_enabled,
            time_offset: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
            lights_on,
            transitioning,
            brightness,
            kelvin,
            tick: false,
            profile_settings: snap.profile_settings.clone(),
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
