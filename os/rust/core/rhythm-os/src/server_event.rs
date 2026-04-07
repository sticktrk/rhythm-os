//! Server-Sent Events types for real-time state updates.
//!
//! Used by the SSE endpoint (`GET /api/events`) to push changes to
//! connected clients. Gated behind `desktop` feature.

use serde::Serialize;

use rhythm_core::{RoomProfileSettings, RoomSnapshot};

use crate::state::MotionSnapshot;

/// A server event broadcast to all connected SSE clients.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum ServerEvent {
    /// Room rhythm state changed (after button, periodic tick, etc.).
    RoomState { rooms: Vec<RoomStateEvent> },
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
    /// Room topology changed (client should re-hello).
    RoomsChanged,
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
    },
}

/// Room rhythm state in an SSE event.
#[derive(Clone, Debug, Serialize)]
pub struct RoomStateEvent {
    pub id: String,
    pub rhythm_enabled: bool,
    pub time_offset: f32,
    pub brightness_offset: f32,
    pub soft_off: bool,
    /// Whether lights are currently on in this room.
    pub lights_on: bool,
    /// Effective brightness percentage (1-100) after offsets.
    pub brightness: u8,
    /// Effective color temperature in Kelvin.
    pub kelvin: u16,
    /// `true` when this event was triggered by the periodic rhythm tick.
    /// Absent (or `false`) for user actions, polls, and other state changes.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tick: bool,
    #[serde(rename = "room_profile", default, skip_serializing_if = "RoomProfileSettings::is_empty")]
    pub room_profile: RoomProfileSettings,
}

impl RoomStateEvent {
    /// Build from an engine room snapshot with display values.
    pub fn from_snapshot(
        snap: &RoomSnapshot,
        lights_on: bool,
        brightness: u8,
        kelvin: u16,
        room_profile: RoomProfileSettings,
    ) -> Self {
        Self {
            id: snap.id.clone(),
            rhythm_enabled: snap.rhythm_enabled,
            time_offset: snap.time_offset_minutes,
            brightness_offset: snap.brightness_offset,
            soft_off: snap.soft_off,
            lights_on,
            brightness,
            kelvin,
            tick: false,
            room_profile,
        }
    }
}

/// Motion timer state in an SSE event.
#[derive(Clone, Debug, Serialize)]
pub struct MotionTimerEvent {
    pub room_id: String,
    pub motion_active: bool,
    pub motion_owned: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remaining_secs: Option<u64>,
    pub timeout_secs: u64,
    pub warning_active: bool,
}

impl MotionTimerEvent {
    /// Build from a motion snapshot.
    pub fn from_snapshot(room_id: &str, snap: &MotionSnapshot) -> Self {
        Self {
            room_id: room_id.to_string(),
            motion_active: snap.motion_active,
            motion_owned: snap.motion_owned,
            remaining_secs: snap.remaining_secs,
            timeout_secs: snap.timeout_secs,
            warning_active: snap.warning_active,
        }
    }
}
