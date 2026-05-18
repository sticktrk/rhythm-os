//! Scenario: Hub disconnect should NOT clear motion timer state.
//!
//! When the SSE connection drops and reconnects, `HubEvent::Disconnected` fires.
//! Motion timers are local state (based on `Instant::now()` timestamps) and don't
//! depend on the hub being connected to count down. Clearing them on disconnect
//! means lights never turn off after a transient SSE blip.

mod harness;

use std::time::Instant;

use harness::{motion_sensor, room, TestHarness};
use rhythm_os::event_loop::{handle_hub_event, MotionSourceState, MotionTimerState};
use rhythm_os::hub::HubEvent;

// ============================================================================
// Scenario: Motion timers survive hub disconnect
// ============================================================================

/// Motion timer state (sensors, motion_owned, warning_active) must be preserved
/// across a `HubEvent::Disconnected`. The timers are purely local — they track
/// `Instant` timestamps that keep counting regardless of hub connectivity.
#[test]
fn motion_timers_survive_hub_disconnect() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    let resolved = harness.resolve("hallway");

    // -- Setup: populate motion timer state as if motion was detected --
    let mut motion = MotionTimerState::new();
    motion.sensors.insert(
        "sensor_01".to_string(),
        MotionSourceState {
            source_node_id: "sensor_01".to_string(),
            target_node_id: resolved.clone(),
            stopped_at: None,
            stopped_at_epoch_ms: None,
        },
    ); // active
    motion.motion_owned.insert(resolved.clone());

    assert!(!motion.sensors.is_empty(), "precondition: sensor tracked");
    assert!(
        motion.motion_owned.contains(&resolved),
        "precondition: room is motion-owned"
    );

    // -- Action: hub disconnects --
    handle_hub_event(
        &harness.state,
        HubEvent::Disconnected {
            hub_key: None,
            reason: "SSE connection dropped".to_string(),
        },
        &mut motion,
    );

    // -- Assert: motion state preserved --
    assert!(
        !motion.sensors.is_empty(),
        "sensors should NOT be cleared on disconnect"
    );
    assert!(
        motion.motion_owned.contains(&resolved),
        "motion_owned should NOT be cleared on disconnect"
    );
}

/// Warning-active state should also survive disconnect.
#[test]
fn warning_active_survives_hub_disconnect() {
    let harness = TestHarness::new().with_discovery(
        vec![room("hallway", "Hallway")],
        vec![motion_sensor("motion_01", "hallway")],
    );
    harness.sync();

    let resolved = harness.resolve("hallway");

    // -- Setup: room in warning dim state (all sensors cleared, countdown near end) --
    let mut motion = MotionTimerState::new();
    motion.sensors.insert(
        "sensor_01".to_string(),
        MotionSourceState {
            source_node_id: "sensor_01".to_string(),
            target_node_id: resolved.clone(),
            stopped_at: Some(Instant::now()),
            stopped_at_epoch_ms: Some(1_700_000_000_000),
        },
    );
    motion.motion_owned.insert(resolved.clone());
    motion.warning_active.insert(resolved.clone());

    // -- Action: hub disconnects --
    handle_hub_event(
        &harness.state,
        HubEvent::Disconnected {
            hub_key: None,
            reason: "SSE timeout".to_string(),
        },
        &mut motion,
    );

    // -- Assert: all three collections preserved --
    assert!(
        !motion.sensors.is_empty(),
        "sensors should survive disconnect"
    );
    assert!(
        motion.motion_owned.contains(&resolved),
        "motion_owned should survive disconnect"
    );
    assert!(
        motion.warning_active.contains(&resolved),
        "warning_active should survive disconnect"
    );
}
