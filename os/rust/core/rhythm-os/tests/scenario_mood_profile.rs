//! Scenario: first-class mood profile state.
//!
//! Mood is a real room state backed by the mode's mood profile. Entering mood
//! renders that profile once, and periodic ticks must not overwrite it.

mod harness;

use std::sync::atomic::{AtomicU64, Ordering};

use harness::*;
use rhythm_core::{Rgb, RoomModeState};
use rhythm_os::commands;
use rhythm_os::storage::{self, FileStorage};

static NEXT_TEMP_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

fn temp_storage_dir() -> std::path::PathBuf {
    let unique_id = NEXT_TEMP_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rhythm_mood_standby_restart_{}_{}",
        std::process::id(),
        unique_id
    ))
}

fn install_storage(harness: &TestHarness, dir: &std::path::Path) {
    let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
    harness.state.lock().unwrap().storage = Some(Box::new(storage));
}

#[test]
fn mood_enabled_does_not_change_off_behavior() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.set_room_mood_enabled("kitchen", true);

    harness.action("kitchen", "on").unwrap();
    spy.reset();
    harness.action("kitchen", "off").unwrap();

    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.hard_off, "off should be a real off state");
    assert!(!snap.soft_off, "off should not enter mood");
    assert_eq!(spy.turn_off_count(), 1);
    assert_eq!(spy.turn_on_count(), 0);
}

#[test]
fn mood_preference_renders_mood_profile() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();

    let resolved = harness.resolve("kitchen");
    commands::do_node_preferences_set(
        &harness.state,
        &resolved,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .expect("mood preference");

    let snap = harness.snapshot("kitchen").unwrap();
    assert!(!snap.hard_off);
    assert!(snap.mood_active);
    assert!(!snap.soft_off);
    assert_eq!(spy.turn_off_count(), 0);
    assert_eq!(spy.turn_on_count(), 1);
    let cmd = spy
        .last_command_for(&resolved)
        .expect("mood should render a light command");
    assert_eq!(cmd.brightness, 1);

    let node_state = commands::build_node_state(&harness.state, &resolved).expect("node state");
    let parsed = serde_json::to_value(node_state).unwrap();
    assert_eq!(parsed["state"], "mood");
    assert_eq!(parsed["mood_enabled"], true);
    assert_eq!(parsed["mood_active"], true);
}

#[test]
fn mood_color_scope_enters_mood_and_periodic_does_not_overwrite_it() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(vec![room("kitchen", "Kitchen")], vec![]);
    harness.sync();
    harness.action("kitchen", "on").unwrap();
    spy.reset();

    let resolved = harness.resolve("kitchen");
    commands::do_set_node_color(
        &harness.state,
        &resolved,
        commands::NodeColorUpdate {
            rgb: Rgb::new(0, 64, 255),
            xy: None,
            brightness: Some(35),
            transition_ms: Some(250),
            scope: commands::NodeColorScope::Mood,
        },
        false,
    )
    .expect("set mood-scoped color");

    let cmd = spy
        .last_command_for(&resolved)
        .expect("color should dispatch directly");
    assert!(cmd.is_direct_color);
    assert_eq!(cmd.rgb, Rgb::new(0, 64, 255));
    assert_eq!(cmd.brightness, 35);
    assert_eq!(cmd.transition_ms, Some(250));

    let snap = harness.snapshot("kitchen").unwrap();
    assert!(snap.mood_active);
    assert!(!snap.soft_off);
    assert!(!snap.hard_off);
    assert!(snap.profile_settings.mood_profile_id.is_some());

    let node_state = commands::build_node_state(&harness.state, &resolved).expect("node state");
    let parsed = serde_json::to_value(node_state).unwrap();
    assert_eq!(parsed["state"], "mood");

    spy.reset();
    let runtime = harness.state.lock().unwrap().hub_runtime().unwrap();
    runtime.periodic_tick_room(&resolved, 14.0).unwrap();
    assert_eq!(spy.turn_on_count(), 0, "periodic must not overwrite mood");
    assert_eq!(spy.turn_off_count(), 0, "periodic must not turn mood off");
}

#[test]
fn mood_and_standby_survive_restart_from_persisted_rooms() {
    let dir = temp_storage_dir();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("den", "Den")]);
    let first = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&first, &dir);
    first.sync();

    let kitchen_id = first.resolve("kitchen");
    let den_id = first.resolve("den");
    commands::do_node_preferences_set(
        &first.state,
        &kitchen_id,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        true,
    )
    .expect("persist mood state");
    commands::do_node_preferences_set(
        &first.state,
        &den_id,
        None,
        None,
        Some(true),
        Some(RoomModeState::Standby),
        None,
        true,
    )
    .expect("persist standby state");

    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("den", "Den")]);
    let restarted = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());
    restarted.sync();

    let kitchen = restarted.snapshot("kitchen").unwrap();
    assert!(kitchen.mood_active, "Mood should survive restart");
    assert!(
        !kitchen.soft_off,
        "Mood should not reload as Standby after restart"
    );
    assert!(!kitchen.hard_off, "Mood should not reload as hard-off");

    let den = restarted.snapshot("den").unwrap();
    assert!(den.soft_off, "Standby should survive restart");
    assert!(
        den.standby_enabled,
        "Standby preference should survive restart"
    );
    assert!(
        !den.mood_active,
        "Standby should not reload as Mood after restart"
    );
    assert!(!den.hard_off, "Standby should not reload as hard-off");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn mood_and_standby_survive_backup_restore() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("den", "Den")]);
    let source = TestHarness::new().with_discovery(rooms, devices);
    source.sync();

    let kitchen_id = source.resolve("kitchen");
    let den_id = source.resolve("den");
    commands::do_node_preferences_set(
        &source.state,
        &kitchen_id,
        None,
        None,
        None,
        Some(RoomModeState::Mood),
        None,
        false,
    )
    .expect("set mood state");
    commands::do_node_preferences_set(
        &source.state,
        &den_id,
        None,
        None,
        Some(true),
        Some(RoomModeState::Standby),
        None,
        false,
    )
    .expect("set standby state");

    let bundle = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    let backup_kitchen = bundle
        .installation
        .rooms
        .get(&kitchen_id)
        .expect("backup should include mood room");
    assert!(backup_kitchen.mood_active);
    assert!(!backup_kitchen.soft_off);
    assert!(!backup_kitchen.hard_off);
    let backup_den = bundle
        .installation
        .rooms
        .get(&den_id)
        .expect("backup should include standby room");
    assert!(backup_den.soft_off);
    assert!(backup_den.standby_enabled);
    assert!(!backup_den.mood_active);
    assert!(!backup_den.hard_off);

    let restore_dir = temp_storage_dir();
    let restored = TestHarness::new();
    install_storage(&restored, &restore_dir);
    commands::do_backup_restore(&restored.state, bundle).expect("restore backup bundle");

    let restored_bundle = commands::build_backup_bundle_dto(&restored.state, false).unwrap();
    let restored_kitchen = restored_bundle
        .installation
        .rooms
        .get(&kitchen_id)
        .expect("restored backup should include mood room");
    assert!(
        restored_kitchen.mood_active,
        "Mood should survive backup restore"
    );
    assert!(
        !restored_kitchen.soft_off,
        "Mood should not restore as Standby"
    );
    assert!(
        !restored_kitchen.hard_off,
        "Mood should not restore as hard-off"
    );

    let restored_den = restored_bundle
        .installation
        .rooms
        .get(&den_id)
        .expect("restored backup should include standby room");
    assert!(
        restored_den.soft_off,
        "Standby should survive backup restore"
    );
    assert!(
        restored_den.standby_enabled,
        "Standby preference should survive backup restore"
    );
    assert!(
        !restored_den.mood_active,
        "Standby should not restore as Mood"
    );
    assert!(
        !restored_den.hard_off,
        "Standby should not restore as hard-off"
    );

    let _ = std::fs::remove_dir_all(restore_dir);
}
