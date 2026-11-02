//! Party dimming preserves the scene through the HTTP queue, restart and restore.
mod harness;

use harness::{light, motion_sensor, room, TestHarness};
use rhythm_core::RoomModeState;
use rhythm_os::{commands, event_loop, handlers, scenes, storage};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn process_brightness_batch(
    source: &TestHarness,
    rx: &std::sync::mpsc::Receiver<rhythm_os::state::WorkItem>,
    count: usize,
    persist: bool,
) {
    // Queue acceptance starts a producer thread; an empty channel does not
    // establish batch completion. Await the admitted writes and final flush.
    for _ in 0..count {
        let item = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("brightness work was not enqueued");
        event_loop::process_work_item(&source.state, item);
    }
    if persist {
        let item = rx
            .recv_timeout(Duration::from_secs(3))
            .expect("lighting persistence was not enqueued");
        assert!(matches!(
            item,
            rhythm_os::state::WorkItem::DeferredPersistLightingSettings
        ));
        event_loop::process_work_item(&source.state, item);
    }
}

fn assert_home_scene_ignores_motion(
    source: &TestHarness,
    spy: &rhythm_core::spy_controller::SpyLightController,
    target_node_id: &str,
) {
    let target_node_id = target_node_id.to_string();
    let before_on = spy.turn_on_count();
    let before_off = spy.turn_off_count();
    let mut motion = event_loop::MotionTimerState::new();
    event_loop::handle_hub_event(
        &source.state,
        rhythm_os::hub::HubEvent::Motion {
            hub_key: Some(source.hub_key.clone()),
            room_id: "kitchen".into(),
            sensor_id: "sensor-1".into(),
            detected: true,
        },
        &mut motion,
    );
    assert!(
        motion.sensors.is_empty(),
        "home scenes must not admit motion"
    );
    assert!(motion.motion_owned.is_empty());
    // A timer armed before scene selection must not dim or turn off the scene.
    motion.sensors.insert(
        "stale-sensor".into(),
        event_loop::MotionSourceState {
            source_node_id: "stale-sensor".into(),
            target_node_id: target_node_id.clone(),
            stopped_at: Some(Instant::now() - Duration::from_secs(3600)),
            stopped_at_epoch_ms: None,
        },
    );
    motion.motion_owned.insert(target_node_id.clone());
    motion.warning_active.insert(target_node_id.clone());
    event_loop::check_motion_timers(&source.state, &mut motion);
    assert!(motion.sensors.is_empty());
    assert!(motion.motion_owned.is_empty());
    assert!(motion.warning_active.is_empty());
    source
        .state
        .lock()
        .unwrap()
        .pending_motion_seed
        .push(rhythm_os::state::MotionSeedEntry {
            source_node_id: "sensor-1".into(),
            target_node_id,
            is_active: true,
            stopped_at_epoch_ms: None,
            motion_owned: Some(true),
            warning_active: false,
        });
    event_loop::apply_pending_motion_seeds(&source.state, &mut motion, Instant::now());
    assert!(
        motion.sensors.is_empty(),
        "restart seeds must not admit motion"
    );
    assert_eq!(spy.turn_on_count(), before_on);
    assert_eq!(spy.turn_off_count(), before_off);
}

#[test]
fn home_scene_suppresses_direct_child_motion_even_with_a_custom_child_binding() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![
            light("lamp-1", "kitchen"),
            motion_sensor("sensor-1", "kitchen"),
        ],
    );
    source.sync();
    let kitchen = source.resolve("kitchen");
    let (child, sensor, runtime) = {
        let s = source.state.lock().unwrap();
        (
            s.canonical_registry
                .find_by_native_id(&source.hub_key, "lamp-1")
                .unwrap()
                .id
                .clone(),
            s.canonical_registry
                .find_by_native_id(&source.hub_key, "sensor-1")
                .unwrap()
                .id
                .clone(),
            s.hub_runtime().unwrap(),
        )
    };
    commands::do_topology_set_control_target(
        &source.state,
        &sensor,
        rhythm_os::topology::NodeControlKind::Motion,
        Some(&child),
    )
    .unwrap();
    commands::do_set_node_color(
        &source.state,
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
    let custom_binding = runtime
        .engine_node_snapshot(&child)
        .unwrap()
        .profile_settings;
    assert!(custom_binding
        .mood_scene_id
        .as_deref()
        .unwrap()
        .starts_with("node-mood-scene-"));
    assert_eq!(custom_binding.mood_scene_palette_span, None);
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    assert!(runtime
        .engine_node_snapshot(&kitchen)
        .unwrap()
        .profile_settings
        .mood_scene_palette_span
        .is_some());
    assert_eq!(
        runtime
            .engine_effective_node_snapshot(&child)
            .unwrap()
            .profile_settings
            .mood_scene_id,
        custom_binding.mood_scene_id
    );
    assert_home_scene_ignores_motion(&source, &spy, &child);
    commands::do_set_node_curve_modifier(
        &source.state,
        &kitchen,
        commands::NodeCurveModifier::Brightness(20),
        false,
    )
    .unwrap();
    assert_home_scene_ignores_motion(&source, &spy, &child);
    // A local Off flag also masks inherited Mood in the effective snapshot.
    commands::do_node_action(&source.state, &child, "off", false).unwrap();
    assert_home_scene_ignores_motion(&source, &spy, &child);
    // Resetting only the child must not unpause motion while its parent still
    // owns the whole-home scene. Resetting the parent releases that ownership.
    commands::do_node_action(&source.state, &child, "reset", false).unwrap();
    assert_home_scene_ignores_motion(&source, &spy, &child);
    commands::do_node_action(&source.state, &kitchen, "reset", false).unwrap();
    // Reenter the child's existing custom Mood after the parent releases home
    // ownership: that standalone custom binding must still permit motion.
    commands::do_node_preferences_set(
        &source.state,
        &child,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .unwrap();
    let child_snapshot = runtime.engine_node_snapshot(&child).unwrap();
    assert!(child_snapshot.mood_active);
    assert_eq!(
        child_snapshot.profile_settings.mood_scene_id,
        custom_binding.mood_scene_id
    );
    assert_eq!(
        child_snapshot.profile_settings.mood_scene_palette_span,
        None
    );
    assert!(runtime
        .engine_node_snapshot(&child)
        .unwrap()
        .profile_settings
        .motion_activation_enabled());
    let mut motion = event_loop::MotionTimerState::new();
    event_loop::handle_hub_event(
        &source.state,
        rhythm_os::hub::HubEvent::Motion {
            hub_key: Some(source.hub_key.clone()),
            room_id: "kitchen".into(),
            sensor_id: "sensor-1".into(),
            detected: true,
        },
        &mut motion,
    );
    assert!(
        motion
            .sensors
            .values()
            .any(|source| source.target_node_id == child),
        "direct-child motion must resume after the parent exits its scene"
    );
}

#[test]
fn scene_brightness_preserves_explicitly_dark_lights() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("lamp-1", "kitchen"), light("lamp-2", "kitchen")],
    );
    source.sync();
    let dark_light = source
        .state
        .lock()
        .unwrap()
        .topology
        .device_nodes()
        .next()
        .unwrap()
        .id
        .clone();
    let mut scene = source.state.lock().unwrap().scenes["halloween"].clone();
    scene.id = "party-with-dark-light".into();
    let layer = scene.light.as_mut().unwrap();
    let mut output = layer.palette[0].clone();
    output.power = scenes::LightScenePower::Off;
    layer.entries.push(scenes::LightSceneEntry {
        target: scenes::LightSceneTargetRef::Node {
            node_id: dark_light,
        },
        output,
    });
    commands::do_scene_upsert(&source.state, scene).unwrap();
    commands::do_home_scene_apply(
        &source.state,
        "party-with-dark-light",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls();
    assert_eq!(initial.len(), 1);
    assert_eq!(spy.turn_off_count(), 1);
    commands::do_set_node_curve_modifier(
        &source.state,
        &source.resolve("kitchen"),
        commands::NodeCurveModifier::Brightness(20),
        false,
    )
    .unwrap();
    let changed = spy.turn_on_calls();
    assert_eq!(changed.len(), 2, "dimming must not turn the dark light on");
    assert_eq!(changed[1].0, initial[0].0);
    assert_eq!(changed[1].1.rgb, initial[0].1.rgb);
    assert_eq!(changed[1].1.brightness, 20);
    assert_eq!(spy.turn_off_count(), 2);
}

#[test]
fn mood_brightness_uses_the_current_profile_after_selecting_a_different_color() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("lamp-1", "kitchen")],
    );
    source.sync();
    let kitchen = source.resolve("kitchen");
    for (profile_id, kelvin) in [
        (rhythm_core::DAY_IDLE_PROFILE_ID, 2700),
        (rhythm_core::SLEEP_IDLE_PROFILE_ID, 5000),
    ] {
        let mut profile = source.config_for(profile_id);
        profile.min_color_temp = kelvin;
        profile.max_color_temp = kelvin;
        profile.curve = rhythm_core::LightCurveShape::Constant {
            brightness: 1.0,
            color_temp: 1.0,
            direct_color: None,
        };
        source.set_config_for(profile_id, profile);
        commands::do_node_preferences_set(
            &source.state,
            &kitchen,
            None,
            None,
            None,
            Some(RoomModeState::Mood),
            Some(&commands::RoomProfileSettingsPatch {
                mood_profile_id: Some(Some(profile_id.to_string())),
                ..Default::default()
            }),
            false,
        )
        .unwrap();
        assert_eq!(spy.turn_on_calls().last().unwrap().1.kelvin, kelvin);
        commands::do_set_node_curve_modifier(
            &source.state,
            &kitchen,
            commands::NodeCurveModifier::Brightness(25),
            false,
        )
        .unwrap();
        let output = spy.turn_on_calls().last().unwrap().1.clone();
        assert_eq!(output.brightness, 25);
        assert_eq!(
            output.kelvin, kelvin,
            "brightness must retain the selected Mood color"
        );
    }
}

#[test]
fn queued_scene_brightness_survives_disk_restart_without_a_backup() {
    let directory = tempfile::tempdir().unwrap();
    let disk = Arc::new(storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap());
    let (source, spy) = TestHarness::with_spy_controller();
    source.state.lock().unwrap().storage = Some(disk.clone());
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            motion_sensor("sensor-1", "kitchen"),
        ],
    );
    source.sync();
    let kitchen = source.resolve("kitchen");
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        scenes::HomeSceneApplyRequest::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls();
    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    source.state.lock().unwrap().work_tx = Some(tx);
    for (brightness, persist) in [(35, true), (20, true), (60, false)] {
        let response = handlers::handle_set_node_curve(
            &source.state,
            &json!({
                "nodes": [{"node_id": kitchen, "brightness": brightness}],
                "dispatch_spacing_ms": 0
            }),
            persist,
        );
        assert_eq!(response.status, 200, "{}", response.body);
        process_brightness_batch(&source, &rx, 1, persist);
    }
    assert_eq!(spy.turn_on_calls().last().unwrap().1.brightness, 60);
    let binding = source.snapshot(&kitchen).unwrap().profile_settings;
    assert_home_scene_ignores_motion(&source, &spy, &kitchen);

    // Read the same files an ordinary process restart reads. Exporting a
    // backup here would hide definitions which were only updated in memory.
    let (restored, restored_spy) = TestHarness::with_spy_controller();
    restored.state.lock().unwrap().storage = Some(disk);
    storage::load_persisted_state(&mut restored.state.lock().unwrap());
    let scene_id = binding.mood_scene_id.as_ref().unwrap();
    assert!(
        restored.state.lock().unwrap().scenes.contains_key(scene_id),
        "persisted room binding must have a persisted scene definition"
    );
    let restored = restored.with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            motion_sensor("sensor-1", "kitchen"),
        ],
    );
    restored.sync();
    assert_eq!(
        restored.snapshot(&kitchen).unwrap().profile_settings,
        binding
    );
    restored_spy.reset();
    commands::do_node_preferences_set(
        &restored.state,
        &kitchen,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .unwrap();
    let resumed = restored_spy.turn_on_calls();
    assert_eq!(resumed.len(), 2);
    for (node, command) in resumed {
        assert_eq!(command.brightness, 20);
        assert_eq!(
            command.rgb,
            initial.iter().find(|(id, _)| id == &node).unwrap().1.rgb
        );
    }
    assert_home_scene_ignores_motion(&restored, &restored_spy, &kitchen);
    assert!(restored
        .snapshot(&kitchen)
        .unwrap()
        .profile_settings
        .motion_activation_enabled());
    commands::do_node_action(&restored.state, &kitchen, "reset", false).unwrap();
    let mut motion = event_loop::MotionTimerState::new();
    event_loop::handle_hub_event(
        &restored.state,
        rhythm_os::hub::HubEvent::Motion {
            hub_key: Some(restored.hub_key.clone()),
            room_id: "kitchen".into(),
            sensor_id: "sensor-1".into(),
            detected: true,
        },
        &mut motion,
    );
    assert!(
        !motion.sensors.is_empty(),
        "normal configured motion resumes after Reset"
    );
}

#[test]
fn queued_mood_profile_brightness_persists_through_both_batch_routes() {
    for curve_route in [true, false] {
        let directory = tempfile::tempdir().unwrap();
        let disk = Arc::new(storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap());
        let (source, _) = TestHarness::with_spy_controller();
        source.state.lock().unwrap().storage = Some(disk.clone());
        let source = source.with_discovery(
            vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
            vec![light("lamp-1", "kitchen"), light("lamp-2", "hall")],
        );
        source.sync();
        let kitchen = source.resolve("kitchen");
        let hall = source.resolve("hall");
        for node_id in [&kitchen, &hall] {
            commands::do_node_preferences_set(
                &source.state,
                node_id,
                None,
                None,
                None,
                Some(RoomModeState::Mood),
                None,
                true,
            )
            .unwrap();
            assert!(source.snapshot(node_id).unwrap().mood_active);
        }
        let (tx, rx) = std::sync::mpsc::sync_channel(32);
        source.state.lock().unwrap().work_tx = Some(tx);
        for brightness in [35, 20] {
            let body = json!({
                "nodes": [{"node_id": kitchen, "brightness": brightness}, {"node_id": hall, "brightness": brightness}],
                "dispatch_spacing_ms": 0
            });
            let response = if curve_route {
                handlers::handle_set_node_curve(&source.state, &body, true)
            } else {
                handlers::handle_set_node_brightness(&source.state, &body, true)
            };
            assert_eq!(response.status, 200, "{}", response.body);
            assert_eq!(
                serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["queued"],
                true
            );
            process_brightness_batch(&source, &rx, 2, true);
        }
        let bindings: Vec<_> = [&kitchen, &hall]
            .into_iter()
            .map(|node_id| (node_id, source.snapshot(node_id).unwrap().profile_settings))
            .collect();
        let (restored, restored_spy) = TestHarness::with_spy_controller();
        restored.state.lock().unwrap().storage = Some(disk);
        storage::load_persisted_state(&mut restored.state.lock().unwrap());
        for (_, binding) in &bindings {
            let profile_id = binding.mood_profile_id.as_ref().unwrap();
            assert_eq!(
                restored
                    .state
                    .lock()
                    .unwrap()
                    .light_profile_config(profile_id)
                    .unwrap()
                    .max_brightness,
                20
            );
        }
        let restored = restored.with_discovery(
            vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
            vec![light("lamp-1", "kitchen"), light("lamp-2", "hall")],
        );
        restored.sync();
        // The scenario harness creates a bare runtime; production lifecycle
        // installs all loaded profile definitions when creating its runtime.
        let (runtime, profiles) = {
            let state = restored.state.lock().unwrap();
            (
                state.hub_runtime().unwrap(),
                state.light_profile_configs.clone(),
            )
        };
        for profile in profiles.into_values() {
            runtime.set_light_profile_config(profile).unwrap();
        }
        for (node_id, binding) in bindings {
            assert_eq!(
                restored.snapshot(node_id).unwrap().profile_settings,
                binding
            );
            restored_spy.reset();
            commands::do_node_preferences_set(
                &restored.state,
                node_id,
                None,
                None,
                None,
                Some(RoomModeState::Mood),
                None,
                false,
            )
            .unwrap();
            assert_eq!(
                restored_spy.turn_on_calls().last().unwrap().1.brightness,
                20
            );
        }
    }
}

#[test]
fn scene_brightness_batch_preserves_colors_and_survives_backup_restart_and_sync() {
    let (source, spy) = TestHarness::with_spy_controller();
    let source = source.with_discovery(
        vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            light("lamp-3", "hall"),
        ],
    );
    source.sync();
    let kitchen = source.resolve("kitchen");
    let hall = source.resolve("hall");
    commands::do_home_scene_apply(
        &source.state,
        "halloween",
        scenes::HomeSceneApplyRequest::default(),
        Duration::ZERO,
    )
    .unwrap();
    let initial = spy.turn_on_calls();
    assert_eq!(initial.len(), 3);
    let seed = source
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .mood_scene_palette_seed;

    let assignments = source
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .mood_scene_device_slots
        .unwrap();
    let disabled_light = assignments
        .iter()
        .min_by_key(|(_, slot)| *slot)
        .unwrap()
        .0
        .clone();
    commands::do_node_preferences_set(
        &source.state,
        &disabled_light,
        None,
        Some(true),
        None,
        None,
        None,
        true,
    )
    .unwrap();

    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    source.state.lock().unwrap().work_tx = Some(tx);
    let response = handlers::handle_set_node_curve(
        &source.state,
        &json!({
            "nodes": [{"node_id": kitchen, "brightness": 20}, {"node_id": hall, "brightness": 20}],
            "correlation_id": "party-brightness-test",
            "dispatch_spacing_ms": 0
        }),
        true,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&response.body).unwrap()["queued"],
        true
    );
    assert_eq!(
        spy.turn_on_calls().len(),
        3,
        "queue acceptance does not mean physical dispatch"
    );
    process_brightness_batch(&source, &rx, 2, true);
    let changed = spy.turn_on_calls();
    assert_eq!(changed.len(), 5);
    for (node, command) in &changed[3..] {
        assert_eq!(
            command.rgb,
            initial.iter().find(|(id, _)| id == node).unwrap().1.rgb
        );
        assert_eq!(command.brightness, 20);
    }
    for room_id in [&kitchen, &hall] {
        let snapshot = source.snapshot(room_id).unwrap();
        assert!(snapshot.mood_active);
        assert_eq!(snapshot.profile_settings.mood_scene_device_mode, Some(true));
        assert!(
            snapshot.profile_settings.motion_activation_enabled(),
            "durable motion preference is unchanged"
        );
        assert_eq!(snapshot.brightness_offset, 0.0);
    }
    let records = source.state.lock().unwrap().light_activity.clone();
    assert_eq!(records.len(), 2);
    for record in records {
        assert_eq!(record.action_id, "set_curve");
        assert_eq!(record.brightness, Some(20));
        assert_eq!(
            record.correlation_id.as_deref(),
            Some("party-brightness-test")
        );
        assert_eq!(record.fanout_of.as_deref(), Some("party-brightness-test"));
        assert_eq!(record.payload.unwrap()["scene_brightness"], true);
    }

    let bundle = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    let (mut restored, restored_spy) = TestHarness::with_spy_controller();
    let directory = tempfile::tempdir().unwrap();
    restored.state.lock().unwrap().storage = Some(Arc::new(
        storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap(),
    ));
    commands::do_backup_restore(&restored.state, bundle).unwrap();
    let key = restored.add_hub("mock", "192.168.1.100");
    restored.set_hub_discovery(
        &key,
        vec![room("kitchen", "Kitchen"), room("hall", "Hall")],
        vec![
            light("lamp-1", "kitchen"),
            light("lamp-2", "kitchen"),
            light("lamp-3", "hall"),
        ],
    );
    storage::load_persisted_state(&mut restored.state.lock().unwrap());
    restored.sync_all();
    assert_eq!(
        restored
            .snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .mood_scene_palette_seed,
        seed
    );
    assert_eq!(
        restored
            .snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .mood_scene_device_mode,
        Some(true)
    );
    assert_eq!(
        restored
            .snapshot(&kitchen)
            .unwrap()
            .profile_settings
            .mood_scene_device_slots
            .as_ref(),
        Some(&assignments)
    );
    assert!(restored.snapshot(&disabled_light).unwrap().disabled);
    restored_spy.reset();
    for room_id in [&kitchen, &hall] {
        commands::do_node_preferences_set(
            &restored.state,
            room_id,
            None,
            None,
            None,
            Some(RoomModeState::Mood),
            None,
            false,
        )
        .unwrap();
    }
    let resumed = restored_spy.turn_on_calls();
    assert_eq!(resumed.len(), 2);
    for (node, command) in &resumed {
        assert_eq!(
            command.rgb,
            initial.iter().find(|(id, _)| id == node).unwrap().1.rgb
        );
        assert_eq!(command.brightness, 20);
    }
    let reexport = commands::build_backup_bundle_dto(&restored.state, false).unwrap();
    assert!(reexport
        .configuration
        .rooms
        .iter()
        .filter(|room| room.id == kitchen || room.id == hall)
        .all(|room| room.room_profile.mood_scene_device_mode == Some(true)));
    assert_eq!(
        source.state.lock().unwrap().scenes["halloween"]
            .light
            .as_ref()
            .unwrap()
            .palette[0]
            .brightness,
        80
    );
}
