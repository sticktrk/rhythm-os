//! Usage-driven Day/Sleep control sequences.
//!
//! A non-issue local usage capture showed that the physical Day/Sleep binding
//! sits alongside ordinary on/off and brightness traffic. These scenarios pin
//! the public automation API across those interactions without importing any
//! home, device, or bug-report data.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::RhythmMode;
use rhythm_os::commands;
use rhythm_os::topology::{AutomationAction, ModeTransitionSelection};

fn day_sleep_cycle() -> AutomationAction {
    AutomationAction::ModeCycle {
        modes: vec![RhythmMode::Day, RhythmMode::Sleep],
        transition: ModeTransitionSelection::None,
    }
}

#[test]
fn day_sleep_cycle_preserves_manual_hard_off_until_the_next_on_press() {
    let (rooms, devices) = rooms_with_lights(&[("bedroom", "Bedroom")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    harness.action("bedroom", "on").unwrap();
    let sleep = commands::do_execute_automation_action(&harness.state, &day_sleep_cycle()).unwrap();
    assert_eq!(sleep.target_mode, RhythmMode::Sleep);
    assert_eq!(sleep.transition_id, None);
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Sleep);

    harness.action("bedroom", "off").unwrap();
    assert!(harness.snapshot("bedroom").unwrap().hard_off);
    spy.reset();

    let day = commands::do_execute_automation_action(&harness.state, &day_sleep_cycle()).unwrap();
    assert_eq!(day.target_mode, RhythmMode::Day);
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Day);
    assert!(
        harness.snapshot("bedroom").unwrap().hard_off,
        "a global mode cycle must not undo a user's explicit hard-off"
    );
    assert_eq!(spy.turn_on_count(), 0);

    harness.action("bedroom", "on").unwrap();
    assert!(!harness.snapshot("bedroom").unwrap().hard_off);
    assert!(spy.turn_on_count() >= 1);
}

#[test]
fn legacy_mode_toggle_alias_round_trips_day_and_sleep() {
    let (rooms, devices) = rooms_with_lights(&[("hall", "Hall")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();
    let toggle = AutomationAction::ModeToggle {
        first_mode: RhythmMode::Day,
        second_mode: RhythmMode::Sleep,
        transition: ModeTransitionSelection::None,
    };

    let first = commands::do_execute_automation_action(&harness.state, &toggle).unwrap();
    assert_eq!(first.target_mode, RhythmMode::Sleep);
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Sleep);

    let second = commands::do_execute_automation_action(&harness.state, &toggle).unwrap();
    assert_eq!(second.target_mode, RhythmMode::Day);
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Day);
}

#[test]
fn invalid_exact_transition_leaves_mode_and_room_output_unchanged() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    harness.action("office", "on").unwrap();
    spy.reset();

    let error = commands::do_execute_automation_action(
        &harness.state,
        &AutomationAction::ModeSet {
            mode: RhythmMode::Sleep,
            transition: ModeTransitionSelection::Exact {
                id: "missing-transition".into(),
            },
        },
    )
    .unwrap_err();

    assert!(error.to_string().contains("Unknown transition"));
    assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Day);
    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
    assert!(!harness.snapshot("office").unwrap().hard_off);
}
