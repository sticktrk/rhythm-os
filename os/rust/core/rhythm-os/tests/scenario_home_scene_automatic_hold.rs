//! A whole-home scene holds its output across automatic work until explicit exit.
mod harness;

use harness::{light, room, TestHarness};
use rhythm_core::{
    LightScheduleConfig, ModeTransitionConfig, ModeTransitionTime, ModeTransitionTrigger,
    RhythmMode, RoomModeDefault, RoomModeState,
};
use rhythm_os::{commands, periodic};
use std::time::Duration;

fn sleep_boundary() -> ModeTransitionConfig {
    ModeTransitionConfig::new(RhythmMode::Day, RhythmMode::Sleep, 0)
        .with_id("sleep_at_22")
        .with_trigger(ModeTransitionTrigger::Scheduled(
            ModeTransitionTime::from_hour_minute(22, 0).unwrap(),
        ))
}

#[test]
fn global_clock_boundary_must_not_replace_halloween_output() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(
        vec![room("party", "Party")],
        vec![light("lamp-1", "party"), light("lamp-2", "party")],
    );
    h.sync();
    h.state
        .lock()
        .unwrap()
        .set_mode_transition_configs(vec![sleep_boundary()]);
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    let initial = spy.turn_on_calls();
    assert_eq!(initial.len(), 2);
    assert!(initial.iter().all(|(_, command)| command.is_direct_color));
    spy.reset();

    // Production periodic boundary detector, with no user mode request.
    periodic::check_mode_transitions(&h.state, 21.99, 22.01);
    assert_eq!(h.state.lock().unwrap().active_mode, RhythmMode::Sleep);
    assert!(h.snapshot("party").unwrap().mood_active);
    assert!(
        spy.turn_on_calls().is_empty(),
        "automatic mode change replaced Halloween: {:?}",
        spy.turn_on_calls()
    );
    assert!(spy.turn_off_calls().is_empty());
}

#[test]
fn global_sleep_default_must_not_exit_halloween() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
    h.sync();
    let id = h.resolve("party");
    let mut configs = rhythm_core::default_mode_configs();
    configs
        .iter_mut()
        .find(|c| c.mode == RhythmMode::Sleep)
        .unwrap()
        .room_defaults
        .push(RoomModeDefault {
            room_id: id,
            state: RoomModeState::HardOff,
        });
    h.set_mode_configs(configs);
    h.state
        .lock()
        .unwrap()
        .set_mode_transition_configs(vec![sleep_boundary()]);
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    spy.reset();
    periodic::check_mode_transitions(&h.state, 21.99, 22.01);
    let snapshot = h.snapshot("party").unwrap();
    assert!(
        snapshot.mood_active && !snapshot.hard_off,
        "automatic Sleep default exited party: mood={}, hard_off={}, off_writes={:?}",
        snapshot.mood_active,
        snapshot.hard_off,
        spy.turn_off_calls()
    );
}

#[test]
fn named_clock_boundary_must_not_exit_halloween() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
    h.sync();
    let id = h.resolve("party");
    {
        let mut state = h.state.lock().unwrap();
        state.utc_offset_hours = 0.0;
        state.timezone_name = None;
    }
    commands::do_light_schedules_set(
        &h.state,
        vec![LightScheduleConfig {
            id: "party-room-schedule".into(),
            name: "Party room schedule".into(),
            enabled: true,
            active_mode: RhythmMode::Day,
            transitions: vec![sleep_boundary()],
        }],
    )
    .unwrap();
    commands::do_light_schedule_assignment_set(&h.state, &id, Some("party-room-schedule"), false)
        .unwrap();
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    spy.reset();
    let date = chrono::NaiveDate::from_ymd_opt(2026, 10, 19).unwrap();
    periodic::check_light_schedule_transitions_between(
        &h.state,
        date.and_hms_opt(21, 59, 0).unwrap(),
        date.and_hms_opt(22, 1, 0).unwrap(),
    );
    let snapshot = h.snapshot("party").unwrap();
    assert_eq!(
        snapshot
            .profile_settings
            .light_schedule
            .unwrap()
            .active_mode(),
        Some(RhythmMode::Sleep)
    );
    assert!(
        snapshot.mood_active,
        "named schedule exited Halloween; writes={:?}",
        spy.turn_on_calls()
    );
    assert!(spy.turn_on_calls().is_empty());
}

#[test]
fn ordinary_rediscovery_preserves_halloween_without_light_writes() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(
        vec![room("party", "Party")],
        vec![light("lamp-1", "party"), light("lamp-2", "party")],
    );
    h.sync();
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    let before = h.snapshot("party").unwrap();
    spy.reset();
    h.sync();
    rhythm_os::room_sync::poll_initial_light_state(&h.state);
    let after = h.snapshot("party").unwrap();
    assert_eq!(after.profile_settings, before.profile_settings);
    assert!(after.mood_active);
    assert!(spy.turn_on_calls().is_empty());
    assert!(spy.turn_off_calls().is_empty());
}

fn queue_home(h: &TestHarness) -> Vec<rhythm_os::state::WorkItem> {
    queue_scene(
        h,
        "halloween",
        rhythm_os::scenes::HomeSceneTargetMode::Devices,
    )
}

fn queue_scene(
    h: &TestHarness,
    scene_id: &str,
    target_mode: rhythm_os::scenes::HomeSceneTargetMode,
) -> Vec<rhythm_os::state::WorkItem> {
    let (tx, rx) = std::sync::mpsc::sync_channel(32);
    h.state.lock().unwrap().work_tx = Some(tx);
    let applied = commands::do_home_scene_apply(
        &h.state,
        scene_id,
        rhythm_os::scenes::HomeSceneApplyRequest {
            target_mode,
            ..Default::default()
        },
        Duration::ZERO,
    )
    .unwrap();
    (0..applied.dispatch_count)
        .map(|_| rx.recv_timeout(Duration::from_secs(5)).unwrap())
        .collect()
}

fn advance_to_sleep(h: &TestHarness) {
    h.state
        .lock()
        .unwrap()
        .set_mode_transition_configs(vec![sleep_boundary()]);
    periodic::check_mode_transitions(&h.state, 21.99, 22.01);
}

#[test]
fn automatic_boundary_finishes_an_in_progress_scene_queue() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(
        vec![room("party", "Party")],
        vec![light("lamp-1", "party"), light("lamp-2", "party")],
    );
    h.sync();
    let items = queue_home(&h);
    assert!(spy.turn_on_calls().is_empty());
    advance_to_sleep(&h);
    for item in items {
        rhythm_os::event_loop::process_work_item(&h.state, item);
    }
    let calls = spy.turn_on_calls();
    assert_eq!(
        calls.len(),
        2,
        "clock advancement must not cancel a paced scene"
    );
    assert!(calls.iter().all(|(_, output)| output.is_direct_color));
    assert!(h.snapshot("party").unwrap().mood_active);
}

#[test]
fn brightness_retires_old_scene_output_only_for_the_edited_owner() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(
        vec![room("party", "Party"), room("other", "Other")],
        vec![
            light("lamp-1", "party"),
            light("lamp-2", "party"),
            light("lamp-3", "other"),
            light("lamp-4", "other"),
        ],
    );
    h.sync();
    let items = queue_home(&h);
    commands::do_set_node_brightness(&h.state, &h.resolve("party"), 35, true).unwrap();
    let dimmed = spy.turn_on_calls();
    assert_eq!(dimmed.len(), 2);
    for item in items {
        rhythm_os::event_loop::process_work_item(&h.state, item);
    }
    let all = spy.turn_on_calls();
    assert_eq!(
        all.len(),
        4,
        "old output must retire for edited room, while other room completes"
    );
    for (node, output) in dimmed {
        assert_eq!(spy.last_command_for(&node).unwrap(), output);
        assert_eq!(output.brightness, 35);
    }
}

#[test]
fn scene_start_retires_older_queued_manual_and_automatic_mode_output() {
    use rhythm_os::state::{NodeOutputProvenance, WorkItem};
    for preserve_home_scene in [false, true] {
        let (h, spy) = TestHarness::with_spy_controller();
        let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
        h.sync();
        let generation = h.state.lock().unwrap().light_dispatch_generation;
        let stale_on = WorkItem::ApplyNodeCommand {
            command_id: "older-mode-on".into(),
            node_id: h.resolve("party"),
            command: rhythm_core::LightingCommand::with_transition(12, 2700, 0),
            dispatch_spacing: Duration::ZERO,
            dispatch_generation: generation,
            provenance: NodeOutputProvenance::Mode {
                preserve_home_scene,
            },
        };
        let stale_off = WorkItem::LightsOffRoom {
            command_id: "older-mode-off".into(),
            node_id: h.resolve("party"),
            transition_ms: None,
            dispatch_spacing: Duration::ZERO,
            dispatch_generation: generation,
            provenance: NodeOutputProvenance::Mode {
                preserve_home_scene,
            },
        };
        commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
            .unwrap();
        spy.reset();
        for item in [stale_on, stale_off] {
            rhythm_os::event_loop::process_work_item(&h.state, item);
        }
        assert!(spy.turn_on_calls().is_empty());
        assert!(spy.turn_off_calls().is_empty());
    }
}

#[test]
fn queued_scene_output_retires_after_scene_exit_or_breaker_cycle() {
    use rhythm_os::state::WorkItem;
    for exit in ["reset", "breaker"] {
        let (h, spy) = TestHarness::with_spy_controller();
        let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
        h.sync();
        let mut items = queue_home(&h);
        // Exercise both queued power routes with the real scene ownership token.
        if let WorkItem::ApplyNodeCommand {
            node_id,
            dispatch_generation,
            provenance,
            ..
        } = &items[0]
        {
            items.push(WorkItem::LightsOffRoom {
                command_id: "scene-off".into(),
                node_id: node_id.clone(),
                transition_ms: None,
                dispatch_spacing: Duration::ZERO,
                dispatch_generation: *dispatch_generation,
                provenance: provenance.clone(),
            });
        } else {
            panic!("Halloween should queue On");
        }
        if exit == "reset" {
            commands::do_node_action(&h.state, &h.resolve("party"), "reset", false).unwrap();
        } else {
            commands::do_light_breaker_set(&h.state, false).unwrap();
            commands::do_light_breaker_set(&h.state, true).unwrap();
        }
        spy.reset();
        for item in items {
            rhythm_os::event_loop::process_work_item(&h.state, item);
        }
        assert!(
            spy.turn_on_calls().is_empty(),
            "{exit} must retire scene On"
        );
        assert!(
            spy.turn_off_calls().is_empty(),
            "{exit} must retire scene Off"
        );
    }
}

#[test]
fn stale_periodic_tick_cannot_reconcile_a_scene_back_to_adaptive() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
    h.sync();
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    spy.reset();
    // Even a tick with the current generation must consult the current owner.
    let item = rhythm_os::state::WorkItem::PeriodicNodeTick {
        command_id: "old-periodic-plan".into(),
        node_id: h.resolve("party"),
        settings_node_id: h.resolve("party"),
        current_hour: 23.0,
        emit_parent_node_id: None,
        dispatch_spacing: Duration::ZERO,
        dispatch_generation: h.state.lock().unwrap().light_dispatch_generation,
    };
    rhythm_os::event_loop::process_work_item(&h.state, item);
    assert!(h.snapshot("party").unwrap().mood_active);
    assert!(spy.turn_on_calls().is_empty());
    assert!(spy.turn_off_calls().is_empty());
}

#[test]
fn disk_restart_and_schedule_replay_preserve_scene_before_any_manual_reapply() {
    use rhythm_os::storage;
    use std::sync::Arc;
    for named in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let disk = Arc::new(storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap());
        let (source, _) = TestHarness::with_spy_controller();
        source.state.lock().unwrap().storage = Some(disk.clone());
        let source = source.with_discovery(
            vec![room("party", "Party")],
            vec![light("lamp-1", "party"), light("lamp-2", "party")],
        );
        source.sync();
        let id = source.resolve("party");
        let mut configs = rhythm_core::default_mode_configs();
        configs
            .iter_mut()
            .find(|c| c.mode == RhythmMode::Sleep)
            .unwrap()
            .room_defaults
            .push(RoomModeDefault {
                room_id: id.clone(),
                state: RoomModeState::HardOff,
            });
        source.set_mode_configs(configs);
        {
            let mut state = source.state.lock().unwrap();
            state.utc_offset_hours = 0.0;
            state.timezone_name = None;
            state.set_mode_transition_configs(vec![sleep_boundary()]);
        }
        commands::do_set_active_mode(&source.state, RhythmMode::Day).unwrap();
        if named {
            commands::do_light_schedules_set(
                &source.state,
                vec![LightScheduleConfig {
                    id: "party-schedule".into(),
                    name: "Party schedule".into(),
                    enabled: true,
                    active_mode: RhythmMode::Day,
                    transitions: vec![sleep_boundary()],
                }],
            )
            .unwrap();
            commands::do_light_schedule_assignment_set(
                &source.state,
                &id,
                Some("party-schedule"),
                false,
            )
            .unwrap();
        }
        commands::do_home_scene_apply(
            &source.state,
            "halloween",
            Default::default(),
            Duration::ZERO,
        )
        .unwrap();
        commands::do_set_node_brightness(&source.state, &id, 35, true).unwrap();
        let before = source.snapshot(&id).unwrap();

        let (restored, spy) = TestHarness::with_spy_controller();
        restored.state.lock().unwrap().storage = Some(disk);
        storage::load_persisted_state(&mut restored.state.lock().unwrap());
        if !named {
            // Production replay can change global mode before hub bootstrap.
            periodic::check_mode_transitions(&restored.state, 21.99, 22.01);
            assert!(restored.state.lock().unwrap().pending_mode_output_apply);
        }
        let restored = restored.with_discovery(
            vec![room("party", "Party")],
            vec![light("lamp-1", "party"), light("lamp-2", "party")],
        );
        restored.sync();
        assert!(
            restored.snapshot(&id).unwrap().mood_active,
            "load+sync must restore active ownership without Mood reapply"
        );
        if named {
            periodic::reconcile_light_schedule_transitions(
                &restored.state,
                chrono::NaiveDate::from_ymd_opt(2026, 10, 19)
                    .unwrap()
                    .and_hms_opt(22, 1, 0)
                    .unwrap(),
            );
            assert_eq!(
                restored
                    .snapshot(&id)
                    .unwrap()
                    .profile_settings
                    .light_schedule
                    .unwrap()
                    .active_mode(),
                Some(RhythmMode::Sleep)
            );
        }
        let after = restored.snapshot(&id).unwrap();
        assert!(after.mood_active && !after.hard_off);
        assert_eq!(
            after.profile_settings.mood_scene_id,
            before.profile_settings.mood_scene_id
        );
        assert_eq!(
            after.profile_settings.mood_scene_palette_seed,
            before.profile_settings.mood_scene_palette_seed
        );
        assert_eq!(
            after.profile_settings.mood_scene_device_slots,
            before.profile_settings.mood_scene_device_slots
        );
        rhythm_os::room_sync::poll_initial_light_state(&restored.state);
        let mut timers = rhythm_os::event_loop::MotionTimerState::new();
        rhythm_os::event_loop::handle_hub_event(
            &restored.state,
            rhythm_os::hub::HubEvent::LightPower {
                hub_key: Some(restored.hub_key.clone()),
                device_id: "lamp-1".into(),
                lights_on: false,
            },
            &mut timers,
        );
        rhythm_os::event_loop::handle_hub_event(
            &restored.state,
            rhythm_os::hub::HubEvent::LightObserved {
                hub_key: Some(restored.hub_key.clone()),
                device_id: "lamp-1".into(),
                observation: rhythm_os::hub::LightObservation {
                    availability: "available".into(),
                    lights_on: Some(false),
                    brightness: Some(7),
                    kelvin: Some(6500),
                    xy: None,
                    rgb: None,
                    context_id: None,
                    source_at_epoch_ms: Some(1234),
                    received_at_epoch_ms: 1234,
                },
                identity_proof: None,
                observation_epoch: None,
                external_user_change: false,
            },
            &mut timers,
        );
        {
            let state = restored.state.lock().unwrap();
            let light = state
                .canonical_registry
                .find_by_native_id(&restored.hub_key, "lamp-1")
                .unwrap();
            assert_eq!(state.light_observations[&light.id].brightness, Some(7));
        }
        assert!(restored.snapshot(&id).unwrap().mood_active);
        assert!(
            spy.turn_on_calls().is_empty(),
            "replay/readback must not replace or reassert scene output"
        );
        assert!(spy.turn_off_calls().is_empty());
    }
}

#[test]
fn manual_mode_exit_and_ordinary_custom_mood_keep_existing_schedule_behavior() {
    for home_scene in [false, true] {
        let (h, spy) = TestHarness::with_spy_controller();
        let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
        h.sync();
        let id = h.resolve("party");
        let mut configs = rhythm_core::default_mode_configs();
        configs
            .iter_mut()
            .find(|c| c.mode == RhythmMode::Sleep)
            .unwrap()
            .room_defaults
            .push(RoomModeDefault {
                room_id: id.clone(),
                state: RoomModeState::HardOff,
            });
        h.set_mode_configs(configs);
        if home_scene {
            commands::do_home_scene_apply(
                &h.state,
                "halloween",
                Default::default(),
                Duration::ZERO,
            )
            .unwrap();
        } else {
            commands::do_node_preferences_set(
                &h.state,
                &id,
                None,
                None,
                None,
                Some(RoomModeState::Mood),
                None,
                false,
            )
            .unwrap();
        }
        spy.reset();
        if home_scene {
            commands::do_set_active_mode(&h.state, RhythmMode::Sleep).unwrap();
        } else {
            advance_to_sleep(&h);
        }
        assert!(h.snapshot(&id).unwrap().hard_off);
        assert!(!h.snapshot(&id).unwrap().mood_active);
        assert!(!spy.turn_off_calls().is_empty());
    }
}

#[test]
fn scheduled_defaults_do_not_mutate_a_child_under_its_parent_scene() {
    let (h, spy) = TestHarness::with_spy_controller();
    let h = h.with_discovery(vec![room("party", "Party")], vec![light("lamp-1", "party")]);
    h.sync();
    let child = h
        .state
        .lock()
        .unwrap()
        .topology
        .device_nodes()
        .next()
        .unwrap()
        .id
        .clone();
    commands::do_node_preferences_set(
        &h.state,
        &child,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .unwrap();
    let mut configs = rhythm_core::default_mode_configs();
    configs
        .iter_mut()
        .find(|c| c.mode == RhythmMode::Sleep)
        .unwrap()
        .room_defaults
        .push(RoomModeDefault {
            room_id: child.clone(),
            state: RoomModeState::HardOff,
        });
    h.set_mode_configs(configs);
    commands::do_home_scene_apply(&h.state, "halloween", Default::default(), Duration::ZERO)
        .unwrap();
    let runtime = h.state.lock().unwrap().hub_runtime().unwrap();
    let before = runtime.engine_node_snapshot(&child).unwrap();
    spy.reset();
    advance_to_sleep(&h);
    let after = runtime.engine_node_snapshot(&child).unwrap();
    assert_eq!(after.mood_active, before.mood_active);
    assert_eq!(after.hard_off, before.hard_off);
    assert_eq!(after.profile_settings, before.profile_settings);
    assert!(spy.turn_on_calls().is_empty());
    assert!(spy.turn_off_calls().is_empty());
}

#[test]
fn automatic_boundary_preserves_room_mode_scene_queue_and_dark_output() {
    use rhythm_os::scenes;
    for target_mode in [
        scenes::HomeSceneTargetMode::Devices,
        scenes::HomeSceneTargetMode::Rooms,
    ] {
        let (h, spy) = TestHarness::with_spy_controller();
        let h = h.with_discovery(
            vec![room("party", "Party")],
            vec![light("lamp-1", "party"), light("lamp-2", "party")],
        );
        h.sync();
        let dark_light = h
            .state
            .lock()
            .unwrap()
            .topology
            .device_nodes()
            .next()
            .unwrap()
            .id
            .clone();
        let mut scene = h.state.lock().unwrap().scenes["halloween"].clone();
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
        commands::do_scene_upsert(&h.state, scene).unwrap();
        let items = queue_scene(&h, "party-with-dark-light", target_mode);
        advance_to_sleep(&h);
        for item in items {
            rhythm_os::event_loop::process_work_item(&h.state, item);
        }
        assert_eq!(spy.turn_on_count(), 1);
        assert_eq!(spy.turn_off_count(), 1);
        assert!(h.snapshot("party").unwrap().mood_active);
    }
}
