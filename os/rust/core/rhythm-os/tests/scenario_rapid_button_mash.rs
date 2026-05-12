//! Rapid button mashing must not produce inconsistent dispatch state.
//!
//! The morning bundle showed three OffPress events in 17 seconds. Pinning that
//! every non-powersave dispatch in a fast burst stays at 1% catches any state
//! machine that interleaves Active/Idle on rapid input.

mod harness;

use harness::*;

#[test]
fn five_off_presses_in_a_row_all_dispatch_one_percent() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    spy.reset();

    for _ in 0..5 {
        h.action("kitchen", "off").unwrap();
    }

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0, "no hard-off across mashes");
    assert!(
        calls.len() >= 5,
        "every press should dispatch (got {})",
        calls.len()
    );
    for (i, (_, cmd)) in calls.iter().enumerate() {
        assert_eq!(
            cmd.brightness, 1,
            "mash press {} brightness must be 1%, got {}",
            i, cmd.brightness
        );
    }

    let snap = h.snapshot("kitchen").unwrap();
    assert!(snap.soft_off, "final state must be soft-off");
    assert!(!snap.hard_off, "final state must not be hard-off");
}

#[test]
fn alternating_on_off_mash_keeps_each_state_correct() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    let sequence = ["on", "off", "on", "off", "on", "off"];
    for action in sequence.iter() {
        h.action("kitchen", action).unwrap();
    }

    let calls = spy.turn_on_calls();
    assert_eq!(spy.turn_off_calls().len(), 0);
    assert!(calls.len() >= sequence.len());

    for (i, (action, (_, cmd))) in sequence.iter().zip(calls.iter()).enumerate() {
        match *action {
            "on" => assert!(
                cmd.brightness > 1,
                "step {} on should be >1, got {}",
                i,
                cmd.brightness
            ),
            "off" => assert_eq!(
                cmd.brightness, 1,
                "step {} off should be 1, got {}",
                i, cmd.brightness
            ),
            _ => unreachable!(),
        }
    }
}
