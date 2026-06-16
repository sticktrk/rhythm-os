//! Reproduction of the morning-bug context.
//!
//! Real bundle (rpiz, 2026-04-28T10:14 UTC) showed the user's room in
//! `hard_off=true` immediately after a `Sleep → Day` transition that fired at
//! ~05:59 EDT. Three minutes later the user pressed the bottom button on the
//! dimmer; logs show `OffPress` was correctly received and dispatched. The
//! hard-off pivot means that press should now remain a full off command.
//!
//! These tests exercise that specific state shape: hard-off entering the press,
//! and the same shape after a mode transition (which under the dispatch-queue
//! invalidator should drop any stale sleep-era queued commands). If a regression
//! lets a non-off command land for `OffPress` here, this is the
//! test that should catch it.

mod harness;

use harness::*;

use rhythm_core::{
    ModeTransitionConfig, ModeTransitionTrigger, RhythmMode, DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;

#[test]
fn off_press_while_hard_off_sends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[("master", "Master")]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118); // 06:02 EDT, late April
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    // Establish hard-off — same state the room was in coming out of Sleep mode
    // before the user's press.
    h.action("master", "on").unwrap();
    h.action("master", "lights_off").unwrap();
    spy.reset();

    h.action("master", "off").unwrap();

    let off_calls = spy.turn_off_calls();
    assert_eq!(
        off_calls.len(),
        1,
        "off press from hard-off must dispatch one turn_off"
    );
    assert_eq!(
        spy.turn_on_calls().len(),
        0,
        "off press from hard-off must not dispatch legacy soft-off"
    );
}

#[test]
fn off_press_immediately_after_sleep_to_day_transition_sends_hard_off() {
    let (rooms, devices) = rooms_with_lights(&[
        ("master", "Master"),
        ("kitchen", "Kitchen"),
        ("guest_bath", "Guest Bath"),
    ]);
    let (h, spy) = TestHarness::with_spy_controller_at(6.04, 118);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    // Seed Sleep mode and a Sleep→Day sunrise transition, mirroring the user's config.
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

    // Sleep state typically leaves rooms hard-off; reproduce that explicitly.
    h.action("master", "on").unwrap();
    h.action("master", "lights_off").unwrap();
    h.action("kitchen", "lights_off").unwrap();
    h.action("guest_bath", "lights_off").unwrap();

    // Trigger the Sleep→Day transition. This bumps `light_dispatch_generation`
    // and runs the mode-apply pass; any queued sleep-era commands targeting
    // `master` are now stale.
    commands::do_set_active_mode_with_trigger(
        &h.state,
        RhythmMode::Day,
        ModeTransitionTrigger::Sunrise,
    )
    .unwrap();

    spy.reset();

    // The user's actual press: bottom button on Master while still hard-off,
    // moments after the day transition.
    h.action("master", "off").unwrap();

    let off_calls = spy.turn_off_calls();
    let master_off_calls: Vec<_> = off_calls
        .iter()
        .filter(|rid| *rid == &h.resolve("master"))
        .collect();
    assert_eq!(
        master_off_calls.len(),
        1,
        "exactly one turn_off for master, got {} (all off calls: {:?})",
        master_off_calls.len(),
        off_calls
    );
    assert!(spy.turn_on_calls().is_empty());
}
