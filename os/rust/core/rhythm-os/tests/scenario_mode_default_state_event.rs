//! Regression for issue #17: when a mode transition flips a room from
//! `HardOff` to `Idle` (or `Active`) via the destination mode's `room_defaults`,
//! the server must broadcast a `NodeState` event for that room *as the engine
//! state changes* — not only later when the dispatcher worker eventually
//! processes the queued lighting command.
//!
//! Bug shape (sleep_to_day, Master room from the user's bundle):
//!   - Master is hard-off (user pressed LightsOff before bed).
//!   - Day mode_config has `room_defaults: [{ master: Idle }]`.
//!   - Sleep→Day transition fires.
//!   - `apply_room_mode_defaults` flips Master HardOff → Idle in the engine
//!     and updates observed_power, but only the `HardOff` *target* branch
//!     emits a `NodeState` event (commands.rs:3397). For Active/Idle targets
//!     the engine mutation is silent.
//!   - The downstream dispatcher queues an `ApplyNodeCommand` for Master,
//!     but that runs on a separate thread with phase-gap pacing — in the
//!     user's log Master's command landed ~9s after the mode change. Until
//!     then, every SSE consumer's last-seen state for Master is `HardOff`,
//!     and the app has no way to know the engine has moved on.
//!
//! This test sets up the scenario, subscribes to the broadcast channel, and
//! triggers the transition without draining any queued work items. Today it
//! observes zero `NodeState` events for Master — proving the staleness window.

mod harness;

use harness::*;

use rhythm_core::{
    ModeConfig, ModeTransitionConfig, ModeTransitionTrigger, RhythmMode, RoomModeDefault,
    RoomModeState, DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;
use rhythm_os::server_event::ServerEvent;
use rhythm_os::state::WorkItem;

#[test]
fn mode_default_flip_emits_node_state_event_synchronously() {
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, _spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    let master_id = h.resolve("master");

    // Mirror the user's mode config from issue #17:
    //   Day:   master = idle  (dim during the day, room not in use)
    //   Sleep: master = active (lights via curve while bedroom is in use)
    let mut day_config = ModeConfig::default_for_mode(RhythmMode::Day);
    day_config.room_defaults = vec![RoomModeDefault {
        room_id: master_id.clone(),
        state: RoomModeState::Idle,
    }];
    let mut sleep_config = ModeConfig::default_for_mode(RhythmMode::Sleep);
    sleep_config.room_defaults = vec![RoomModeDefault {
        room_id: master_id.clone(),
        state: RoomModeState::Active,
    }];
    h.set_mode_configs(vec![day_config, sleep_config]);

    {
        let mut s = h.state.lock().unwrap();
        let transition = ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_trigger(ModeTransitionTrigger::Sunrise);
        s.set_mode_transition_configs(vec![transition]);
    }

    // Move into Sleep through the public API so the runtime's active profile
    // is updated too (otherwise sync_active_mode_from_runtime would resync
    // active_mode back to Day on the next do_node_action call).
    commands::do_set_active_mode(&h.state, RhythmMode::Sleep).unwrap();

    // User puts the bedroom in hard_off before bed.
    h.action("master", "on").unwrap();
    h.action("master", "lights_off").unwrap();
    let pre_snap = h.snapshot("master").unwrap();
    assert!(
        pre_snap.hard_off,
        "setup: master must be hard_off going into sleep_to_day"
    );

    // Subscribe to the broadcast channel before triggering the transition.
    // Install a work_tx so apply_active_mode_outputs queues lighting commands
    // instead of dispatching inline — production always uses the async path,
    // so we should too. We deliberately *do not* drain work_rx, so the only
    // events we observe come from synchronous emits during the mode change.
    let (tx, mut rx) = tokio::sync::broadcast::channel(64);
    let (work_tx, _work_rx_keep_alive) = std::sync::mpsc::sync_channel::<WorkItem>(64);
    {
        let mut s = h.state.lock().unwrap();
        s.event_tx = Some(tx);
        s.work_tx = Some(work_tx);
    }

    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .unwrap();

    // Sanity-check that apply_room_mode_defaults actually ran — without this
    // the staleness assertion below would pass for the wrong reason.
    let mid_snap = h.snapshot("master").unwrap();
    assert!(
        !mid_snap.hard_off,
        "engine: hard_off must clear when day room_default Idle activates"
    );
    assert!(
        mid_snap.soft_off,
        "engine: soft_off must be set for Idle target"
    );

    // Drain everything broadcast so far. We are *not* processing the queued
    // ApplyNodeCommand work items, so the only NodeState events that can
    // exist here are ones emitted directly by the mode-change apply.
    let mut node_state_events_for_master: Vec<rhythm_os::server_event::NodeStateEvent> = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let ServerEvent::NodeState { nodes } = event {
            for node in nodes {
                if node.id == master_id {
                    node_state_events_for_master.push(node);
                }
            }
        }
    }

    assert!(
        !node_state_events_for_master.is_empty(),
        "apply_room_mode_defaults must emit at least one NodeState event for master \
         when it flips HardOff -> Idle. Without this, every SSE consumer keeps showing \
         HardOff until the dispatcher worker eventually processes the queued lighting \
         command (~9s in production)."
    );

    let last = node_state_events_for_master
        .last()
        .expect("checked non-empty above");
    assert_eq!(
        last.state,
        RoomModeState::Idle,
        "the emitted NodeState event for master must report state=Idle, matching the engine"
    );
}
