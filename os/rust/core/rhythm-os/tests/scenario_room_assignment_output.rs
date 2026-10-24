//! Moving a light into a room must finish its output refresh without holding
//! the topology transaction needed by scene application and hub controllers.

mod harness;

use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::spy_controller::{SpyCall, SpyLightController};
use rhythm_core::{Rgb, RoomModeState};
use rhythm_os::commands::{self, NodeColorScope, NodeColorUpdate};
use rhythm_os::topology::DevicePlacement;

/// Source and target rooms, with the source room's light as the moved device.
fn assignment_fixture() -> (TestHarness, Arc<SpyLightController>, String, String) {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("source", "Source"), ("target", "Target")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let target = harness.resolve("target");
    let device_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-source")
        .unwrap()
        .id
        .clone();
    (harness, spy, device_id, target)
}

fn mood_assignment_fixture(
    standalone: bool,
) -> (TestHarness, Arc<SpyLightController>, String, String) {
    let (harness, spy, device_id, target) = assignment_fixture();

    if standalone {
        commands::do_canonical_assign_room(&harness.state, &device_id, None).unwrap();
        let entry_id = harness
            .state
            .lock()
            .unwrap()
            .canonical_registry
            .triage()
            .pending_by_kind(rhythm_os::canonical::triage::TriageKind::UnassignedDevice)[0]
            .id
            .clone();
        commands::do_triage_new_device(&harness.state, &entry_id).unwrap();
    }

    let color = Rgb::new(0, 64, 255);
    commands::do_set_node_color(
        &harness.state,
        &target,
        NodeColorUpdate {
            rgb: color,
            xy: None,
            brightness: Some(35),
            transition_ms: None,
            scope: NodeColorScope::Mood,
        },
        false,
    )
    .unwrap();
    let target_before = harness.snapshot(&target).unwrap();
    assert!(target_before.mood_active);
    assert!(target_before.profile_settings.mood_scene_id.is_some());
    spy.reset();
    (harness, spy, device_id, target)
}

fn wait_for_committed_move(harness: &TestHarness, device_id: &str, target: &str) {
    let topology_lock = harness
        .state
        .lock()
        .unwrap()
        .external_topology_transaction_lock
        .clone();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let committed = harness
            .state
            .lock()
            .unwrap()
            .topology
            .device_parent_room_id(device_id)
            == Some(target);
        if committed && topology_lock.try_lock().is_ok() {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "waiting for a scene must not retain the room assignment's topology lock"
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}

fn assign_to_mood_room(standalone: bool, scene_in_progress: bool) {
    let (harness, spy, device_id, target) = mood_assignment_fixture(standalone);
    let source = harness.resolve("source");
    let target_before = harness.snapshot(&target).unwrap();
    let scene_lock = harness
        .state
        .lock()
        .unwrap()
        .scene_lifecycle_transaction_lock
        .clone();
    // Simulate another scene operation between acquiring its lifecycle lock
    // and acquiring the topology lock. Assignment must not invert that order.
    let scene_guard = scene_in_progress.then(|| scene_lock.lock().unwrap());
    let state = harness.state.clone();
    let moved_id = device_id.clone();
    let target_id = target.clone();
    let (tx, rx) = mpsc::channel();
    let assignment = std::thread::spawn(move || {
        let result = commands::do_canonical_assign_room(&state, &moved_id, Some(&target_id));
        let _ = tx.send(result);
    });
    if scene_in_progress {
        wait_for_committed_move(&harness, &device_id, &target);
        assert_eq!(
            spy.turn_on_count(),
            0,
            "scene refresh waits for the existing scene operation"
        );
    }
    drop(scene_guard);
    // A regression must fail in bounded time instead of hanging the test suite
    // on a second acquisition of the non-reentrant topology transaction.
    rx.recv_timeout(Duration::from_secs(5))
        .expect("room assignment deadlocked while refreshing its bound Mood scene")
        .unwrap();
    assignment.join().unwrap();

    let state = harness.state.lock().unwrap();
    assert!(state.external_topology_transaction_lock.try_lock().is_ok());
    let node = state.topology.get_device_node(&device_id).unwrap();
    assert_eq!(node.parent_id.as_deref(), Some(target.as_str()));
    assert_eq!(node.placement, DevicePlacement::UserOverride);
    assert_eq!(
        state
            .canonical_registry
            .get(&device_id)
            .unwrap()
            .room_id
            .as_deref(),
        Some(target.as_str())
    );
    drop(state);

    let command = spy
        .last_command_for(&device_id)
        .expect("moved light receives the room's scene");
    assert_eq!(command.rgb, Rgb::new(0, 64, 255));
    assert_eq!(command.brightness, 35);
    assert_eq!(spy.turn_on_count(), 1, "refresh only the moved light");
    assert_eq!(
        harness
            .snapshot(&target)
            .unwrap()
            .profile_settings
            .mood_scene_id,
        target_before.profile_settings.mood_scene_id
    );

    // The next ordinary control and topology mutation must still work.
    harness.action(&source, "on").unwrap();
    commands::do_canonical_assign_room(&harness.state, &device_id, None).unwrap();
    assert!(harness
        .state
        .lock()
        .unwrap()
        .topology
        .device_parent_room_id(&device_id)
        .is_none());
}

#[test]
fn moving_a_light_into_a_bound_mood_scene_completes() {
    assign_to_mood_room(false, false);
}

#[test]
fn assigning_a_standalone_light_into_a_bound_mood_scene_completes() {
    assign_to_mood_room(true, false);
}

#[test]
fn assignment_waiting_for_another_scene_does_not_block_topology() {
    assign_to_mood_room(false, true);
}

#[test]
fn a_second_move_cannot_be_overwritten_by_the_first_moves_delayed_scene() {
    let (harness, spy, device_id, target) = mood_assignment_fixture(false);
    let source = harness.resolve("source");
    let scene_lock = harness
        .state
        .lock()
        .unwrap()
        .scene_lifecycle_transaction_lock
        .clone();
    let scene_guard = scene_lock.lock().unwrap();
    let (first_tx, first_rx) = mpsc::channel();
    let state = harness.state.clone();
    let moved_id = device_id.clone();
    let target_id = target.clone();
    let first = std::thread::spawn(move || {
        let _ = first_tx.send(commands::do_canonical_assign_room(
            &state,
            &moved_id,
            Some(&target_id),
        ));
    });
    wait_for_committed_move(&harness, &device_id, &target);

    // A controller for another device can acquire topology while this light's
    // own command gate still protects the delayed scene and its room move.
    let node_gate = harness
        .state
        .lock()
        .unwrap()
        .node_preference_write_locks
        .get(&device_id)
        .cloned()
        .expect("assignment should own the device's command gate");
    assert!(node_gate.try_lock().is_err());
    let (second_tx, second_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let state = harness.state.clone();
    let moved_id = device_id.clone();
    let source_id = source.clone();
    let second = std::thread::spawn(move || {
        started_tx.send(()).unwrap();
        let _ = second_tx.send(commands::do_canonical_assign_room(
            &state,
            &moved_id,
            Some(&source_id),
        ));
    });
    started_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(matches!(
        second_rx.recv_timeout(Duration::from_millis(50)),
        Err(mpsc::RecvTimeoutError::Timeout)
    ));
    drop(scene_guard);

    first_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    second_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .unwrap();
    first.join().unwrap();
    second.join().unwrap();
    assert_eq!(
        harness.snapshot(&device_id).unwrap().parent_id.as_deref(),
        Some(source.as_str())
    );
    assert_eq!(
        spy.turn_on_count(),
        2,
        "each ordered move refreshes the light once"
    );
    assert_ne!(
        spy.last_command_for(&device_id).unwrap().rgb,
        Rgb::new(0, 64, 255),
        "the old destination's delayed Mood scene must not overwrite the later room output"
    );
}

#[test]
fn a_room_change_during_the_scene_wait_replaces_the_stale_scene() {
    let (harness, spy, device_id, target) = mood_assignment_fixture(false);
    let scene_lock = harness
        .state
        .lock()
        .unwrap()
        .scene_lifecycle_transaction_lock
        .clone();
    let scene_guard = scene_lock.lock().unwrap();
    let (tx, rx) = mpsc::channel();
    let state = harness.state.clone();
    let moved_id = device_id.clone();
    let target_id = target.clone();
    let assignment = std::thread::spawn(move || {
        let _ = tx.send(commands::do_canonical_assign_room(
            &state,
            &moved_id,
            Some(&target_id),
        ));
    });
    wait_for_committed_move(&harness, &device_id, &target);
    // The refresh reads the room before it waits for the scene transaction.
    // Give it time to take that snapshot, then turn the room off.
    std::thread::sleep(Duration::from_millis(100));
    commands::do_node_preferences_set(
        &harness.state,
        &target,
        None,
        None,
        None,
        Some(RoomModeState::HardOff),
        None,
        false,
    )
    .unwrap();
    spy.reset();
    drop(scene_guard);

    rx.recv_timeout(Duration::from_secs(5)).unwrap().unwrap();
    assignment.join().unwrap();

    assert!(harness.snapshot(&target).unwrap().hard_off);
    let last_device_call = spy.calls().into_iter().rev().find(|call| match call {
        SpyCall::TurnOn { room_id, .. } | SpyCall::TurnOff { room_id, .. } => room_id == &device_id,
        SpyCall::AnyLightsOn { .. } => false,
    });
    assert!(
        matches!(last_device_call, Some(SpyCall::TurnOff { .. })),
        "the moved light must end in the room's current Off state, got {last_device_call:?}"
    );
}

#[test]
fn assigning_an_unknown_device_leaves_no_command_gate() {
    let (harness, _spy, _device_id, target) = assignment_fixture();

    let error = commands::do_canonical_assign_room(&harness.state, "missing-device", Some(&target))
        .unwrap_err();

    assert!(error.to_string().contains("Device not found"));
    assert!(!harness
        .state
        .lock()
        .unwrap()
        .node_preference_write_locks
        .contains_key("missing-device"));
}

#[test]
fn room_assignment_preserves_active_off_standby_and_unbound_mood_output() {
    for mode in [
        RoomModeState::Active,
        RoomModeState::HardOff,
        RoomModeState::Standby,
        RoomModeState::Mood,
    ] {
        let (harness, spy, device_id, target) = assignment_fixture();
        commands::do_node_preferences_set(
            &harness.state,
            &target,
            None,
            None,
            None,
            Some(mode),
            None,
            false,
        )
        .unwrap();
        let before = harness.snapshot(&target).unwrap();
        assert!(before.profile_settings.mood_scene_id.is_none());
        spy.reset();

        let outcome = commands::do_canonical_assign_room_with_outcome(
            &harness.state,
            &device_id,
            Some(&target),
        )
        .unwrap();

        assert!(outcome.canonical_committed);
        assert_eq!(
            harness.snapshot(&device_id).unwrap().parent_id.as_deref(),
            Some(target.as_str())
        );
        if mode == RoomModeState::HardOff {
            assert_eq!(spy.turn_off_count(), 1);
            assert_eq!(spy.turn_on_count(), 0);
        } else {
            assert_eq!(
                spy.turn_on_count(),
                1,
                "{mode:?} must refresh the moved light"
            );
            assert_eq!(spy.turn_off_count(), 0);
            assert!(spy.last_command_for(&device_id).is_some());
        }
        let after = harness.snapshot(&target).unwrap();
        assert_eq!(
            (after.hard_off, after.soft_off, after.mood_active),
            (before.hard_off, before.soft_off, before.mood_active)
        );
    }
}

#[test]
fn explicit_room_color_includes_lights_moved_after_home_scene_assignment() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[
        ("source", "Source"),
        ("first", "First"),
        ("second", "Second"),
    ]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    commands::do_home_scene_apply(
        &harness.state,
        "halloween",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    let device_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-source")
        .unwrap()
        .id
        .clone();
    let set_color = |target: &str, rgb| {
        commands::do_set_node_color(
            &harness.state,
            target,
            NodeColorUpdate {
                rgb,
                xy: None,
                brightness: Some(47),
                transition_ms: None,
                scope: NodeColorScope::Mood,
            },
            true,
        )
        .unwrap();
    };
    // The same light works independently before it joins a room whose
    // whole-home scene was applied before this membership change.
    commands::do_canonical_assign_room(&harness.state, &device_id, None).unwrap();
    let entry_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .triage()
        .pending_by_kind(rhythm_os::canonical::triage::TriageKind::UnassignedDevice)[0]
        .id
        .clone();
    commands::do_triage_new_device(&harness.state, &entry_id).unwrap();
    spy.reset();
    set_color(&device_id, Rgb::new(255, 0, 0));
    assert_eq!(
        spy.last_command_for(&device_id).unwrap().rgb,
        Rgb::new(255, 0, 0)
    );

    for (room, rgb) in [
        ("first", Rgb::new(0, 0, 255)),
        ("second", Rgb::new(0, 255, 0)),
    ] {
        let target = harness.resolve(room);
        let before = harness.snapshot(&target).unwrap().profile_settings;
        assert_eq!(before.mood_scene_device_mode, Some(true));
        assert!(!before
            .mood_scene_device_slots
            .as_ref()
            .unwrap()
            .contains_key(&device_id));
        commands::do_canonical_assign_room(&harness.state, &device_id, Some(&target)).unwrap();
        spy.reset();
        set_color(&target, rgb);
        let calls = spy.turn_on_calls();
        let command = calls
            .iter()
            .find(|(id, _)| {
                id == &device_id || id == &rhythm_os::topology::device_light_node_id(&device_id)
            })
            .map(|(_, command)| command)
            .unwrap_or_else(|| {
                panic!("explicit room color must reach the newly assigned light: {calls:?}")
            });
        assert_eq!(command.rgb, rgb);
        assert_eq!(command.brightness, 47);
        assert_eq!(
            spy.turn_on_count(),
            2,
            "only the current room's two lights change"
        );
        let after = harness.snapshot(&target).unwrap().profile_settings;
        assert_eq!(
            after.mood_scene_palette_span, before.mood_scene_palette_span,
            "an explicit color edit retains whole-home motion ownership"
        );
        // Subsequent dimming and Mood resume must continue to include the light.
        spy.reset();
        commands::do_set_node_curve_modifier(
            &harness.state,
            &target,
            commands::NodeCurveModifier::Brightness(25),
            true,
        )
        .unwrap();
        let command = spy
            .last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
            .unwrap();
        assert_eq!(command.rgb, rgb);
        assert_eq!(command.brightness, 25);
        spy.reset();
        harness.action(&target, "off").unwrap();
        commands::do_node_preferences_set(
            &harness.state,
            &target,
            None,
            None,
            None,
            Some(RoomModeState::Mood),
            None,
            true,
        )
        .unwrap();
        assert_eq!(
            spy.last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
                .unwrap()
                .rgb,
            rgb
        );
    }
    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let (mut restored, restored_spy) = TestHarness::with_spy_controller();
    restored.state.lock().unwrap().storage = Some(Arc::new(
        rhythm_os::storage::FileStorage::new(directory.path().to_str().unwrap()).unwrap(),
    ));
    commands::do_backup_restore(&restored.state, bundle).unwrap();
    let key = restored.add_hub("mock", "192.168.1.100");
    let (rooms, devices) = rooms_with_lights(&[
        ("source", "Source"),
        ("first", "First"),
        ("second", "Second"),
    ]);
    restored.set_hub_discovery(&key, rooms, devices);
    rhythm_os::storage::load_persisted_state(&mut restored.state.lock().unwrap());
    restored.sync_all();
    restored_spy.reset();
    let target = restored.resolve("second");
    commands::do_node_preferences_set(
        &restored.state,
        &target,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .unwrap();
    let command = restored_spy
        .last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
        .unwrap();
    assert_eq!(command.rgb, Rgb::new(0, 255, 0));
    assert_eq!(command.brightness, 25);
    assert_eq!(restored_spy.turn_on_count(), 2);
}

#[test]
fn explicit_child_color_after_move_keeps_siblings_and_disabled_lights_untouched() {
    let (harness, spy, device_id, target) = assignment_fixture();
    commands::do_home_scene_apply(
        &harness.state,
        "halloween",
        Default::default(),
        Duration::ZERO,
    )
    .unwrap();
    let room_binding = harness.snapshot(&target).unwrap().profile_settings;
    commands::do_canonical_assign_room(&harness.state, &device_id, Some(&target)).unwrap();
    let color = Rgb::new(255, 0, 128);
    spy.reset();
    commands::do_set_node_color(
        &harness.state,
        &device_id,
        NodeColorUpdate {
            rgb: color,
            xy: None,
            brightness: Some(60),
            transition_ms: None,
            scope: NodeColorScope::Mood,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        spy.turn_on_count(),
        1,
        "a direct color edit changes only its target"
    );
    assert_eq!(
        spy.last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
            .unwrap()
            .rgb,
        color
    );
    assert_eq!(
        harness.snapshot(&target).unwrap().profile_settings,
        room_binding
    );

    commands::do_node_preferences_set(
        &harness.state,
        &device_id,
        None,
        Some(true),
        None,
        None,
        None,
        false,
    )
    .unwrap();
    spy.reset();
    commands::do_set_node_color(
        &harness.state,
        &target,
        NodeColorUpdate {
            rgb: color,
            xy: None,
            brightness: Some(60),
            transition_ms: None,
            scope: NodeColorScope::Mood,
        },
        false,
    )
    .unwrap();
    assert_eq!(
        spy.turn_on_count(),
        1,
        "disabled lights stay excluded from room color edits"
    );
    assert!(spy
        .last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
        .is_none());
    assert!(!harness
        .snapshot(&target)
        .unwrap()
        .profile_settings
        .mood_scene_device_slots
        .unwrap()
        .contains_key(&device_id));
}

fn plain_blue_after_assignment(stale_saved_membership: bool) {
    let (harness, spy, device_id, target) = mood_assignment_fixture(false);
    // Start with an ordinary single-color room. No whole-home scene is applied.
    // Older saved settings can nevertheless retain per-device dispatch metadata;
    // reproduce that state without assuming which user action created it.
    if stale_saved_membership {
        let (runtime, existing) = {
            let state = harness.state.lock().unwrap();
            (
                state.hub_runtime().unwrap(),
                state
                    .canonical_registry
                    .find_by_native_id(&harness.hub_key, "light-target")
                    .unwrap()
                    .id
                    .clone(),
            )
        };
        let snapshot = runtime.engine_node_snapshot(&target).unwrap();
        let mut saved = rhythm_core::RestoredNodeState::from(&snapshot);
        saved.profile_settings.mood_scene_device_mode = Some(true);
        saved.profile_settings.mood_scene_palette_span = Some(2);
        saved.profile_settings.mood_scene_device_slots =
            Some(std::collections::BTreeMap::from([(existing, 0)]));
        runtime.restore_node_state(&target, saved);
    }
    commands::do_canonical_assign_room(&harness.state, &device_id, Some(&target)).unwrap();
    spy.reset();
    let response = rhythm_os::handlers::handle_set_node_color(
        &harness.state,
        &serde_json::json!({
            "node_id": target,
            "rgb": {"r": 0, "g": 0, "b": 255},
            "brightness": 80,
            "scope": "auto"
        }),
        false,
    );
    assert_eq!(response.status, 200, "{}", response.body);
    let command = spy
        .last_command_for(&rhythm_os::topology::device_light_node_id(&device_id))
        .or_else(|| spy.last_command_for(&target))
        .expect("plain room blue must dispatch to the moved light or its current room");
    assert_eq!(command.rgb, Rgb::new(0, 0, 255));
    assert_eq!(command.brightness, 80);
}

#[test]
fn plain_room_blue_after_assignment_works_without_saved_device_assignments() {
    plain_blue_after_assignment(false);
}

#[test]
fn plain_room_blue_repairs_stale_saved_membership_without_applying_a_home_scene() {
    plain_blue_after_assignment(true);
}
