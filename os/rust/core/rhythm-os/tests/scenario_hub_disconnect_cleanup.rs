//! Scenario regression: explicit hub disconnect prunes that hub's bindings from API state.

mod harness;

use harness::{rooms_with_lights, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::commands;

fn setup_bound_kitchen() -> (TestHarness, HubKey) {
    let (rooms, devices) =
        rooms_with_lights(&[("mock-kitchen", "Kitchen"), ("mock-bedroom", "Bedroom")]);
    let mut harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let ha_key = harness.add_hub("ha", "192.168.1.200");
    let (ha_rooms, ha_devices) = rooms_with_lights(&[("ha-kitchen", "Kitchen")]);
    harness.set_hub_discovery(&ha_key, ha_rooms, ha_devices);
    harness.sync_hub(&ha_key);

    let (entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("cross-hub room binding triage should exist");
    harness.triage_bind(&entry_id).unwrap();

    assert_eq!(harness.hub_target_count("mock-kitchen"), 2);
    (harness, ha_key)
}

#[test]
fn disconnect_one_hub_removes_its_binding_but_keeps_the_rhythm_room() {
    let (harness, ha_key) = setup_bound_kitchen();
    let kitchen_id = harness.resolve("mock-kitchen");

    commands::do_hub_disconnect_one(&harness.state, "ha", "192.168.1.200").unwrap();

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let kitchen = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"].as_str() == Some(kitchen_id.as_str()))
        .expect("kitchen node should still exist in API state");
    let hub_types: Vec<&str> = kitchen["hub_types"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect();

    let state = harness.state.lock().unwrap();
    let room = state
        .topology
        .get(&kitchen_id)
        .expect("topology room should survive single-hub disconnect");
    assert_eq!(
        room.hub_room_bindings.len(),
        1,
        "only the remaining active hub binding should survive"
    );
    assert!(
        room.hub_room_bindings
            .iter()
            .all(|binding| binding.hub_key != ha_key),
        "disconnected hub binding should be removed"
    );
    assert_eq!(
        state.topology.room_count(),
        2,
        "Rhythm rooms should be preserved"
    );
    assert!(
        !hub_types.contains(&"ha"),
        "API state should not report the removed hub type"
    );
    assert!(
        hub_types.contains(&"mock"),
        "API state should still report the surviving hub type"
    );
}
