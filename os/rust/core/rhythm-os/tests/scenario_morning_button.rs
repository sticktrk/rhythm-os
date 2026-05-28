//! Morning-bug reproduction file.
//!
//! Real bug report: pressing the bottom button on a Hue dimmer in the morning
//! produced lights that were "not dim". The default scenario harness fixes time
//! at 14:00 on June 21, so existing coverage misses every other hour of the
//! day. These tests now pin OffPress → hard-off across a representative slice
//! of times.
//!
//! If any of these fail, the engine has a time-dependent regression. If they
//! all pass, the bug lives in the platform layer (button-press classification
//! in `hue_buttons.rs`, or the Hue transport).

mod harness;

use harness::*;

fn assert_off_hard_offs(hour: f32, day_of_year: u32, label: &str) {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (h, spy) = TestHarness::with_spy_controller_at(hour, day_of_year);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    h.set_settings(Some(false));

    h.action("kitchen", "on").unwrap();
    spy.reset();

    h.action("kitchen", "off").unwrap();

    assert_eq!(
        spy.turn_off_calls().len(),
        1,
        "{}: off must hard-off",
        label
    );
    assert_eq!(
        spy.turn_on_calls().len(),
        0,
        "{}: off must not dispatch legacy soft-off turn_on",
        label
    );
}

#[test]
fn bottom_button_at_6am_hard_offs() {
    assert_off_hard_offs(6.0, 172, "6am summer");
}

#[test]
fn bottom_button_at_2am_hard_offs() {
    assert_off_hard_offs(2.0, 172, "2am summer");
}

#[test]
fn bottom_button_at_noon_hard_offs() {
    assert_off_hard_offs(12.0, 172, "noon summer");
}

#[test]
fn bottom_button_at_9pm_hard_offs() {
    assert_off_hard_offs(21.0, 172, "9pm summer");
}

#[test]
fn bottom_button_winter_morning_hard_offs() {
    // day 15 = mid-January (low-sun curve regime)
    assert_off_hard_offs(6.0, 15, "6am winter");
}
