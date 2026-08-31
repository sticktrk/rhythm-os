//! Mode transitions own their configured target state, including when the
//! room was hard-off before the transition.

mod harness;

use harness::*;

use rhythm_core::{
    default_mode_configs, ModeTransitionConfig, ModeTransitionTrigger, RhythmMode, RoomModeDefault,
    RoomModeState, DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;

#[test]
fn mode_transition_wakes_hard_off_room_then_off_press_restores_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    let master_id = h.resolve("master");
    let mut mode_configs = default_mode_configs();
    mode_configs
        .iter_mut()
        .find(|config| config.mode == RhythmMode::Day)
        .unwrap()
        .room_defaults
        .push(RoomModeDefault {
            room_id: master_id,
            state: RoomModeState::Active,
        });
    h.set_mode_configs(mode_configs);
    {
        let mut s = h.state.lock().unwrap();
        s.set_mode_transition_configs(vec![ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_trigger(ModeTransitionTrigger::Sunrise)]);
    }
    commands::do_set_active_mode(&h.state, RhythmMode::Sleep).unwrap();

    h.action("master", "on").unwrap();
    h.action("master", "lights_off").unwrap();
    assert!(
        h.snapshot("master").unwrap().hard_off,
        "setup: hard_off must be true going into transition"
    );
    spy.reset();

    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .unwrap();

    assert!(
        !h.snapshot("master").unwrap().hard_off,
        "the configured Day target must replace the prior hard-off state"
    );
    assert_eq!(spy.turn_on_calls().len(), 1);

    spy.reset();
    h.action("master", "off").unwrap();

    assert_eq!(spy.turn_off_calls().len(), 1);
    assert_eq!(spy.turn_on_calls().len(), 0);
    let post_snap = h.snapshot("master").unwrap();
    assert!(!post_snap.soft_off, "off press must not establish soft-off");
    assert!(post_snap.hard_off, "off press must establish hard-off");
}
