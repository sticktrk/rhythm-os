//! Matrix coverage for OffPress behavior.
//!
//! The default user-facing contract is simple: pressing the bottom button fully
//! turns lights off. Disabling the legacy `power_save` setting no longer
//! changes that behavior. This file iterates the matrix so any future bug
//! ("but did you try at hour X with state Y?") is caught without asking that
//! question manually.
//!
//! Replaces what would otherwise be a `proptest`-style randomized check; we
//! avoid the dep by enumerating a representative grid that exercises curve
//! regimes (winter/summer, early-morning/midday/evening), and the room states
//! the engine can be in entering the press.

mod harness;

use harness::*;

#[derive(Clone, Copy, Debug)]
enum PriorState {
    Fresh,
    Active,
    LegacySoftOff,
    HardOff,
}

fn prepare(h: &TestHarness, prior: PriorState) {
    match prior {
        PriorState::Fresh => {}
        PriorState::Active => {
            h.action("kitchen", "on").unwrap();
        }
        PriorState::LegacySoftOff => {
            h.action("kitchen", "on").unwrap();
            h.action("kitchen", "off").unwrap();
        }
        PriorState::HardOff => {
            h.action("kitchen", "on").unwrap();
            h.action("kitchen", "lights_off").unwrap();
        }
    }
}

#[test]
fn off_press_defaults_to_hard_off_across_matrix() {
    // (hour, day_of_year) — winter morning through summer evening.
    let times = [
        (2.0_f32, 15_u32), // 2am winter
        (6.0, 15),         // 6am winter
        (12.0, 15),        // noon winter
        (18.0, 15),        // 6pm winter
        (6.0, 80),         // 6am spring equinox
        (6.0, 172),        // 6am summer solstice
        (12.0, 172),       // noon summer
        (21.0, 172),       // 9pm summer
        (6.0, 264),        // 6am fall equinox
        (6.0, 356),        // 6am winter solstice eve
    ];
    let states = [PriorState::Fresh, PriorState::Active, PriorState::HardOff];

    let mut failures: Vec<String> = Vec::new();

    for (hour, day) in times {
        for prior in states {
            let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
            let (h, spy) = TestHarness::with_spy_controller_at(hour, day);
            let h = h.with_discovery(rooms, devices);
            h.sync();

            prepare(&h, prior);
            spy.reset();

            h.action("kitchen", "off").unwrap();

            let calls = spy.turn_on_calls();
            let off_calls = spy.turn_off_calls().len();
            let observed = calls.last().map(|(_, c)| c.brightness);
            let snap = h.snapshot("kitchen").unwrap();

            if off_calls != 1 || observed.is_some() || !snap.hard_off || snap.soft_off {
                failures.push(format!(
                    "hour={} day={} prior={:?}: turn_off_calls={} last_bri={:?} soft_off={} hard_off={}",
                    hour, day, prior, off_calls, observed, snap.soft_off, snap.hard_off
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "default off_press matrix failures ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}

#[test]
fn off_press_stays_hard_off_when_power_save_disabled_across_matrix() {
    // (hour, day_of_year) — winter morning through summer evening.
    let times = [
        (2.0_f32, 15_u32), // 2am winter
        (6.0, 15),         // 6am winter
        (12.0, 15),        // noon winter
        (18.0, 15),        // 6pm winter
        (6.0, 80),         // 6am spring equinox
        (6.0, 172),        // 6am summer solstice
        (12.0, 172),       // noon summer
        (21.0, 172),       // 9pm summer
        (6.0, 264),        // 6am fall equinox
        (6.0, 356),        // 6am winter solstice eve
    ];
    let states = [
        PriorState::Fresh,
        PriorState::Active,
        PriorState::LegacySoftOff,
        PriorState::HardOff,
    ];

    let mut failures: Vec<String> = Vec::new();

    for (hour, day) in times {
        for prior in states {
            let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
            let (h, spy) = TestHarness::with_spy_controller_at(hour, day);
            let h = h.with_discovery(rooms, devices);
            h.sync();
            h.set_settings(Some(false));

            prepare(&h, prior);
            spy.reset();

            h.action("kitchen", "off").unwrap();

            let calls = spy.turn_on_calls();
            let off_calls = spy.turn_off_calls().len();
            let observed = calls.last().map(|(_, c)| c.brightness);
            let snap = h.snapshot("kitchen").unwrap();

            if off_calls != 1 || observed.is_some() || !snap.hard_off || snap.soft_off {
                failures.push(format!(
                    "hour={} day={} prior={:?}: turn_off_calls={} last_bri={:?} soft_off={} hard_off={}",
                    hour, day, prior, off_calls, observed, snap.soft_off, snap.hard_off
                ));
            }
        }
    }

    assert!(
        failures.is_empty(),
        "power_save-disabled off_press matrix failures ({}):\n  {}",
        failures.len(),
        failures.join("\n  ")
    );
}
