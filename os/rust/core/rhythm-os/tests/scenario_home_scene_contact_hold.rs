//! Automatic contacts yield to whole-home scene ownership.
mod harness;
use harness::{light, room, TestHarness};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::{commands, discovery::DiscoveredDevice, event_loop, hub::HubEvent};
use std::time::Duration;
#[test]
fn active_halloween_should_ignore_contact_open_and_close() {
    for child_target in [false, true] {
        for open in [false, true] {
            let (harness, spy) = TestHarness::with_spy_controller();
            let harness = harness.with_discovery(
                vec![room("party", "Party")],
                vec![
                    light("lamp", "party"),
                    DiscoveredDevice {
                        device_id: "door".into(),
                        room_id: Some("party".into()),
                        buttons: vec![],
                        device_type: DeviceType::Contact,
                    },
                ],
            );
            harness.sync();
            let child = harness
                .state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&harness.hub_key, "lamp")
                .unwrap()
                .id
                .clone();
            let sensor = harness
                .state
                .lock()
                .unwrap()
                .canonical_registry
                .find_by_native_id(&harness.hub_key, "door")
                .unwrap()
                .id
                .clone();
            if child_target {
                commands::do_topology_set_control_target(
                    &harness.state,
                    &sensor,
                    rhythm_os::topology::NodeControlKind::Switch,
                    Some(&child),
                )
                .unwrap();
                commands::do_set_node_color(
                    &harness.state,
                    &child,
                    commands::NodeColorUpdate {
                        rgb: rhythm_core::Rgb::new(0, 64, 255),
                        xy: None,
                        brightness: Some(60),
                        transition_ms: None,
                        scope: commands::NodeColorScope::Mood,
                    },
                    false,
                )
                .unwrap();
            }
            commands::do_home_scene_apply(
                &harness.state,
                "halloween",
                Default::default(),
                Duration::ZERO,
            )
            .unwrap();
            let node_id = if child_target {
                child
            } else {
                harness.resolve("party")
            };
            let runtime = harness.state.lock().unwrap().hub_runtime().unwrap();
            let before = runtime.engine_node_snapshot(&node_id).unwrap();
            assert!(before.mood_active);
            let before_on = spy.turn_on_count();
            let before_off = spy.turn_off_count();
            event_loop::handle_hub_event(
                &harness.state,
                HubEvent::Contact {
                    hub_key: Some(harness.hub_key.clone()),
                    room_id: "party".into(),
                    sensor_id: "door".into(),
                    open,
                },
                &mut event_loop::MotionTimerState::new(),
            );
            std::thread::sleep(Duration::from_millis(250));
            let after = runtime.engine_node_snapshot(&node_id).unwrap();
            assert_eq!(
                (spy.turn_on_count(), spy.turn_off_count()),
                (before_on, before_off),
                "child={child_target} open={open}"
            );
            assert!(after.mood_active);
            assert_eq!(after.hard_off, before.hard_off);
            // Explicit exit restores the sensor's normal On/Off behavior.
            commands::do_node_action(&harness.state, &harness.resolve("party"), "reset", false)
                .unwrap();
            let before_on = spy.turn_on_count();
            let before_off = spy.turn_off_count();
            event_loop::handle_hub_event(
                &harness.state,
                HubEvent::Contact {
                    hub_key: Some(harness.hub_key.clone()),
                    room_id: "party".into(),
                    sensor_id: "door".into(),
                    open,
                },
                &mut event_loop::MotionTimerState::new(),
            );
            let deadline = std::time::Instant::now() + Duration::from_secs(3);
            while spy.turn_on_count() == before_on && spy.turn_off_count() == before_off {
                assert!(
                    std::time::Instant::now() < deadline,
                    "contact did not resume after explicit exit"
                );
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }
}
