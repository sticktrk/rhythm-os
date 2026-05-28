//! Rapid button mashing must not produce inconsistent dispatch state.
//!
//! The morning bundle showed three OffPress events in 17 seconds. Pinning that
//! every dispatch in a fast burst stays hard-off catches any state machine that
//! interleaves Active/Off incorrectly on rapid input.

mod harness;

use harness::*;

#[test]
fn five_off_presses_in_a_row_all_dispatch_hard_off() {
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

    let off_calls = spy.turn_off_calls();
    assert_eq!(
        spy.turn_on_calls().len(),
        0,
        "no legacy soft-off across mashes"
    );
    assert!(
        off_calls.len() >= 5,
        "every press should dispatch (got {})",
        off_calls.len()
    );

    let snap = h.snapshot("kitchen").unwrap();
    assert!(!snap.soft_off, "final state must not be legacy soft-off");
    assert!(snap.hard_off, "final state must be hard-off");
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

    let calls = spy.calls();
    assert!(calls.len() >= sequence.len());

    for (i, (action, call)) in sequence.iter().zip(calls.iter()).enumerate() {
        match *action {
            "on" => match call {
                rhythm_core::spy_controller::SpyCall::TurnOn { command, .. } => assert!(
                    command.brightness > 1,
                    "step {} on should be >1, got {}",
                    i,
                    command.brightness
                ),
                other => panic!("step {} expected turn_on, got {:?}", i, other),
            },
            "off" => assert!(
                matches!(call, rhythm_core::spy_controller::SpyCall::TurnOff { .. }),
                "step {} expected hard-off, got {:?}",
                i,
                call
            ),
            _ => unreachable!(),
        }
    }
}
