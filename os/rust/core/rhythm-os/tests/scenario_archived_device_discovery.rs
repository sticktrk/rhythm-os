//! Integrations that do not own explicit commissioning keep rediscovery behavior.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::commands;

#[test]
fn default_discovery_restores_the_archived_canonical_identity() {
    let (harness, _) = TestHarness::with_spy_controller();
    let (rooms, devices) = rooms_with_lights(&[("room", "Room")]);
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let device_id = harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-room")
        .unwrap()
        .id
        .clone();
    commands::do_canonical_rename_device(&harness.state, &device_id, "Custom light").unwrap();
    commands::do_device_archive(&harness.state, &device_id, &harness.hub_key).unwrap();
    assert!(harness
        .state
        .lock()
        .unwrap()
        .canonical_registry
        .get(&device_id)
        .unwrap()
        .is_removed());

    harness.sync();
    let state = harness.state.lock().unwrap();
    let restored = state
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-room")
        .unwrap();
    assert!(!restored.is_removed());
    assert_eq!(restored.id, device_id);
    assert_eq!(restored.name, "Custom light");
}
