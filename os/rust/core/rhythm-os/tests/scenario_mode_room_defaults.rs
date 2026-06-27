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

#[test]
fn mode_activation_reasserts_existing_hard_off_default() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("drop_zone", "Drop Zone")], vec![]);
    harness.sync();
    harness.set_settings(Some(false));

    harness.action("drop_zone", "on").unwrap();
    harness.action("drop_zone", "lights_off").unwrap();
    let drop_zone_id = harness.resolve("drop_zone");
    assert!(
        harness.snapshot("drop_zone").unwrap().hard_off,
        "setup: room should already be logically hard-off"
    );

    harness.set_lights_on("drop_zone", true);
    assert!(
        harness.lights_on("drop_zone"),
        "setup: observed power should simulate external Hue drift back on"
    );
    spy.reset();

    let response = handlers::handle_put_mode(
        &harness.state,
        &json!({
            "active": "sleep",
            "configs": [
                {
                    "mode": "day",
                    "active_profile_id": "rhythm",
                    "room_defaults": [
                        { "room_id": drop_zone_id.clone(), "state": "hard_off" }
                    ]
                },
                {
                    "mode": "sleep",
                    "active_profile_id": "sleep",
                    "room_defaults": [
                        { "room_id": drop_zone_id.clone(), "state": "hard_off" }
                    ]
                }
            ]
        }),
    );

    assert_eq!(response.status, 200, "mode activation should succeed");
    assert!(
        spy.turn_off_calls().contains(&drop_zone_id),
        "activating a mode must physically reassert an explicit hard-off default even when the room was already logically hard-off"
    );
    assert!(
        !harness.lights_on("drop_zone"),
        "hard-off reassertion should update observed power back to off"
    );
}

#[test]
fn same_mode_transition_resets_rooms_to_mode_defaults() {
    let (harness, spy) = TestHarness::with_spy_controller_at(10.0, 125);
    let harness = harness.with_discovery(
        vec![
            room("guest_bath", "Guest Bath"),
            room("mud_room", "Mud Room"),
            room("office", "Office"),
        ],
        vec![],
    );
    harness.sync();

    let guest_bath_id = harness.resolve("guest_bath");
    let mud_room_id = harness.resolve("mud_room");
    let office_id = harness.resolve("office");

    let mode_response = handlers::handle_put_mode(
        &harness.state,
        &json!({
            "configs": [{
                "mode": "day",
                "active_profile_id": "rhythm",
                "room_defaults": [
                    { "room_id": office_id, "state": "standby" }
                ]
            }]
        }),
    );
    assert_eq!(
        mode_response.status, 200,
        "mode config update should succeed"
    );

    let transitions_response = handlers::handle_put_transitions(
        &harness.state,
        &json!({
            "transitions": [{
                "from_mode": "sleep",
                "to_mode": "day",
                "trigger": { "kind": "solar", "event": "sunrise" },
                "duration_ms": { "mode": "fixed", "value": 1000 }
            }]
        }),
    );
    assert_eq!(
        transitions_response.status, 200,
        "transition config update should succeed"
    );
    let transitions: serde_json::Value = serde_json::from_str(&transitions_response.body).unwrap();
    let transition_id = transitions["transitions"][0]["id"]
        .as_str()
        .expect("transition id")
        .to_string();

    // Drift the active Day mode away from its defaults: two rooms are not in
    // Day's explicit room_defaults list, so they should fall back to Active.
    harness.action("guest_bath", "on").unwrap();
    harness.set_room_preferences("guest_bath", Some(true), None, Some(true));
    harness.set_room_offset("guest_bath", 45.0);

    harness.action("mud_room", "on").unwrap();
    harness.action("mud_room", "lights_off").unwrap();
    harness.set_room_offset("mud_room", -30.0);

    harness.action("office", "on").unwrap();
    spy.reset();

    let trigger_response = handlers::handle_post_transition_trigger(&harness.state, &transition_id);
    assert_eq!(
        trigger_response.status, 200,
        "same-mode trigger should succeed"
    );
    let parsed: serde_json::Value = serde_json::from_str(&trigger_response.body).unwrap();
    assert_eq!(parsed["active"], "day");
    assert_eq!(parsed["last_change"]["transition_id"], transition_id);

    let guest_bath = harness.snapshot("guest_bath").unwrap();
    assert!(
        !guest_bath.soft_off && !guest_bath.hard_off,
        "unlisted room should reset to the implicit Active default"
    );
    assert_eq!(guest_bath.time_offset_minutes, 0.0);

    let mud_room = harness.snapshot("mud_room").unwrap();
    assert!(
        !mud_room.soft_off && !mud_room.hard_off,
        "unlisted hard-off room should reset to the implicit Active default"
    );
    assert_eq!(mud_room.time_offset_minutes, 0.0);

    let office = harness.snapshot("office").unwrap();
    assert!(
        office.soft_off && !office.hard_off,
        "explicit Day room_default should still win over implicit Active"
    );

    assert!(harness.lights_on("guest_bath"));
    assert!(harness.lights_on("mud_room"));
    assert!(harness.lights_on("office"));

    let turn_on_calls = spy.turn_on_calls();
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, _)| room_id == &guest_bath_id),
        "same-mode reapply should dispatch the implicit Active room"
    );
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, _)| room_id == &mud_room_id),
        "same-mode reapply should turn a hard-off implicit Active room back on"
    );
    assert!(
        turn_on_calls
            .iter()
            .any(|(room_id, command)| { room_id == &office_id && command.brightness == 1 }),
        "explicit Standby default should render the standby profile"
    );
}
