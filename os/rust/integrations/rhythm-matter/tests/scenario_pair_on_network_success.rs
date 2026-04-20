#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_on_network_success() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi",
                "rendezvous": "on_network"
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);
    assert_eq!(session.device.unwrap().device_id, "matter-100");

    let requests = rig.transport.commission_requests();
    assert_eq!(
        requests[0].rendezvous,
        rhythm_matter::transport::MatterCommissioningRendezvous::OnNetwork
    );
}
