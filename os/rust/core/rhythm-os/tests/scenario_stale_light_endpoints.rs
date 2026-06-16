//! Scenario regression: missing bulbs should hide immediately but reactivate on rediscovery.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

#[test]
fn stale_single_hub_light_hides_node_and_reactivates_same_canonical_device() {
    let harness = TestHarness::new().with_discovery(
        vec![room("office", "Office")],
        vec![light("desk-lamp", "office")],
    );
    harness.sync();

    let room_id = harness.resolve("office");
    let canonical_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "desk-lamp")
            .unwrap()
            .id
            .clone()
    };

    harness.set_hub_discovery(&harness.hub_key, vec![room("office", "Office")], vec![]);
    harness.sync();

    {
        let state = harness.state.lock().unwrap();
        let device = state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "desk-lamp")
            .expect("canonical device should still be remembered by native id");
        assert_eq!(device.id, canonical_id);
        assert_eq!(device.room_id.as_deref(), Some(room_id.as_str()));
        assert!(
            !device.endpoint_by_native_id("desk-lamp").unwrap().active,
            "missing endpoint should be marked inactive"
        );
        assert!(
            state.topology.get_device_node(&canonical_id).is_none(),
            "topology node should be hidden when no active endpoints remain"
        );
        assert!(
            state
                .hub_runtime()
                .expect("runtime should exist")
                .engine_node_snapshot(&canonical_id)
                .is_none(),
            "runtime node should be removed while the device is hidden"
        );
    }

    let hidden_snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    assert!(
        hidden_snapshot["nodes"]
            .as_array()
            .unwrap()
            .iter()
            .all(|node| node["id"].as_str() != Some(canonical_id.as_str())),
        "hidden device should disappear from API state"
    );

    harness.set_hub_discovery(
        &harness.hub_key,
        vec![room("office", "Office")],
        vec![light("desk-lamp", "office")],
    );
    harness.sync();

    {
        let state = harness.state.lock().unwrap();
        let device = state
            .canonical_registry
            .find_by_native_id(&harness.hub_key, "desk-lamp")
            .expect("canonical device should reactivate");
        assert_eq!(device.id, canonical_id);
        assert!(
            device.endpoint_by_native_id("desk-lamp").unwrap().active,
            "rediscovered endpoint should reactivate"
        );
        assert_eq!(
            state
                .topology
                .get_device_node(&canonical_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(room_id.as_str()),
            "device should return to its prior assigned room"
        );
    }
}

#[test]
fn stale_cross_hub_endpoint_hides_only_missing_hub_and_reactivates_cleanly() {
    let harness = TestHarness::new().with_discovery(
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    harness.sync();

    let mut harness = harness;
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let (room_entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("room binding triage should exist");
    commands::do_triage_bind_room(&harness.state, &room_entry_id).unwrap();

    let (merge_entry_id, canonical_id) = {
        let state = harness.state.lock().unwrap();
        let entry = state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .into_iter()
            .next()
            .expect("device merge triage should exist");
        (
            entry.id.clone(),
            entry.candidate_matches[0].canonical_id.clone(),
        )
    };
    commands::do_triage_merge(&harness.state, &merge_entry_id, &canonical_id).unwrap();

    let kitchen_id = harness.resolve("mock-kitchen");
    {
        let state = harness.state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert_eq!(device.active_endpoints().count(), 2);
        assert_eq!(
            state
                .topology
                .get_device_node(&canonical_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(kitchen_id.as_str())
        );
    }

    harness.set_hub_discovery(&ha_key, vec![room("ha-kitchen", "Kitchen")], vec![]);
    harness.sync_hub(&ha_key);

    {
        let state = harness.state.lock().unwrap();
        let device = state.canonical_registry.get(&canonical_id).unwrap();
        assert_eq!(
            device.active_endpoints().count(),
            1,
            "the surviving hub endpoint should keep the device visible"
        );
        assert!(
            !device.endpoint_for_hub(&ha_key).unwrap().active,
            "missing hub endpoint should be inactive"
        );
        assert_eq!(
            state
                .topology
                .get_device_node(&canonical_id)
                .unwrap()
                .parent_id
                .as_deref(),
            Some(kitchen_id.as_str()),
            "device should stay attached through the surviving hub endpoint"
        );
    }

    let snapshot: serde_json::Value =
        serde_json::from_str(&commands::build_state_snapshot(&harness.state).unwrap()).unwrap();
    let node = snapshot["nodes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|node| node["id"].as_str() == Some(canonical_id.as_str()))
        .expect("device node should remain in API state");
    let hub_types: Vec<&str> = node["hub_types"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|value| value.as_str())
        .collect();
    assert!(hub_types.contains(&"mock"));
    assert!(
        !hub_types.contains(&"ha"),
        "inactive hub endpoint should disappear from node hub_types"
    );

    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let state = harness.state.lock().unwrap();
    let device = state.canonical_registry.get(&canonical_id).unwrap();
    assert_eq!(device.active_endpoints().count(), 2);
    assert!(
        device.endpoint_for_hub(&ha_key).unwrap().active,
        "rediscovered hub endpoint should reactivate on the same canonical device"
    );
}
