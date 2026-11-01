//! Global mode boundaries use real instants through daylight saving changes.

mod harness;

use chrono::{Duration, NaiveDate, NaiveDateTime};
use harness::{rooms_with_lights, TestHarness};
use rhythm_core::{
    ModeChangeCause, ModeTransitionConfig, ModeTransitionTime, ModeTransitionTrigger, RhythmMode,
    SolarEvent, Timezone, RHYTHM_PROFILE_ID, SLEEP_PROFILE_ID,
};
use rhythm_os::{commands, periodic};

fn utc(month: u32, day: u32, hour: u32, minute: u32, second: u32) -> NaiveDateTime {
    NaiveDate::from_ymd_opt(2026, month, day)
        .unwrap()
        .and_hms_opt(hour, minute, second)
        .unwrap()
}

fn scheduled(hour: u8, minute: u8) -> ModeTransitionConfig {
    ModeTransitionConfig::new(RhythmMode::Sleep, RhythmMode::Day, 0)
        .with_id("scheduled_wake")
        .with_trigger(ModeTransitionTrigger::Scheduled(
            ModeTransitionTime::from_hour_minute(hour, minute).unwrap(),
        ))
}

fn configured_harness(
    timezone_name: Option<&str>,
    utc_offset_hours: f32,
    transition: ModeTransitionConfig,
) -> TestHarness {
    let (rooms, devices) = rooms_with_lights(&[("bedroom", "Bedroom")]);
    let h = TestHarness::new_at(7.0, 305).with_discovery(rooms, devices);
    h.sync();
    {
        let mut state = h.state.lock().unwrap();
        state.latitude = Some(40.7);
        state.longitude = Some(-74.0);
        state.timezone_name = timezone_name.map(str::to_owned);
        state.utc_offset_hours = utc_offset_hours;
    }
    commands::do_settings_set(
        &h.state,
        Some(false),
        None,
        None,
        Some(vec![transition]),
        None,
        None,
    )
    .unwrap();
    commands::do_set_active_mode(&h.state, RhythmMode::Sleep).unwrap();
    assert_mode(&h, RhythmMode::Sleep);
    h
}

fn assert_mode(h: &TestHarness, expected: RhythmMode) {
    let (mode, runtime) = {
        let state = h.state.lock().unwrap();
        (state.active_mode, state.hub_runtime().unwrap())
    };
    assert_eq!(mode, expected);
    assert_eq!(
        runtime.active_light_profile_id(),
        match expected {
            RhythmMode::Day => RHYTHM_PROFILE_ID,
            RhythmMode::Sleep => SLEEP_PROFILE_ID,
        },
        "the automatic mode change must reach the running engine"
    );
}

fn assert_scheduled_wake(h: &TestHarness) {
    assert_mode(h, RhythmMode::Day);
    let state = h.state.lock().unwrap();
    assert_eq!(state.last_active_mode_cause, ModeChangeCause::Schedule);
    assert_eq!(
        state.last_active_mode_transition_id.as_deref(),
        Some("scheduled_wake")
    );
}

#[test]
fn spring_forward_still_fires_a_boundary_immediately_before_the_skipped_hour() {
    let h = configured_harness(Some("America/New_York"), -4.0, scheduled(1, 59));

    // 01:58:30 EST -> 03:00:30 EDT includes the real 01:59 boundary.
    periodic::check_mode_transitions_between(&h.state, utc(3, 8, 6, 58, 30), utc(3, 8, 7, 0, 30));

    assert_scheduled_wake(&h);
}

#[test]
fn spring_forward_does_not_invent_a_nonexistent_wall_clock_boundary() {
    let h = configured_harness(Some("America/New_York"), -4.0, scheduled(2, 30));

    periodic::check_mode_transitions_between(&h.state, utc(3, 8, 6, 58, 30), utc(3, 8, 7, 0, 30));

    assert_mode(&h, RhythmMode::Sleep);
    assert_eq!(
        h.state.lock().unwrap().last_active_mode_cause,
        ModeChangeCause::Manual
    );
}

#[test]
fn fall_back_fires_only_the_first_occurrence_even_after_a_manual_mode_change() {
    let h = configured_harness(Some("America/New_York"), -5.0, scheduled(1, 30));

    periodic::check_mode_transitions_between(&h.state, utc(11, 1, 5, 29, 30), utc(11, 1, 5, 30, 0));
    assert_scheduled_wake(&h);

    commands::do_set_active_mode(&h.state, RhythmMode::Sleep).unwrap();
    periodic::check_mode_transitions_between(&h.state, utc(11, 1, 6, 29, 30), utc(11, 1, 6, 30, 0));

    assert_mode(&h, RhythmMode::Sleep);
    assert_eq!(
        h.state.lock().unwrap().last_active_mode_cause,
        ModeChangeCause::Manual
    );
}

#[test]
fn delayed_fall_back_check_does_not_replay_a_boundary_before_its_utc_window() {
    let h = configured_harness(Some("America/New_York"), -5.0, scheduled(0, 30));

    // 01:00 EDT -> 01:10 EST: 00:30 occurred before this window.
    periodic::check_mode_transitions_between(&h.state, utc(11, 1, 5, 0, 0), utc(11, 1, 6, 10, 0));

    assert_mode(&h, RhythmMode::Sleep);
}

#[test]
fn delayed_fall_back_check_keeps_the_real_boundary_inside_its_utc_window() {
    let h = configured_harness(Some("America/New_York"), -5.0, scheduled(1, 30));

    // 01:30 EDT occurs inside this window despite ending at 01:10 EST.
    periodic::check_mode_transitions_between(&h.state, utc(11, 1, 5, 0, 0), utc(11, 1, 6, 10, 0));

    assert_scheduled_wake(&h);
}

#[test]
fn fractional_fixed_offset_still_crosses_a_boundary_at_local_midnight() {
    let h = configured_harness(None, 5.5, scheduled(0, 0));

    // 23:58:30 -> 00:00:30 in the fixed +05:30 location.
    periodic::check_mode_transitions_between(
        &h.state,
        utc(11, 1, 18, 28, 30),
        utc(11, 1, 18, 30, 30),
    );

    assert_scheduled_wake(&h);
}

#[test]
fn disabled_and_wrong_source_mode_boundaries_preserve_the_selected_mode() {
    let mut wrong_source = scheduled(1, 30);
    wrong_source.from_mode = RhythmMode::Day;
    wrong_source.to_mode = RhythmMode::Sleep;

    for transition in [scheduled(1, 30).with_trigger_enabled(false), wrong_source] {
        let h = configured_harness(Some("America/New_York"), -5.0, transition);
        let original_change = {
            let state = h.state.lock().unwrap();
            (
                state.last_active_mode_cause,
                state.last_active_mode_transition_id.clone(),
                state.last_active_mode_change_utc_ms,
            )
        };

        periodic::check_mode_transitions_between(
            &h.state,
            utc(11, 1, 5, 29, 30),
            utc(11, 1, 5, 30, 30),
        );

        assert_mode(&h, RhythmMode::Sleep);
        let state = h.state.lock().unwrap();
        assert_eq!(
            (
                state.last_active_mode_cause,
                state.last_active_mode_transition_id.clone(),
                state.last_active_mode_change_utc_ms,
            ),
            original_change,
            "an ineligible boundary must not reapply the current mode"
        );
    }
}

#[test]
fn solar_boundary_retains_its_offset_from_the_real_sunrise() {
    let h = configured_harness(
        Some("America/New_York"),
        -5.0,
        ModeTransitionConfig::new(RhythmMode::Sleep, RhythmMode::Day, 0)
            .with_id("after_sunrise")
            .with_trigger(ModeTransitionTrigger::solar(SolarEvent::Sunrise, 30)),
    );
    let timezone = Timezone::new("America/New_York");
    let sunrise_hour = rhythm_core::calculate_sunrise(40.7, -74.0, 2026, 11, 1, &timezone);
    let sunrise_local =
        utc(11, 1, 0, 0, 0) + Duration::seconds((sunrise_hour * 3600.0).round() as i64);
    let sunrise_utc = timezone.utc_datetime_from_local(sunrise_local).unwrap();

    periodic::check_mode_transitions_between(
        &h.state,
        sunrise_utc - Duration::seconds(30),
        sunrise_utc + Duration::seconds(30),
    );
    assert_mode(&h, RhythmMode::Sleep);

    let boundary_utc = sunrise_utc + Duration::minutes(30);
    periodic::check_mode_transitions_between(
        &h.state,
        boundary_utc - Duration::seconds(30),
        boundary_utc + Duration::seconds(30),
    );

    assert_mode(&h, RhythmMode::Day);
    let state = h.state.lock().unwrap();
    assert_eq!(state.last_active_mode_cause, ModeChangeCause::Schedule);
    assert_eq!(
        state.last_active_mode_transition_id.as_deref(),
        Some("after_sunrise")
    );
}
