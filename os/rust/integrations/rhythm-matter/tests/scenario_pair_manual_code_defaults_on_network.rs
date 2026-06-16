#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_manual_code_defaults_on_network() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi"
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);

    let requests = rig.transport.commission_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(
        requests[0].rendezvous,
        rhythm_matter::transport::MatterCommissioningRendezvous::OnNetwork
    );
}
