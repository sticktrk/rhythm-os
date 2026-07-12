//! Scenario: per-room motion admission survives the full appliance lifecycle.

mod harness;

use std::sync::atomic::{AtomicU64, Ordering};

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::bundle::{BACKUP_BUNDLE_SCHEMA_VERSION, LEGACY_BACKUP_SCHEMA_VERSION};
use rhythm_os::commands::{self, RoomProfileSettingsPatch};
use rhythm_os::storage::{self, FileStorage};

static NEXT_TEMP_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

fn temp_storage_dir() -> std::path::PathBuf {
    let unique_id = NEXT_TEMP_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rhythm_motion_activation_{}_{}",
        std::process::id(),
        unique_id
    ))
}

fn install_storage(harness: &TestHarness, dir: &std::path::Path) {
    let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
    harness.state.lock().unwrap().storage = Some(std::sync::Arc::new(storage));
}

fn set_motion_activation(harness: &TestHarness, node_id: &str, enabled: bool, persist: bool) {
    commands::do_node_preferences_set(
        &harness.state,
        node_id,
        None,
        None,
        None,
        None,
        Some(&RoomProfileSettingsPatch {
            motion_activation_enabled: Some(Some(enabled)),
            ..Default::default()
        }),
        persist,
    )
    .expect("set motion activation");
}

#[test]
fn motion_only_preference_does_not_dispatch_a_light_command() {
    let (harness, spy) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    harness.action("kitchen", "on").unwrap();
    spy.reset();

    let room_id = harness.resolve("kitchen");
    set_motion_activation(&harness, &room_id, false, false);

    assert_eq!(spy.turn_on_count(), 0);
    assert_eq!(spy.turn_off_count(), 0);
    assert!(!harness
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .motion_activation_enabled());
}

#[test]
fn motion_activation_survives_restart() {
    let dir = temp_storage_dir();
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let first = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&first, &dir);
    first.sync();
    let room_id = first.resolve("kitchen");
    set_motion_activation(&first, &room_id, false, true);

    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let restarted = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());
    restarted.sync();

    assert!(!restarted
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .motion_activation_enabled());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn motion_activation_survives_backup_restore_reconnect_and_reexport() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let source = TestHarness::new().with_discovery(rooms, devices);
    source.sync();
    let room_id = source.resolve("kitchen");
    set_motion_activation(&source, &room_id, false, false);

    let bundle = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    assert_eq!(bundle.schema_version, BACKUP_BUNDLE_SCHEMA_VERSION);
    assert_eq!(
        bundle
            .installation
            .rooms
            .get(&room_id)
            .unwrap()
            .profile_settings
            .motion_activation_enabled,
        Some(false)
    );
    assert_eq!(
        bundle
            .configuration
            .rooms
            .iter()
            .find(|room| room.id == room_id)
            .unwrap()
            .room_profile
            .motion_activation_enabled,
        Some(false)
    );

    let restore_dir = temp_storage_dir();
    let restored = TestHarness::new();
    install_storage(&restored, &restore_dir);
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let reexported = commands::build_backup_bundle_dto(&restored.state, false).unwrap();
    assert_eq!(reexported.schema_version, BACKUP_BUNDLE_SCHEMA_VERSION);
    assert_eq!(
        reexported
            .installation
            .rooms
            .get(&room_id)
            .unwrap()
            .profile_settings
            .motion_activation_enabled,
        Some(false)
    );

    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let reconnected = TestHarness::new().with_discovery(rooms, devices);
    install_storage(&reconnected, &restore_dir);
    storage::load_persisted_state(&mut reconnected.state.lock().unwrap());
    reconnected.sync();
    assert!(!reconnected
        .snapshot("kitchen")
        .unwrap()
        .profile_settings
        .motion_activation_enabled());

    let _ = std::fs::remove_dir_all(restore_dir);
}

#[test]
fn legacy_v1_backup_without_motion_preference_restores_enabled_default() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let source = TestHarness::new().with_discovery(rooms, devices);
    source.sync();
    let room_id = source.resolve("kitchen");

    let mut legacy = commands::build_backup_bundle_dto(&source.state, false).unwrap();
    legacy.schema_version = LEGACY_BACKUP_SCHEMA_VERSION;
    for room in legacy.installation.rooms.iter_mut() {
        room.profile_settings.motion_activation_enabled = None;
    }
    for room in &mut legacy.configuration.rooms {
        room.room_profile.motion_activation_enabled = None;
    }

    let restore_dir = temp_storage_dir();
    let restored = TestHarness::new();
    install_storage(&restored, &restore_dir);
    commands::do_backup_restore(&restored.state, legacy).unwrap();
    let reexported = commands::build_backup_bundle_dto(&restored.state, false).unwrap();

    assert!(reexported
        .installation
        .rooms
        .get(&room_id)
        .unwrap()
        .profile_settings
        .motion_activation_enabled());
    let _ = std::fs::remove_dir_all(restore_dir);
}
