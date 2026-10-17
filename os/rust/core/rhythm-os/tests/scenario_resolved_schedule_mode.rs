//! Appliance schedule readback must agree across node responses and SSE without
//! turning a transient resolved mode into a persisted setting.
mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::{ModeTransitionTime, RhythmMode};
use rhythm_os::{commands, handlers};
use serde_json::{json, Value};

#[test]
fn legacy_schedule_mode_reaches_every_readback_and_inherited_child() {
    let (h, spy) = TestHarness::with_spy_controller_at(12.0, 172);
    let (rooms, devices) = rooms_with_lights(&[("studio", "Studio")]);
    let h = h.with_discovery(rooms, devices);
    h.sync();
    let id = h.resolve("studio");

    // The response builders use the appliance wall clock. Place the synthetic
    // boundaries six hours away so the scenario is independent of run time.
    let hello: Value =
        serde_json::from_str(&commands::build_state_snapshot(&h.state).unwrap()).unwrap();
    let hour = hello["location"]["current_local_hour"].as_f64().unwrap() as u8;
    let time = |offset: u8| format!("{:02}:00", (hour + offset) % 24);

    for (wake, sleep, expected) in [
        (time(6), time(18), RhythmMode::Sleep),
        (time(18), time(6), RhythmMode::Day),
        (time(6), time(18), RhythmMode::Sleep),
    ] {
        let result = handlers::handle_put_node_preferences(
            &h.state,
            &json!({
                "node_id": id,
                "profile_settings": {
                    "room_schedule": {"source": "follow_time", "wake_time": wake, "sleep_time": sleep},
                    // Echoed readback must never become a persisted preference.
                    "resolved_mode": "day",
                },
            }),
            true,
        );
        assert_eq!(result.status, 200, "{}", result.body);
        let response: Value = serde_json::from_str(&result.body).unwrap();
        let expected = serde_json::to_value(expected).unwrap();
        assert_eq!(
            response["nodes"][0]["profile_settings"]["resolved_mode"],
            expected
        );
        let saved = h.snapshot("studio").unwrap().profile_settings;
        assert_eq!(
            saved.room_schedule.unwrap().wake_time,
            ModeTransitionTime::parse(&wake).unwrap()
        );
        assert!(serde_json::to_value(&saved)
            .unwrap()
            .get("resolved_mode")
            .is_none());

        spy.reset();
        let full: Value =
            serde_json::from_str(&commands::build_state_snapshot(&h.state).unwrap()).unwrap();
        let poll: Value =
            serde_json::from_str(&commands::build_nodes_state(&h.state).unwrap()).unwrap();
        for body in [&full, &poll] {
            let nodes = body["nodes"].as_array().unwrap();
            assert_eq!(nodes.len(), 2, "room and attached light");
            for node in nodes {
                assert_eq!(node["profile_settings"]["resolved_mode"], expected);
                assert!(node["room_profile"].get("resolved_mode").is_none());
            }
        }
        let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
        for snap in runtime.engine_all_room_snapshots() {
            let snap = runtime.engine_effective_node_snapshot(&snap.id).unwrap();
            let event =
                serde_json::to_value(commands::build_node_state_event(&h.state, &snap)).unwrap();
            assert_eq!(event["profile_settings"]["resolved_mode"], expected);
        }
        let room = serde_json::to_value(commands::build_room_rhythm_state(&h.state, &id).unwrap())
            .unwrap();
        assert_eq!(room["profile_settings"]["resolved_mode"], expected);
        assert_eq!(h.snapshot("studio").unwrap().profile_settings, saved);
        assert_eq!(h.state.lock().unwrap().active_mode, RhythmMode::Day);
        assert_eq!(spy.turn_on_count(), 0, "readback must never dispatch");
        assert_eq!(spy.turn_off_count(), 0);
        let backup = commands::build_backup_bundle(&h.state, false).unwrap();
        assert!(!backup.contains("resolved_mode"));
        h.sync();
        assert_eq!(h.snapshot("studio").unwrap().profile_settings, saved);
    }
}
