//! Regression for issue #26: when the user pairs a roomless light device
//! (the bundle was a Matter Cync A19 commissioned outside any room), manual
//! mode transitions and `PUT /api/mode` re-applies must reach that device,
//! not silently wait for the next periodic tick.
//!
//! Bug shape:
//!   - `apply_active_mode_outputs` consulted `engine_all_room_snapshots()`,
//!     which the production runtime filters to `kind.is_room()`. Standalone
//!     `LightDevice` nodes (kind=LightDevice, parent_id=None) are *not* rooms,
//!     so they were dropped from the iteration and never received a command.
//!   - The user's bundle (issue #26) shows two `sleep_to_day` triggers and a
//!     `PUT /api/mode` between 10:33 and 10:34 — none of which reached the
//!     bulb. The first dispatch landed on the next periodic cycle ~3 minutes
//!     later, which the user reported as "single matter bulb does not seem to
//!     be updating to new light tuning parameters."
//!
//! This test sets up a roomless light device, drives a manual transition with
//! a `SpyLightController` attached, and asserts the spy received the command.
//! Pre-fix the spy stays empty; post-fix it records a `TurnOn`.

mod harness;

use harness::*;

use rhythm_core::{
    ModeTransitionConfig, ModeTransitionTrigger, RhythmMode, RoomModeState,
    DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;
use rhythm_os::state::{ObservedPowerSource, ObservedPowerState};

#[test]
fn manual_transition_dispatches_to_roomless_light_device() {
    // 10 AM in May — the curve is well-defined and we don't risk landing on
    // an idle/HardOff target by accident.
    let (h, spy) = TestHarness::with_spy_controller_at(10.0, 125);
    let h = h.with_discovery(vec![], vec![light("matter-100", "")]);
    h.sync();

    let standalone_id = {
        let s = h.state.lock().unwrap();
        s.canonical_registry
            .find_by_native_id(&h.hub_key, "matter-100")
            .expect("matter-100 should be canonical-registered")
            .id
            .clone()
    };

    {
        let snap = h
            .state
            .lock()
            .unwrap()
            .hub_runtime()
            .expect("runtime present after sync")
            .engine_node_snapshot(&standalone_id)
            .expect("standalone device should exist as a runtime node");
        assert_eq!(
            snap.kind,
            rhythm_core::LightNodeKind::LightDevice,
            "discovered roomless light must surface as a LightDevice node"
        );
        assert!(
            snap.parent_id.is_none(),
            "roomless light must have no parent — that's what makes it standalone"
        );
    }

    // Mark rhythm-enabled and Active so the periodic eligibility filter and
    // visibility check both pass — same as the user's setup before the
    // sleep_to_day trigger in the bundle.
    commands::do_node_preferences_set(
        &h.state,
        &standalone_id,
        Some(true),
        Some(false),
        None,
        Some(RoomModeState::Active),
        None,
        false,
    )
    .expect("enable rhythm on the standalone device");

    // The mode-apply path only dispatches to "visible" Active nodes
    // (lights_on per observed cache). Mirror what the engine event loop does
    // when a real device reports its first ON state.
    {
        let mut s = h.state.lock().unwrap();
        s.room_observed_power.insert(
            standalone_id.clone(),
            ObservedPowerState::new(true, ObservedPowerSource::Command),
        );
    }

    // Configure a Sleep -> Day transition so do_set_active_mode_with_trigger
    // resolves the same path do_trigger_transition does in production.
    {
        let mut s = h.state.lock().unwrap();
        s.active_mode = RhythmMode::Sleep;
        s.set_mode_transition_configs(vec![ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_trigger(ModeTransitionTrigger::Sunrise)]);
    }

    // No work_tx installed -> apply_active_mode_outputs runs inline through
    // apply_room_commands_inline -> runtime.apply_room_command -> spy.
    let pre_dispatch = spy.turn_on_calls().len();
    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .expect("manual transition should succeed");

    let new_calls: Vec<_> = spy.turn_on_calls().into_iter().skip(pre_dispatch).collect();
    assert!(
        new_calls.iter().any(|(id, _)| id == &standalone_id),
        "manual sleep_to_day transition must dispatch to the standalone \
         LightDevice; spy saw turn_on calls = {:?}",
        new_calls
    );
}
