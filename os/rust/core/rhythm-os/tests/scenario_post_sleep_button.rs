//! Reproduction of the morning-bug context.
//!
//! Real bundle (rpiz, 2026-04-28T10:14 UTC) showed the user's room in
//! `hard_off=true` immediately after a `Sleep → Day` transition that fired at
//! ~05:59 EDT. Three minutes later the user pressed the bottom button on the
//! dimmer; logs show `OffPress` was correctly received and dispatched, but the
//! lights "weren't dim".
//!
//! These tests exercise that specific state shape: hard-off entering the press,
//! and the same shape after a mode transition (which under the dispatch-queue
//! invalidator should drop any stale sleep-era queued commands). If a regression
//! lets a non-1% command land for non-powersave `OffPress` here, this is the
//! test that should catch it.

mod harness;

use harness::*;

use rhythm_core::{
    ModeTransitionConfig, ModeTransitionTrigger, RhythmMode, DEFAULT_MODE_TRANSITION_DURATION_MS,
};
use rhythm_os::commands;

#[test]
fn off_press_while_hard_off_dims_to_one_percent() {
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

    assert_eq!(spy.turn_off_calls().len(), 0, "soft-off must not hard-off");
    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        1,
        "off press from hard-off must dispatch one turn_on"
    );
    assert_eq!(
        calls[0].1.brightness, 1,
        "off press from hard-off must be 1%, got {}",
        calls[0].1.brightness
    );
}

#[test]
fn off_press_immediately_after_sleep_to_day_transition_dims_to_one_percent() {
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

    assert_eq!(
        spy.turn_off_calls().len(),
        0,
        "post-transition soft-off must not hard-off"
    );
    let calls = spy.turn_on_calls();
    let master_calls: Vec<_> = calls
        .iter()
        .filter(|(rid, _)| rid == &h.resolve("master"))
        .collect();
    assert_eq!(
        master_calls.len(),
        1,
        "exactly one turn_on for master, got {} (all calls: {:?})",
        master_calls.len(),
        calls
            .iter()
            .map(|(rid, c)| (rid.clone(), c.brightness))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        master_calls[0].1.brightness, 1,
        "post-transition off press must be 1%, got {}",
        master_calls[0].1.brightness
    );
}
