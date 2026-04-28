//! User presses bottom button DURING an in-flight Sleep→Day mode transition.
//!
//! When sleep→day fires for a room that was lit during sleep, the engine
//! issues a fade-in command (the "expired_transitions" counter in periodic
//! logs counts these). If the user presses the bottom button while that fade
//! is playing on the bulb, the OffPress must dispatch 1% and *that* must be
//! the last command — no later automatic dispatch should overwrite it.

mod harness;

use harness::*;

use rhythm_core::{
    default_mode_configs, ModeTransitionConfig, ModeTransitionTrigger, RhythmMode,
    DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;

#[test]
fn off_press_during_mode_transition_fade_dims_to_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();

    h.set_mode_configs(default_mode_configs());
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

    // Lights were on through sleep mode — this is what makes mode-apply
    // schedule a fade-in for the room.
    h.action("master", "on").unwrap();
    h.set_lights_on("master", true);
    spy.reset();

    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .unwrap();

    // Mode-apply may or may not dispatch a fade-in synchronously depending on
    // mode-config gating; either way, the next press is what matters.
    let fade_bri = spy
        .turn_on_calls()
        .last()
        .map(|(_, c)| c.brightness)
        .unwrap_or(0);

    spy.reset();

    // User presses bottom button right after the transition fired.
    h.action("master", "off").unwrap();

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert!(!calls.is_empty(), "off press post-transition must dispatch turn_on");
    assert_eq!(
        calls.last().unwrap().1.brightness,
        1,
        "off press post-transition must end at 1% (fade was at {}, last call was {})",
        fade_bri,
        calls.last().unwrap().1.brightness
    );

    let snap = h.snapshot("master").unwrap();
    assert!(snap.soft_off, "final state must be soft-off");
    assert!(!snap.hard_off);
}
