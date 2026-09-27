//! Reset must use the same node-local mode as its lighting profile.

mod harness;

use harness::{room, TestHarness};
use rhythm_core::{LightScheduleConfig, RhythmMode};
use rhythm_os::{commands, handlers};
use serde_json::json;

#[test]
fn reset_uses_named_schedule_defaults_through_the_node_action_api() {
    for target in ["active", "standby", "mood", "hard_off"] {
        let (harness, spy) = TestHarness::with_spy_controller_at(12.0, 172);
        let harness = harness.with_discovery(vec![room("studio", "Studio")], vec![]);
        harness.sync();
        let id = harness.resolve("studio");
        let opposite = if target == "hard_off" {
            "active"
        } else {
            "hard_off"
        };
        let response = handlers::handle_put_mode(
            &harness.state,
            &json!({
                "configs": [
                    {"mode": "day", "active_profile_id": "rhythm",
                     "room_defaults": [{"room_id": id, "state": opposite}]},
                    {"mode": "sleep", "active_profile_id": "sleep",
                     "room_defaults": [{"room_id": id, "state": target}]}
                ]
            }),
        );
        assert_eq!(response.status, 200);
        commands::do_light_schedules_set(
            &harness.state,
            vec![LightScheduleConfig {
                id: "late-shift".into(),
                name: "Late shift".into(),
                enabled: true,
                active_mode: RhythmMode::Sleep,
                transitions: vec![rhythm_core::ModeTransitionConfig::new(
                    RhythmMode::Sleep,
                    RhythmMode::Day,
                    0,
                )
                .with_id("wake")],
            }],
        )
        .unwrap();
        commands::do_light_schedule_assignment_set(&harness.state, &id, Some("late-shift"), false)
            .unwrap();

        // An explicit On remains On even if the schedule default is Off.
        harness.action("studio", "reset").unwrap();
        assert!(!harness.snapshot("studio").unwrap().hard_off);
        let settings = harness.snapshot("studio").unwrap().profile_settings;

        for _ in 0..2 {
            spy.reset();
            let result: serde_json::Value =
                serde_json::from_str(&harness.action("studio", "reset_to_mode_default").unwrap())
                    .unwrap();
            assert_eq!(result["state"], target, "{result}");
            let snapshot = harness.snapshot("studio").unwrap();
            assert_eq!(snapshot.hard_off, target == "hard_off", "{target}");
            assert_eq!(snapshot.soft_off, target == "standby", "{target}");
            assert_eq!(snapshot.mood_active, target == "mood", "{target}");
            assert!(snapshot.rhythm_enabled);
            assert_eq!(snapshot.time_offset_minutes, 0.0);
            assert_eq!(snapshot.brightness_offset, 0.0);
            assert_eq!(snapshot.profile_settings, settings);
            assert_eq!(harness.state.lock().unwrap().active_mode, RhythmMode::Day);
            if target == "hard_off" {
                assert!(spy.turn_off_calls().contains(&id));
                assert!(spy.turn_on_calls().is_empty());
            } else {
                assert!(spy.turn_off_calls().is_empty());
                assert!(spy.turn_on_calls().iter().any(|(node, _)| node == &id));
            }
        }
    }
}
