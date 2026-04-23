//! Scenario regression: explicit room-binding target selection must merge into the chosen room.

mod harness;

use harness::{light, room, TestHarness};

#[test]
fn room_binding_target_override_merges_third_hub_into_requested_room() {
    let harness = TestHarness::new().with_discovery(
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("mock-lamp", "mock-kitchen")],
    );
    harness.sync();

    let mut harness = harness;
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("ha-lamp", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let (ha_entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("second hub should queue a room binding proposal");
    let result = harness.triage_new(&ha_entry_id).unwrap();
    assert_eq!(result, r#"{"status":"kept_separate"}"#);
    assert_eq!(harness.topology_room_count(), 2);

    let aux_key = harness.add_hub("aux", "192.168.1.250");
    harness.set_hub_discovery(
        &aux_key,
        vec![room("aux-kitchen", "Kitchen")],
        vec![light("aux-lamp", "aux-kitchen")],
    );
    harness.sync_hub(&aux_key);

    let (entry_id, _, default_target_id) = harness
        .triage_room_binding(0)
        .expect("third hub should queue a room binding proposal");
    let mock_room_id = harness.resolve("mock-kitchen");
    let ha_room_id = harness.resolve("ha-kitchen");
    let chosen_target = if default_target_id == mock_room_id {
        ha_room_id.clone()
    } else {
        mock_room_id.clone()
    };
    let untouched_room = if chosen_target == mock_room_id {
        ha_room_id.clone()
    } else {
        mock_room_id.clone()
    };

    harness.triage_bind_to(&entry_id, &chosen_target).unwrap();

    let state = harness.state.lock().unwrap();
    assert_eq!(state.topology.room_count(), 2);
    assert_eq!(
        state.topology.translate_room_id(&aux_key, "aux-kitchen"),
        Some(chosen_target.as_str()),
        "third hub should merge into the explicitly selected target room"
    );
    assert_eq!(
        state
            .topology
            .get(&chosen_target)
            .expect("chosen target room should exist")
            .hub_room_bindings
            .len(),
        2,
        "chosen target should now own both its original hub and the third hub"
    );
    assert_eq!(
        state
            .topology
            .get(&untouched_room)
            .expect("untouched room should still exist")
            .hub_room_bindings
            .len(),
        1,
        "the non-selected room should remain independent"
    );
    assert_eq!(state.canonical_registry.triage().pending_room_count(), 0);
}
