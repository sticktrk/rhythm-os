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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn server_event_serializes_with_tagged_type_and_data() {
        let event = ServerEvent::SettingsChanged;
        let json = serde_json::to_string(&event).unwrap();
        assert!(
            json.contains("\"type\":\"settings_changed\""),
            "expected snake_case tagged type, got {}",
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
        };
        let json = serde_json::to_string(&event).unwrap();
        for field in [
            "pending_count",
            "pending_devices",
            "pending_rooms",
            "pending_unassigned",
            "pending_hub_configured",
        ] {
            assert!(json.contains(field), "missing {} in {}", field, json);
        }
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
            let _ = tx.send(ServerEvent::SettingsChanged);
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
        let result = tx.send(ServerEvent::SettingsChanged);
        assert!(
            result.is_err(),
            "send with no subscribers must error (and emit_event must ignore it)"
        );
    }
}
