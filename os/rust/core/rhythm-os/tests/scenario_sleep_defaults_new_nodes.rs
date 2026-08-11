//! New synced rooms and lights should opt out of Sleep mode by default.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_core::{ModeConfig, RhythmMode, RoomModeDefault, RoomModeState};
use rhythm_os::commands::{self, RoomParams};

fn sleep_default_state(harness: &TestHarness, node_id: &str) -> Option<RoomModeState> {
    harness
        .state
        .lock()
        .unwrap()
        .mode_configs()
        .into_iter()
        .find(|config| config.mode == RhythmMode::Sleep)
        .and_then(|config| {
            config
                .room_defaults
                .into_iter()
                .find(|default| default.room_id == node_id)
                .map(|default| default.state)
        })
}

#[test]
fn synced_new_room_defaults_to_hard_off_in_sleep_mode() {
    let harness = TestHarness::new().with_discovery(vec![room("nursery", "Nursery")], vec![]);
    harness.sync();

    let nursery_id = harness.resolve("nursery");
    assert_eq!(
        sleep_default_state(&harness, &nursery_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn manually_created_room_defaults_to_hard_off_in_sleep_mode() {
    let harness = TestHarness::new();
    let created: serde_json::Value = serde_json::from_str(
        &commands::do_topology_create_room(&harness.state, "Guest Bath Sink")
            .expect("room creation should succeed"),
    )
    .expect("room response should be valid JSON");
    let room_id = created["id"]
        .as_str()
        .expect("room response should include an id");

    assert_eq!(
        sleep_default_state(&harness, room_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn new_synced_light_in_existing_room_seeds_parent_sleep_default() {
    let harness = TestHarness::new().with_discovery(vec![room("studio", "Studio")], vec![]);
    harness.sync();
    let studio_id = harness.resolve("studio");

    harness.set_mode_configs(vec![ModeConfig::default_for_mode(RhythmMode::Sleep)]);
    assert_eq!(sleep_default_state(&harness, &studio_id), None);

    harness.set_hub_discovery(
        &harness.hub_key,
        vec![room("studio", "Studio")],
        vec![light("light-studio", "studio")],
    );
    harness.sync();

    assert_eq!(
        sleep_default_state(&harness, &studio_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn new_room_device_list_light_seeds_parent_sleep_default_before_identity_sync() {
    let harness = TestHarness::new().with_discovery(vec![room("studio", "Studio")], vec![]);
    harness.sync();
    let studio_id = harness.resolve("studio");

    harness.set_mode_configs(vec![ModeConfig::default_for_mode(RhythmMode::Sleep)]);
    assert_eq!(sleep_default_state(&harness, &studio_id), None);

    commands::do_room_set(
        &harness.state,
        &RoomParams {
            id: "studio".to_string(),
            name: "Studio".to_string(),
            grouped_light_id: "studio_grouped".to_string(),
            rhythm_enabled: true,
            disabled: false,
            state: None,
            device_ids: vec!["hub-light-studio".to_string()],
        },
        &harness.hub_key,
        false,
        false,
    )
    .expect("room_set should accept updated light membership");

    assert_eq!(
        sleep_default_state(&harness, &studio_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn roomless_synced_light_defaults_to_hard_off_in_sleep_mode() {
    let harness = TestHarness::new().with_discovery(vec![], vec![light("matter-100", "")]);
    harness.sync();

    let canonical_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "matter-100")
        .expect("roomless light should be canonical-registered")
        .id
        .clone();

    assert_eq!(
        sleep_default_state(&harness, &canonical_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn new_synced_light_preserves_existing_parent_sleep_default() {
    let harness = TestHarness::new().with_discovery(vec![room("den", "Den")], vec![]);
    harness.sync();
    let den_id = harness.resolve("den");
    let mut sleep = ModeConfig::default_for_mode(RhythmMode::Sleep);
    sleep.room_defaults = vec![RoomModeDefault {
        room_id: den_id.clone(),
        state: RoomModeState::Active,
    }];
    harness.set_mode_configs(vec![sleep]);

    harness.set_hub_discovery(
        &harness.hub_key,
        vec![room("den", "Den")],
        vec![light("light-den", "den")],
    );
    harness.sync();

    assert_eq!(
        sleep_default_state(&harness, &den_id),
        Some(RoomModeState::Active)
    );
}

#[test]
fn moving_room_backed_light_seeds_destination_sleep_default() {
    let harness = TestHarness::new().with_discovery(
        vec![room("matter-100", "Matter Bulb")],
        vec![light("matter-100", "matter-100")],
    );
    harness.sync();

    let source_room_id = harness.resolve("matter-100");
    let canonical_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "matter-100")
        .expect("light should be canonical-registered")
        .id
        .clone();
    let created: serde_json::Value = serde_json::from_str(
        &commands::do_topology_create_room(&harness.state, "Guest Bath Sink")
            .expect("room creation should succeed"),
    )
    .expect("room response should be valid JSON");
    let destination_room_id = created["id"]
        .as_str()
        .expect("room response should include an id")
        .to_string();

    let mut sleep = ModeConfig::default_for_mode(RhythmMode::Sleep);
    sleep.room_defaults = vec![RoomModeDefault {
        room_id: source_room_id,
        state: RoomModeState::HardOff,
    }];
    harness.set_mode_configs(vec![sleep]);
    assert_eq!(
        sleep_default_state(&harness, &destination_room_id),
        None,
        "pre-fix persisted rooms can be missing the destination default"
    );

    commands::do_canonical_assign_room(&harness.state, &canonical_id, Some(&destination_room_id))
        .expect("room assignment should succeed");

    assert_eq!(
        sleep_default_state(&harness, &destination_room_id),
        Some(RoomModeState::HardOff)
    );
}

#[test]
fn moving_room_backed_light_preserves_destination_sleep_choice() {
    let harness = TestHarness::new().with_discovery(
        vec![room("matter-100", "Matter Bulb")],
        vec![light("matter-100", "matter-100")],
    );
    harness.sync();

    let canonical_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "matter-100")
        .expect("light should be canonical-registered")
        .id
        .clone();
    let created: serde_json::Value = serde_json::from_str(
        &commands::do_topology_create_room(&harness.state, "Night Light")
            .expect("room creation should succeed"),
    )
    .expect("room response should be valid JSON");
    let destination_room_id = created["id"]
        .as_str()
        .expect("room response should include an id")
        .to_string();
    let mut sleep = ModeConfig::default_for_mode(RhythmMode::Sleep);
    sleep.room_defaults = vec![RoomModeDefault {
        room_id: destination_room_id.clone(),
        state: RoomModeState::Active,
    }];
    harness.set_mode_configs(vec![sleep]);

    commands::do_canonical_assign_room(&harness.state, &canonical_id, Some(&destination_room_id))
        .expect("room assignment should succeed");

    assert_eq!(
        sleep_default_state(&harness, &destination_room_id),
        Some(RoomModeState::Active)
    );
}
