#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};

#[test]
fn scenario_unpair_removes_device_from_canonical_state() {
    let rig = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    rhythm_os::room_sync::sync_all_hubs(&rig.state).unwrap();

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let canonical_id = {
        let state = rig.state.lock().unwrap();
        state
            .canonical_registry
            .find_by_native_id(&hub_key, "matter-200")
            .expect("synced Matter device should exist before unpair")
            .id
            .clone()
    };

    let result = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_unpairing(
            &rig.state,
            &serde_json::json!({
                "device_id": canonical_id
            }),
        )
        .unwrap();

    assert_eq!(result.status, rhythm_os::pairing::PairingStatus::Complete);

    let state = rig.state.lock().unwrap();
    assert!(
        state
            .canonical_registry
            .find_by_native_id(&hub_key, "matter-200")
            .is_none(),
        "unpair should remove the Matter device from canonical registry"
    );
    assert!(
        state.topology.get_device_node(&canonical_id).is_none(),
        "unpair should remove the topology node"
    );
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        0,
        "unpair should clear unassigned-device triage for the removed device"
    );
}
