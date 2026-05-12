//! Mode transitions with `preserve_hard_off=true` must keep hard-off rooms
//! hard-off, but a subsequent non-powersave `OffPress` must still soft-off
//! them to 1%.
//!
//! The factory-default Sleep↔Day transitions both have `preserve_hard_off=true`
//! (see `factory_default_config.rs:267,277`), which is what the user runs in
//! production. Coverage for the post-transition button shape needs to exercise
//! that flag explicitly — `scenario_post_sleep_button` doesn't seed it
//! (default `Manual` trigger uses the same default but the path matters).

mod harness;

use harness::*;

use rhythm_core::{
    default_mode_configs, ModeTransitionConfig, ModeTransitionTrigger, RhythmMode,
    DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;

#[test]
fn preserve_hard_off_transition_keeps_hard_off_then_off_press_soft_offs() {
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.set_mode_configs(default_mode_configs());
    {
        let mut s = h.state.lock().unwrap();
        s.active_mode = RhythmMode::Sleep;
        // Default constructor sets preserve_hard_off = true; assert that
        // anchor explicitly so a future change to the default doesn't
        // silently weaken this test.
        let transition = ModeTransitionConfig::new(
            RhythmMode::Sleep,
            RhythmMode::Day,
            DEFAULT_MODE_TRANSITION_DURATION_MS,
        )
        .with_trigger(ModeTransitionTrigger::Sunrise);
        assert!(
            transition.preserve_hard_off,
            "test premise: preserve_hard_off defaults true"
        );
        s.set_mode_transition_configs(vec![transition]);
    }

    h.action("master", "on").unwrap();
    h.action("master", "lights_off").unwrap();
    let pre_snap = h.snapshot("master").unwrap();
    assert!(
        pre_snap.hard_off,
        "setup: hard_off must be true going into transition"
    );
    spy.reset();

    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .unwrap();

    let mid_snap = h.snapshot("master").unwrap();
    assert!(
        mid_snap.hard_off,
        "preserve_hard_off=true must keep hard_off through Sleep→Day"
    );
    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "preserved hard-off rooms must not be hard-off'd again"
    );

    spy.reset();

    h.action("master", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert_eq!(calls.len(), 1, "off press must dispatch one turn_on");
    assert_eq!(calls[0].1.brightness, 1, "soft-off brightness must be 1%");

    let post_snap = h.snapshot("master").unwrap();
    assert!(post_snap.soft_off, "off press must establish soft-off");
    assert!(
        !post_snap.hard_off,
        "off press must clear hard_off (recovery path)"
    );
}
