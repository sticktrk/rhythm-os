//! Coverage-buffer scenarios from field behavior logs.
//!
//! These extend the first log-derived scenario file with higher-level user and
//! light behavior: periodic-visible transition state, app write bursts, external
//! power reports, motion warning/timeout behavior, and storage/restart paths.

mod harness;

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use harness::{light, motion_sensor, room, rooms_with_lights, TestHarness};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::{RhythmMode, RoomModeState};
use rhythm_os::commands::{self, QueuedNodePreferencesPatch};
use rhythm_os::discovery::DiscoveredDevice;
use rhythm_os::event_loop::{self, MotionSourceState, MotionTimerState};
use rhythm_os::hub::HubEvent;
use rhythm_os::state::{RoomModeTransition, WorkItem};
use rhythm_os::storage::{self, FileStorage, Storage};

static NEXT_TEMP_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

fn temp_storage_dir(prefix: &str) -> std::path::PathBuf {
    let unique_id = NEXT_TEMP_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rhythm_{prefix}_{}_{}",
        std::process::id(),
        unique_id
    ))
}

fn install_storage(harness: &TestHarness, dir: &std::path::Path) {
    let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
    harness.state.lock().unwrap().storage = Some(std::sync::Arc::new(storage));
}

fn contact_sensor(device_id: &str, room_id: &str) -> DiscoveredDevice {
    DiscoveredDevice {
        device_id: device_id.to_string(),
        room_id: (!room_id.is_empty()).then(|| room_id.to_string()),
        buttons: vec![],
        device_type: DeviceType::Contact,
    }
}

fn light_node_id(harness: &TestHarness, native_id: &str) -> String {
    let state = harness.state.lock().unwrap();
    state
        .canonical_registry
        .find_by_native_id(&harness.hub_key, native_id)
        .expect("light should be registered")
        .id
        .clone()
}

fn wait_for(mut predicate: impl FnMut() -> bool, message: &str) {
    for _ in 0..100 {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(predicate(), "{}", message);
}

#[test]
fn transition_active_room_is_visible_then_user_action_clears_transition() {
    let (rooms, devices) = rooms_with_lights(&[("studio", "Studio")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();
    let studio_id = harness.resolve("studio");

    {
        let mut state = harness.state.lock().unwrap();
        state.room_mode_transitions.insert(
            studio_id.clone(),
            RoomModeTransition {
                ends_at: Instant::now() + Duration::from_secs(2),
                periodic_resume_at: Instant::now() + Duration::from_secs(60),
            },
        );
    }

    let before: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let studio = before["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"].as_str() == Some(studio_id.as_str()))
        .expect("studio node should be visible");
    assert_eq!(studio["transitioning"], true);

    harness.action("studio", "on").unwrap();

    assert!(
        !harness
            .state
            .lock()
            .unwrap()
            .room_mode_transitions
            .contains_key(&studio_id),
        "explicit user action should clear transition hold for that room"
    );
}

#[test]
fn rhythm_disabled_room_stays_visible_but_periodic_worker_does_not_dispatch() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let office_id = harness.resolve("office");

    commands::do_node_preferences_set(
        &harness.state,
        &office_id,
        Some(false),
        None,
        None,
        None,
        None,
        false,
    )
    .unwrap();

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let office = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"].as_str() == Some(office_id.as_str()))
        .expect("disabled room should remain visible");
    assert_eq!(office["rhythm_enabled"], false);

    let generation = harness.state.lock().unwrap().light_dispatch_generation;
    event_loop::process_work_item(
        &harness.state,
        WorkItem::PeriodicNodeTick {
            command_id: "periodic-rhythm-disabled".into(),
            node_id: office_id.clone(),
            settings_node_id: office_id,
            current_hour: 14.0,
            emit_parent_node_id: None,
            dispatch_spacing: Duration::ZERO,
            dispatch_generation: generation,
        },
    );

    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
}

#[test]
fn motion_clear_enters_warning_then_owned_timeout_turns_room_off() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(
        vec![room("hall", "Hall")],
        vec![
            light("light-hall", "hall"),
            motion_sensor("motion-hall", "hall"),
        ],
    );
    harness.sync();
    let hall_id = harness.resolve("hall");
    let mut motion = MotionTimerState::new();

    motion.sensors.insert(
        "motion-hall".into(),
        MotionSourceState {
            source_node_id: "motion-hall".into(),
            target_node_id: hall_id.clone(),
            stopped_at: Some(Instant::now() - Duration::from_secs(1_150)),
            stopped_at_epoch_ms: None,
        },
    );
    motion.motion_owned.insert(hall_id.clone());
    harness.set_lights_on("hall", true);

    event_loop::check_motion_timers(&harness.state, &mut motion);
    assert!(motion.warning_active.contains(&hall_id));

    if let Some(source) = motion.sensors.get_mut("motion-hall") {
        source.stopped_at = Some(Instant::now() - Duration::from_secs(1_201));
    }
    spy.reset();

    event_loop::check_motion_timers(&harness.state, &mut motion);
    wait_for(
        || spy.turn_off_count() >= 1,
        "expired owned motion countdown should turn the room off",
    );

    assert!(!motion.motion_owned.contains(&hall_id));
    assert!(!motion.warning_active.contains(&hall_id));
    assert!(!motion.sensors.contains_key("motion-hall"));
}

#[test]
fn external_physical_light_reports_update_observed_state_without_dispatch() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let mut motion = MotionTimerState::new();

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::LightPower {
            hub_key: Some(harness.hub_key.clone()),
            device_id: "light-kitchen".into(),
            lights_on: true,
        },
        &mut motion,
    );
    assert!(harness.lights_on("kitchen"));

    event_loop::handle_hub_event(
        &harness.state,
        HubEvent::LightPower {
            hub_key: Some(harness.hub_key.clone()),
            device_id: "light-kitchen".into(),
            lights_on: false,
        },
        &mut motion,
    );

    assert!(!harness.lights_on("kitchen"));
    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
}

#[test]
fn queued_preference_batch_updates_parent_and_light_device_nodes() {
    let harness = TestHarness::new().with_discovery(
        vec![room("drop", "Drop Zone")],
        vec![light("matter-100", "drop")],
    );
    harness.sync();
    let drop_id = harness.resolve("drop");
    let light_id = light_node_id(&harness, "matter-100");
    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(8);
    harness.state.lock().unwrap().work_tx = Some(work_tx);

    commands::queue_node_preferences_batch(
        &harness.state,
        vec![
            QueuedNodePreferencesPatch {
                node_id: drop_id.clone(),
                rhythm_enabled: Some(true),
                disabled: None,
                standby_enabled: Some(true),
                target_state: Some(RoomModeState::Standby),
                room_profile: None,
            },
            QueuedNodePreferencesPatch {
                node_id: light_id.clone(),
                rhythm_enabled: Some(false),
                disabled: Some(true),
                standby_enabled: None,
                target_state: None,
                room_profile: None,
            },
        ],
        false,
        Duration::ZERO,
    )
    .unwrap();

    for _ in 0..2 {
        let item = work_rx.recv_timeout(Duration::from_secs(1)).unwrap();
        event_loop::process_work_item(&harness.state, item);
    }

    let room_snap = harness.snapshot("drop").unwrap();
    assert!(room_snap.soft_off);
    assert!(room_snap.standby_enabled);

    let light_snap = harness
        .state
        .lock()
        .unwrap()
        .hub_runtime()
        .unwrap()
        .engine_node_snapshot(&light_id)
        .expect("light node should remain addressable");
    assert!(light_snap.disabled);
    assert!(!light_snap.rhythm_enabled);
}

#[test]
fn queued_app_action_can_recover_a_hard_off_room() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("den", "Den")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let den_id = harness.resolve("den");
    harness.action("den", "lights_off").unwrap();
    assert!(harness.snapshot("den").unwrap().hard_off);

    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(4);
    harness.state.lock().unwrap().work_tx = Some(work_tx);
    spy.reset();

    commands::queue_node_action(&harness.state, &den_id, "on", false, Duration::ZERO).unwrap();
    let item = work_rx.recv_timeout(Duration::from_secs(1)).unwrap();
    event_loop::process_work_item(&harness.state, item);

    assert!(spy.turn_on_count() >= 1);
    assert!(harness.lights_on("den"));
    assert!(!harness.snapshot("den").unwrap().hard_off);
}

#[test]
fn topology_with_inputs_and_lights_persists_and_reloads_before_runtime() {
    let dir = temp_storage_dir("topology_reload");
    let first = TestHarness::new().with_discovery(
        vec![room("drop", "Drop Zone")],
        vec![
            light("matter-100", "drop"),
            motion_sensor("motion-drop", "drop"),
            contact_sensor("contact-drop", "drop"),
        ],
    );
    install_storage(&first, &dir);
    first.sync();

    {
        let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
        let state = first.state.lock().unwrap();
        storage
            .save_canonical_registry(&serde_json::to_value(&state.canonical_registry).unwrap())
            .unwrap();
        storage
            .save_topology(&serde_json::to_value(&state.topology).unwrap())
            .unwrap();
    }

    let restarted = TestHarness::new();
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&restarted.state).unwrap()).unwrap();
    let nodes = snapshot["nodes"].as_array().unwrap();
    for expected_name in ["matter-100", "motion-drop", "contact-drop"] {
        assert!(
            nodes
                .iter()
                .any(|node| node["name"].as_str() == Some(expected_name)),
            "reloaded topology should expose {expected_name}"
        );
    }

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn corrupt_persisted_topology_degrades_to_empty_topology() {
    let dir = temp_storage_dir("corrupt_topology");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("topology.json"), "{").unwrap();
    std::fs::write(dir.join("canonical_registry.json"), "{").unwrap();

    let harness = TestHarness::new();
    install_storage(&harness, &dir);
    storage::load_persisted_state(&mut harness.state.lock().unwrap());

    assert_eq!(harness.state.lock().unwrap().topology.room_count(), 0);
    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    assert!(snapshot["nodes"].as_array().unwrap().is_empty());

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn persisted_mode_change_survives_restart_and_sync() {
    let dir = temp_storage_dir("mode_restart");
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let first = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&first, &dir);
    first.sync();

    commands::do_settings_set(
        &first.state,
        Some(false),
        Some(RhythmMode::Sleep),
        None,
        None,
        Some(false),
        None,
    )
    .unwrap();
    {
        let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
        let state = first.state.lock().unwrap();
        storage
            .save_canonical_registry(&serde_json::to_value(&state.canonical_registry).unwrap())
            .unwrap();
        storage
            .save_topology(&serde_json::to_value(&state.topology).unwrap())
            .unwrap();
    }

    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let restarted = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());
    restarted.sync();

    let state = restarted.state.lock().unwrap();
    assert_eq!(state.active_mode, RhythmMode::Sleep);
    assert!(!state.power_save);
    assert!(!state.auto_update);

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn state_snapshot_after_runtime_loss_uses_reloaded_topology() {
    let dir = temp_storage_dir("runtime_loss_topology");
    let first = TestHarness::new().with_discovery(
        vec![room("mud", "Mud Room")],
        vec![
            light("matter-200", "mud"),
            contact_sensor("switch-mud", "mud"),
        ],
    );
    install_storage(&first, &dir);
    first.sync();
    {
        let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
        let state = first.state.lock().unwrap();
        storage
            .save_canonical_registry(&serde_json::to_value(&state.canonical_registry).unwrap())
            .unwrap();
        storage
            .save_topology(&serde_json::to_value(&state.topology).unwrap())
            .unwrap();
    }

    let restarted = TestHarness::new();
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());
    {
        let mut state = restarted.state.lock().unwrap();
        for hub in state.hubs.values_mut() {
            hub.runtime = None;
        }
    }

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&restarted.state).unwrap()).unwrap();
    let node_names: Vec<_> = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|node| node["name"].as_str())
        .collect();
    assert!(node_names.contains(&"Mud Room"));
    assert!(node_names.contains(&"matter-200"));
    assert!(node_names.contains(&"switch-mud"));

    let _ = std::fs::remove_dir_all(dir);
}
