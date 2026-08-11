//! Scenario regression: a user-moved Hue light must keep its Rhythm room after restart.

mod harness;

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use harness::{light, room, TestHarness};
use rhythm_os::commands;
use rhythm_os::storage::{self, FileStorage};
use rhythm_os::topology::DevicePlacement;

static NEXT_TEMP_STORAGE_ID: AtomicU64 = AtomicU64::new(0);

fn temp_storage_dir() -> std::path::PathBuf {
    let unique_id = NEXT_TEMP_STORAGE_ID.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "rhythm_hue_move_restart_{}_{}",
        std::process::id(),
        unique_id
    ))
}

fn install_storage(harness: &TestHarness, dir: &std::path::Path) {
    let storage = FileStorage::new(dir.to_str().unwrap()).unwrap();
    harness.state.lock().unwrap().storage = Some(Arc::new(storage));
}

#[test]
fn user_moved_hue_light_survives_persisted_load_and_rediscovery() {
    let dir = temp_storage_dir();
    let mut first = TestHarness::new();
    let hue_key = first.add_hub("hue", "192.168.1.10");
    first.set_hub_discovery(
        &hue_key,
        vec![room("hue-source", "Source room")],
        vec![light("hue-light-1", "hue-source")],
    );
    install_storage(&first, &dir);
    first.sync_hub(&hue_key);

    let canonical_id = {
        let state = first.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&hue_key, "hue-light-1")
            .unwrap()
            .id
            .clone()
    };
    let destination_id = {
        let created = commands::do_topology_create_room(&first.state, "Destination room").unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&created).unwrap();
        parsed["id"].as_str().unwrap().to_string()
    };
    commands::do_canonical_assign_room(&first.state, &canonical_id, Some(&destination_id)).unwrap();

    let mut restarted = TestHarness::new();
    let restarted_hue_key = restarted.add_hub("hue", "192.168.1.10");
    restarted.set_hub_discovery(
        &restarted_hue_key,
        vec![room("hue-source", "Source room")],
        vec![light("hue-light-1", "hue-source")],
    );
    install_storage(&restarted, &dir);
    storage::load_persisted_state(&mut restarted.state.lock().unwrap());
    restarted.sync_hub(&restarted_hue_key);

    let state = restarted.state.lock().unwrap();
    let node = state.topology.get_device_node(&canonical_id).unwrap();
    assert_eq!(node.parent_id.as_deref(), Some(destination_id.as_str()));
    assert_eq!(node.placement, DevicePlacement::UserOverride);
    assert_eq!(
        state
            .canonical_registry
            .get(&canonical_id)
            .unwrap()
            .room_id
            .as_deref(),
        Some(destination_id.as_str())
    );
    drop(state);

    let _ = std::fs::remove_dir_all(dir);
}
