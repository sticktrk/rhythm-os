//! Scenario regression: initial hub sync assigns canonical devices to imported rooms.

mod harness;

use harness::{rooms_with_lights, TestHarness};

#[test]
fn initial_sync_assigns_canonical_devices_to_imported_rooms() {
    let (rooms, devices) =
        rooms_with_lights(&[("hue-kitchen", "Kitchen"), ("hue-bedroom", "Bedroom")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);

    let report = harness.sync();
    assert_eq!(report.rooms_added, 2);
    assert_eq!(harness.triage_pending_count(), 0);

    let kitchen_room_id = harness.resolve("hue-kitchen");
    let bedroom_room_id = harness.resolve("hue-bedroom");

    let state = harness.state.lock().unwrap();
    let kitchen_device = state
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-hue-kitchen")
        .expect("kitchen device should be in canonical registry");
    let bedroom_device = state
        .canonical_registry
        .find_by_native_id(&harness.hub_key, "light-hue-bedroom")
        .expect("bedroom device should be in canonical registry");

    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        0
    );
    assert_eq!(
        kitchen_device.room_id.as_deref(),
        Some(kitchen_room_id.as_str())
    );
    assert_eq!(
        bedroom_device.room_id.as_deref(),
        Some(bedroom_room_id.as_str())
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&kitchen_device.id)
            .and_then(|node| node.parent_id.as_deref()),
        Some(kitchen_room_id.as_str())
    );
    assert_eq!(
        state
            .topology
            .get_device_node(&bedroom_device.id)
            .and_then(|node| node.parent_id.as_deref()),
        Some(bedroom_room_id.as_str())
    );
}
