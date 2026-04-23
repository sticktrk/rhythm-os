#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;

#[test]
fn scenario_repeated_sync_does_not_duplicate_roomless_matter_state() {
    let rig = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    let first = rhythm_os::room_sync::sync_all_hubs(&rig.state).unwrap();
    let second = rhythm_os::room_sync::sync_all_hubs(&rig.state).unwrap();
    assert_eq!(first.rooms_added, 0);
    assert_eq!(second.rooms_added, 0);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = rig.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-200")
        .expect("roomless Matter device should remain addressable after repeated sync");
    let canonical_id = canonical.id.clone();

    assert_eq!(state.canonical_registry.device_count(), 1);
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_by_kind(rhythm_os::canonical::triage::TriageKind::UnassignedDevice)
            .len(),
        1
    );
    assert!(
        state.topology.get_device_node(&canonical_id).is_some(),
        "repeated sync should not drop the standalone topology node"
    );
}
