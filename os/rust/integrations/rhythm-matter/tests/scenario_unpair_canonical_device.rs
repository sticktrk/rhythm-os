#![cfg(all(feature = "desktop", feature = "test-support"))]

#[path = "common/mod.rs"]
mod harness;

use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::canonical::registry::ResolveResult;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};

#[test]
fn scenario_unpair_accepts_canonical_device_id() {
    let rig = harness::connect_rig(None);
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    let canonical_id = {
        let mut state = rig.state.lock().unwrap();
        let identity = DiscoveredIdentity {
            native_id: "matter-44".to_string(),
            room_id: "matter-44".to_string(),
            room_name: "Desk Lamp".to_string(),
            name: "Desk Lamp".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("44")],
            manufacturer: None,
            model: None,
        };

        match state.canonical_registry.resolve(&identity, &hub_key, 1) {
            ResolveResult::AlreadyKnown { canonical_id }
            | ResolveResult::ReApproved { canonical_id }
            | ResolveResult::Created { canonical_id } => canonical_id,
            ResolveResult::Queued { .. } => panic!("unexpected triage for test device"),
        }
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
    assert_eq!(result.device_id.as_deref(), Some("matter-44"));
    assert_eq!(rig.transport.decommissioned(), vec![(44, false)]);
}
