//! Scenario regression: unassigning a device must preserve standalone query targets.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::commands;
use rhythm_os::topology::DevicePlacement;

#[test]
fn unassign_device_keeps_synthetic_registry_room_for_standalone_queries() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "light-office")
            .unwrap()
            .id
            .clone()
    };

    commands::do_canonical_assign_room(&harness.state, &canonical_id, None).unwrap();

    let state = harness.state.lock().unwrap();
    let node = state
        .topology
        .get_device_node(&canonical_id)
        .expect("standalone topology device node should exist");
    assert_eq!(node.parent_id, None);
    // Explicit user-unassign is a UserOverride with no parent, so the next
    // sync's hub-default re-attach skips the device (issue #43).
    assert_eq!(node.placement, DevicePlacement::UserOverride);
    assert_eq!(
        state
            .canonical_registry
            .get(&canonical_id)
            .unwrap()
            .room_id
            .as_deref(),
        None
    );
    let registry = state
        .hubs
        .get(&harness.hub_key)
        .and_then(|hub| hub.registry.as_ref())
        .expect("hub registry should exist")
        .lock()
        .unwrap();
    assert_eq!(
        registry.devices_for_room("light-office"),
        vec!["light-office".to_string()]
    );
    assert_eq!(
        registry.get_grouped_light_id("light-office"),
        Some("light-office".to_string())
    );
    drop(registry);
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert!(
        state
            .hub_runtime()
            .and_then(|runtime| runtime.engine_node_snapshot(&canonical_id))
            .is_none(),
        "unassigned device should stay quarantined until triage resolves it"
    );
}
