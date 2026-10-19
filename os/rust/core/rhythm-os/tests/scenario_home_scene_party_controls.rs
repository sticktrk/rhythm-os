//! Brightness controls must keep an active party scene's color.
mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::commands;
use std::time::Duration;

#[test]
fn child_brightness_keeps_the_parent_halloween_color() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("lamp-1", "kitchen")],
    );
    source.sync();
    let child = source
        .state
        .lock()
        .unwrap()
        .topology
        .device_nodes()
        .next()
        .unwrap()
        .id
        .clone();
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls().last().unwrap().1.clone();
    commands::do_set_node_brightness(&source.state, &child, 20, false).unwrap();
    let adjusted = spy.turn_on_calls().last().unwrap().1.clone();
    assert_eq!(
        adjusted.rgb, initial.rgb,
        "child brightness must keep its Halloween color"
    );
    assert_eq!(adjusted.brightness, 20);
}

#[test]
fn room_dim_button_keeps_the_halloween_color() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("lamp-1", "kitchen")],
    );
    source.sync();
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls().last().unwrap().1.clone();
    commands::do_node_action(&source.state, &source.resolve("kitchen"), "dim_down", false).unwrap();
    let adjusted = spy.turn_on_calls().last().unwrap().1.clone();
    assert_eq!(
        adjusted.rgb, initial.rgb,
        "dimming must keep the Halloween color"
    );
}

fn output_for<'a>(
    calls: &'a [(String, rhythm_core::LightingCommand)],
    node: &str,
) -> Option<&'a rhythm_core::LightingCommand> {
    calls
        .iter()
        .find(|(id, _)| id == node || id == &rhythm_os::topology::device_light_node_id(node))
        .map(|(_, value)| value)
}

fn fixture() -> (
    TestHarness,
    std::sync::Arc<rhythm_core::spy_controller::SpyLightController>,
    String,
    String,
) {
    fixture_with_storage(None)
}

fn fixture_with_storage(
    storage: Option<std::sync::Arc<dyn rhythm_os::storage::Storage>>,
) -> (
    TestHarness,
    std::sync::Arc<rhythm_core::spy_controller::SpyLightController>,
    String,
    String,
) {
    let (h, spy) = TestHarness::with_spy_controller();
    h.state.lock().unwrap().storage = storage;
    let h = h.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            rhythm_os::discovery::DiscoveredDevice {
                device_id: "button-1".into(),
                room_id: Some("kitchen".into()),
                buttons: vec![],
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Button,
            },
        ],
    );
    h.sync();
    let (child, sibling) = {
        let s = h.state.lock().unwrap();
        (
            s.canonical_registry
                .find_by_native_id(&h.hub_key, "lamp-1")
                .unwrap()
                .id
                .clone(),
            s.canonical_registry
                .find_by_native_id(&h.hub_key, "lamp-2")
                .unwrap()
                .id
                .clone(),
        )
    };
    (h, spy, child, sibling)
}

#[test]
fn child_edits_use_parent_assignment_preserve_sibling_and_follow_new_theme() {
    for mode in [
        rhythm_os::scenes::HomeSceneTargetMode::Devices,
        rhythm_os::scenes::HomeSceneTargetMode::Rooms,
    ] {
        let (h, spy, child, sibling) = fixture();
        commands::do_set_node_color(
            &h.state,
            &child,
            commands::NodeColorUpdate {
                rgb: rhythm_core::Rgb::new(0, 0, 255),
                xy: None,
                brightness: Some(70),
                transition_ms: None,
                scope: commands::NodeColorScope::Mood,
            },
            false,
        )
        .unwrap();
        let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
        let child_binding = runtime
            .engine_node_snapshot(&child)
            .unwrap()
            .profile_settings;
        for theme in ["halloween", "party-green"] {
            if theme == "party-green" {
                let mut scene = h.state.lock().unwrap().scenes["halloween"].clone();
                scene.id = theme.into();
                for output in &mut scene.light.as_mut().unwrap().palette {
                    output.color = Some(rhythm_os::scenes::LightSceneColor::Rgb {
                        rgb: rhythm_core::Rgb::new(0, 255, 0),
                    });
                }
                commands::do_scene_upsert(&h.state, scene).unwrap();
            }
            spy.reset();
            commands::do_home_scene_apply(
                &h.state,
                theme,
                rhythm_os::scenes::HomeSceneApplyRequest {
                    target_mode: mode,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
            let initial = spy.turn_on_calls();
            let prior_child = output_for(&initial, &child)
                .or_else(|| (initial.len() == 1).then(|| &initial[0].1))
                .unwrap()
                .clone();
            let prior_sibling = output_for(&initial, &sibling)
                .or_else(|| (initial.len() == 1).then(|| &initial[0].1))
                .unwrap()
                .clone();
            for brightness in [20, 35] {
                spy.reset();
                commands::do_set_node_curve_modifier(
                    &h.state,
                    &child,
                    commands::NodeCurveModifier::Brightness(brightness),
                    false,
                )
                .unwrap();
                let changed = spy.turn_on_calls();
                let output = output_for(&changed, &child).unwrap();
                let mut expected = prior_child.clone();
                expected.brightness = brightness;
                assert_eq!(
                    output, &expected,
                    "child output must come from {theme} parent binding"
                );
                assert_eq!(
                    output_for(&changed, &sibling).unwrap(),
                    &prior_sibling,
                    "sibling output must not shift when child becomes explicit"
                );
                assert_eq!(
                    runtime
                        .engine_node_snapshot(&child)
                        .unwrap()
                        .profile_settings,
                    child_binding,
                    "child must not gain a stale brightness-scene owner"
                );
            }
        }
    }
}

#[test]
fn all_button_and_api_dim_routes_preserve_scene_colors_and_durable_definitions() {
    use rhythm_core::ButtonAction;
    use rhythm_os::storage::Storage;
    use rhythm_os::{event_loop, storage};
    for route in ["direct", "queued", "batch", "physical", "physical_full"] {
        for (name, action, delta) in [
            ("dim_down", ButtonAction::DownPress, -20_i16),
            ("dim_up", ButtonAction::UpPress, 20),
            ("step_down", ButtonAction::DownHold, -20),
            ("step_up", ButtonAction::UpHold, 20),
        ] {
            let directory = tempfile::tempdir().unwrap();
            let disk = std::sync::Arc::new(
                storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap(),
            );
            let (h, spy, _, _) = fixture_with_storage(Some(disk.clone()));
            let parent = h.resolve("kitchen");
            commands::do_home_scene_apply(
                &h.state,
                "halloween",
                Default::default(),
                Duration::ZERO,
            )
            .unwrap();
            let initial = spy.turn_on_calls();
            spy.reset();
            let (tx, rx) =
                std::sync::mpsc::sync_channel(if route == "physical_full" { 1 } else { 32 });
            if route == "physical_full" {
                tx.send(rhythm_os::state::WorkItem::DeferredPersist {
                    node_id: "queue-full".into(),
                })
                .unwrap();
            }
            h.state.lock().unwrap().work_tx = Some(tx);
            match route {
                "direct" => {
                    commands::do_node_action(&h.state, &parent, name, true).unwrap();
                }
                "queued" => {
                    commands::queue_node_action(&h.state, &parent, name, true, Duration::ZERO)
                        .unwrap();
                    event_loop::process_work_item(
                        &h.state,
                        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                    );
                }
                "batch" => {
                    commands::queue_node_action_batch(
                        &h.state,
                        vec![(parent.clone(), name.into())],
                        true,
                        Duration::ZERO,
                    )
                    .unwrap();
                    for _ in 0..2 {
                        event_loop::process_work_item(
                            &h.state,
                            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                        );
                    }
                }
                "physical" | "physical_full" => {
                    let mut motion = event_loop::MotionTimerState::new();
                    event_loop::handle_hub_event(
                        &h.state,
                        rhythm_os::hub::HubEvent::Button {
                            hub_key: Some(h.hub_key.clone()),
                            room_id: "kitchen".into(),
                            action,
                            device_id: Some("button-1".into()),
                        },
                        &mut motion,
                    );
                    if route == "physical_full" {
                        let deadline = std::time::Instant::now() + Duration::from_secs(2);
                        loop {
                            let saved = disk.load_rooms().unwrap();
                            if saved
                                .get(&parent)
                                .and_then(|room| room.profile_settings.mood_scene_id.as_deref())
                                .is_some_and(|id| id != "halloween")
                            {
                                break;
                            }
                            assert!(
                                std::time::Instant::now() < deadline,
                                "full queue must not drop durable scene update"
                            );
                            std::thread::sleep(Duration::from_millis(5));
                        }
                    } else {
                        event_loop::process_work_item(
                            &h.state,
                            rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                        );
                    }
                }
                _ => unreachable!(),
            }
            let changed = spy.turn_on_calls();
            assert_eq!(changed.len(), 2, "{route}/{name}");
            for (id, output) in &changed {
                let mut expected = initial.iter().find(|(old, _)| old == id).unwrap().1.clone();
                expected.brightness = (i16::from(expected.brightness) + delta).clamp(1, 100) as u8;
                assert_eq!(
                    output, &expected,
                    "{route}/{name} must only change brightness"
                );
            }
            assert!(h.snapshot(&parent).unwrap().mood_active);
            let (restored, restored_spy) = TestHarness::with_spy_controller();
            restored.state.lock().unwrap().storage = Some(disk);
            storage::load_persisted_state(&mut restored.state.lock().unwrap());
            let restored = restored.with_discovery(
                vec![room("kitchen", "Kitchen")],
                vec![light("lamp-1", "kitchen"), light("lamp-2", "kitchen")],
            );
            restored.sync();
            restored_spy.reset();
            commands::do_node_preferences_set(
                &restored.state,
                &parent,
                None,
                None,
                None,
                Some(rhythm_core::RoomModeState::Mood),
                None,
                false,
            )
            .unwrap();
            assert_eq!(
                restored_spy.turn_on_calls(),
                changed,
                "{route}/{name} must replay the durable adjusted scene"
            );
        }
    }
}

#[test]
fn child_brightness_and_dim_buttons_preserve_explicit_off_outputs() {
    let (h, spy, child, _) = fixture();
    let parent = h.resolve("kitchen");
    let mut scene = h.state.lock().unwrap().scenes["halloween"].clone();
    scene.id = "dark-party".into();
    let layer = scene.light.as_mut().unwrap();
    let mut output = layer.palette[0].clone();
    output.power = rhythm_os::scenes::LightScenePower::Off;
    layer.entries.push(rhythm_os::scenes::LightSceneEntry {
        target: rhythm_os::scenes::LightSceneTargetRef::Node {
            node_id: child.clone(),
        },
        output,
    });
    commands::do_scene_upsert(&h.state, scene).unwrap();
    commands::do_home_scene_apply(&h.state, "dark-party", Default::default(), Duration::ZERO)
        .unwrap();
    spy.reset();
    commands::do_set_node_brightness(&h.state, &child, 20, false).unwrap();
    assert!(!spy.turn_on_calls().iter().any(|(id, _)| id == &child));
    assert_eq!(spy.turn_off_count(), 1);
    spy.reset();
    commands::do_node_action(&h.state, &parent, "dim_up", false).unwrap();
    assert!(!spy.turn_on_calls().iter().any(|(id, _)| id == &child));
    assert_eq!(spy.turn_off_count(), 1);
    commands::do_node_action(&h.state, &parent, "reset", false).unwrap();
    assert!(
        !h.snapshot(&parent).unwrap().mood_active,
        "explicit Reset still exits the scene"
    );
}

#[test]
fn queued_child_brightness_keeps_parent_and_siblings_canonical_after_restart() {
    use rhythm_os::{event_loop, storage};
    for mode in [
        rhythm_os::scenes::HomeSceneTargetMode::Devices,
        rhythm_os::scenes::HomeSceneTargetMode::Rooms,
    ] {
        for batch in [false, true] {
            let directory = tempfile::tempdir().unwrap();
            let disk = std::sync::Arc::new(
                storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap(),
            );
            let (h, spy, child, sibling) = fixture_with_storage(Some(disk.clone()));
            let parent = h.resolve("kitchen");
            commands::do_set_node_color(
                &h.state,
                &child,
                commands::NodeColorUpdate {
                    rgb: rhythm_core::Rgb::new(0, 0, 255),
                    xy: None,
                    brightness: Some(70),
                    transition_ms: None,
                    scope: commands::NodeColorScope::Mood,
                },
                true,
            )
            .unwrap();
            spy.reset();
            commands::do_home_scene_apply(
                &h.state,
                "halloween",
                rhythm_os::scenes::HomeSceneApplyRequest {
                    target_mode: mode,
                    ..Default::default()
                },
                Duration::ZERO,
            )
            .unwrap();
            let initial = spy.turn_on_calls();
            let prior_child = output_for(&initial, &child)
                .or_else(|| (initial.len() == 1).then(|| &initial[0].1))
                .unwrap()
                .clone();
            let prior_sibling = output_for(&initial, &sibling)
                .or_else(|| (initial.len() == 1).then(|| &initial[0].1))
                .unwrap()
                .clone();
            spy.reset();
            let (tx, rx) = std::sync::mpsc::sync_channel(8);
            h.state.lock().unwrap().work_tx = Some(tx);
            if batch {
                commands::queue_set_node_curve_modifier_batch(
                    &h.state,
                    vec![(child.clone(), commands::NodeCurveModifier::Brightness(23))],
                    true,
                    Duration::ZERO,
                )
                .unwrap();
            } else {
                commands::queue_set_node_brightness(&h.state, &child, 23, true, Duration::ZERO)
                    .unwrap();
            }
            for _ in 0..if batch { 2 } else { 1 } {
                event_loop::process_work_item(
                    &h.state,
                    rx.recv_timeout(Duration::from_secs(2)).unwrap(),
                );
            }
            let changed = spy.turn_on_calls();
            let mut expected = prior_child;
            expected.brightness = 23;
            assert_eq!(output_for(&changed, &child).unwrap(), &expected);
            assert_eq!(output_for(&changed, &sibling).unwrap(), &prior_sibling);
            let (restored, restored_spy) = TestHarness::with_spy_controller();
            restored.state.lock().unwrap().storage = Some(disk);
            storage::load_persisted_state(&mut restored.state.lock().unwrap());
            let restored = restored.with_discovery(
                vec![room("kitchen", "Kitchen")],
                vec![light("lamp-1", "kitchen"), light("lamp-2", "kitchen")],
            );
            restored.sync();
            restored_spy.reset();
            commands::do_node_preferences_set(
                &restored.state,
                &parent,
                None,
                None,
                None,
                Some(rhythm_core::RoomModeState::Mood),
                None,
                false,
            )
            .unwrap();
            assert_eq!(
                restored_spy.turn_on_calls(),
                changed,
                "queued child edit must replay the parent scene after restart"
            );
            assert!(restored.snapshot(&parent).unwrap().mood_active);
        }
    }
}

#[test]
fn child_display_follows_parent_scene_assignment_until_explicit_exit() {
    for mode in [
        rhythm_os::scenes::HomeSceneTargetMode::Devices,
        rhythm_os::scenes::HomeSceneTargetMode::Rooms,
    ] {
        let (h, spy, child, sibling) = fixture();
        let parent = h.resolve("kitchen");
        let blue = rhythm_core::Rgb::new(0, 0, 255);
        commands::do_set_node_color(
            &h.state,
            &child,
            commands::NodeColorUpdate {
                rgb: blue,
                xy: None,
                brightness: Some(70),
                transition_ms: None,
                scope: commands::NodeColorScope::Mood,
            },
            false,
        )
        .unwrap();
        spy.reset();
        commands::do_home_scene_apply(
            &h.state,
            "halloween",
            rhythm_os::scenes::HomeSceneApplyRequest {
                target_mode: mode,
                ..Default::default()
            },
            Duration::ZERO,
        )
        .unwrap();
        for adjusted in [false, true] {
            if adjusted {
                spy.reset();
                let response =
                    commands::do_set_node_brightness(&h.state, &child, 23, false).unwrap();
                let response: serde_json::Value = serde_json::from_str(&response).unwrap();
                assert_eq!(response["brightness"], 23);
            }
            let calls = spy.turn_on_calls();
            for id in [&child, &sibling] {
                let sent = output_for(&calls, id)
                    .or_else(|| (calls.len() == 1).then(|| &calls[0].1))
                    .unwrap();
                let displayed = commands::build_node_state(&h.state, id).unwrap();
                assert_eq!(
                    displayed.brightness, sent.brightness,
                    "reported brightness must match the active parent assignment"
                );
                assert_eq!(
                    displayed.color,
                    Some(sent.rgb),
                    "saved child custom color must not mask its parent scene"
                );
                let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
                let snapshot = runtime.engine_effective_node_snapshot(id).unwrap();
                let event = commands::build_node_state_event(&h.state, &snapshot);
                let event = serde_json::to_value(event).unwrap();
                assert_eq!(event["brightness"], sent.brightness);
                assert_eq!(event["color"], serde_json::to_value(sent.rgb).unwrap());
            }
        }
        commands::do_node_action(&h.state, &parent, "reset", false).unwrap();
        assert!(!h.snapshot(&parent).unwrap().mood_active);
        spy.reset();
        commands::do_node_preferences_set(
            &h.state,
            &child,
            None,
            None,
            None,
            Some(rhythm_core::RoomModeState::Mood),
            None,
            false,
        )
        .unwrap();
        let displayed = commands::build_node_state(&h.state, &child).unwrap();
        assert_eq!(
            displayed.color,
            Some(blue),
            "parent Reset releases the child's ordinary saved Mood"
        );
        assert_eq!(displayed.brightness, 70);
        assert_eq!(spy.turn_on_calls().last().unwrap().1.rgb, blue);
    }
}
