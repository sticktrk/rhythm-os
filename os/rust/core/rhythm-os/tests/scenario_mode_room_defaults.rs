//! Scenario: per-room mode defaults via `PUT /api/mode`.
//!
//! Exercises the public mode API path end-to-end:
//! 1. Hub discovery creates rooms and runtime state.
//! 2. User configures room defaults on the sleep mode and activates it.
//! 3. Rooms move to active / mood / off before output recalculation.
//! 4. The API response echoes the stored room defaults.

mod harness;

use harness::{room, TestHarness};
use rhythm_os::handlers;
use serde_json::json;

#[test]
fn put_mode_applies_room_defaults_on_mode_activation() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![
            room("kitchen", "Kitchen"),
            room("office", "Office"),
            room("balcony", "Balcony"),
            room("pantry", "Pantry"),
        ],
        vec![],
    );
    harness.sync();

    // Start with two active rooms and one explicitly hard-off room so the
    // mode defaults have distinct states to drive.
    harness.action("kitchen", "on").unwrap();
    harness.action("office", "on").unwrap();
    harness.action("balcony", "on").unwrap();
    harness.action("pantry", "on").unwrap();
    harness.action("balcony", "lights_off").unwrap();

    spy.reset();

    let response = handlers::handle_put_mode(
        &harness.state,
        &json!({
            "active": "sleep",
            "configs": [{
                "mode": "sleep",
                "active_profile_id": "sleep",
                "idle_profile_id": "sleep_idle",
                "room_defaults": [
                    { "room_id": harness.resolve("kitchen"), "state": "mood" },
                    { "room_id": harness.resolve("office"), "state": "hard_off" },
                    { "room_id": harness.resolve("balcony"), "state": "active" },
                    { "room_id": harness.resolve("pantry"), "state": "standby" }
                ]
            }]
        }),
    );

    assert_eq!(response.status, 200, "mode update should succeed");
    let parsed: serde_json::Value = serde_json::from_str(&response.body).unwrap();
    assert_eq!(parsed["active"], "sleep");

    let sleep_config = parsed["configs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|config| config["mode"] == "sleep")
        .expect("sleep config should be returned");
    assert_eq!(sleep_config["room_defaults"].as_array().unwrap().len(), 4);

    let kitchen = harness.snapshot("kitchen").unwrap();
    assert!(kitchen.mood_active, "mood target should enter mood state");
    assert!(!kitchen.soft_off, "mood target should not enter standby");
    assert!(!kitchen.hard_off, "mood target should not hard-off");

    let office = harness.snapshot("office").unwrap();
    assert!(office.hard_off, "office should move to hard-off");
    assert!(!office.soft_off, "hard-off room should not retain soft_off");

    let balcony = harness.snapshot("balcony").unwrap();
    assert!(!balcony.soft_off, "balcony should become active");
    assert!(!balcony.hard_off, "active default should clear hard-off");

    let pantry = harness.snapshot("pantry").unwrap();
    assert!(pantry.soft_off, "standby target should enter standby");
    assert!(!pantry.mood_active, "standby target should not enter mood");
    assert!(!pantry.hard_off, "standby target should not hard-off");

    assert!(
        harness.lights_on("kitchen"),
        "mood default should be tracked on"
    );
    assert!(
        !harness.lights_on("office"),
        "hard-off room should be tracked off"
    );
    assert!(
        harness.lights_on("balcony"),
        "reactivated room should be tracked on"
    );
    assert!(
        harness.lights_on("pantry"),
        "standby default should be tracked on"
    );

    let kitchen_id = harness.resolve("kitchen");
    let office_id = harness.resolve("office");
    let balcony_id = harness.resolve("balcony");
    let pantry_id = harness.resolve("pantry");

    let turn_on_calls = spy.turn_on_calls();
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, command)| room_id == &kitchen_id && command.brightness == 1),
        "mood default should render the mood profile for kitchen"
    );
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, _)| room_id == &balcony_id),
        "active default should re-render balcony"
    );
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, command)| room_id == &pantry_id && command.brightness == 1),
        "standby default should render the standby profile for pantry"
    );
    assert!(
        spy.turn_off_calls().contains(&office_id),
        "hard-off default should send turn_off for office"
    );
}
