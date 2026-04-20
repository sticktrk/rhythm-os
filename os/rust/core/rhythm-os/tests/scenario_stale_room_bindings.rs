//! Scenario regression: stale hub rooms remove bindings, not Rhythm rooms.

mod harness;

use harness::{rooms_with_lights, TestHarness};

#[test]
fn resync_with_no_rooms_prunes_source_binding_but_preserves_rhythm_room() {
    let (rooms, devices) = rooms_with_lights(&[("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let topology_room_id = harness.resolve("office");
    {
        let state = harness.state.lock().unwrap();
        let room = state
            .topology
            .get(&topology_room_id)
            .expect("topology room should exist after initial sync");
        assert_eq!(room.hub_room_bindings.len(), 1);
    }

    harness.set_hub_discovery(&harness.hub_key, vec![], vec![]);
    let report = harness.sync();

    assert_eq!(
        report.rooms_removed, 1,
        "empty rediscovery should prune the stale source room"
    );

    let registry = {
        let state = harness.state.lock().unwrap();
        let room = state
            .topology
            .get(&topology_room_id)
            .expect("Rhythm room should survive binding removal");
        assert!(
            room.hub_room_bindings.is_empty(),
            "source bindings should be removed"
        );
        assert!(
            state
                .hub_runtime()
                .expect("runtime should still exist")
                .engine_room_snapshot(&topology_room_id)
                .is_some(),
            "runtime room should remain because only bindings were pruned"
        );
        state
            .hubs
            .get(&harness.hub_key)
            .and_then(|hub| hub.registry.clone())
            .expect("hub registry should exist")
    };

    let registry_rooms = registry.lock().unwrap().rooms();
    assert!(
        registry_rooms.is_empty(),
        "native source room should be removed from the hub registry"
    );
}
