#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_phone_setup_payload_handoff_keeps_original_recovery_code() {
    let rig = harness::connect_rig(None);
    let original = "MT:Y.K908OC16750648G00";
    let temporary = "34970112332";
    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": original,
                "handoff_setup_payload": temporary,
                "network": "wifi",
                "rendezvous": "phone"
            }),
        )
        .unwrap();
    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);
    let device = session.device.unwrap();
    let requests = rig.transport.commission_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].setup_payload, temporary);
    assert_eq!(
        requests[0].rendezvous,
        rhythm_matter::transport::MatterCommissioningRendezvous::OnNetwork
    );
    let recovery = rhythm_matter::setup_recovery::load_setup_payload(
        &rig.state,
        &rig.hub_data.fabric_id,
        &device.device_id,
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovery.setup_payload, original);
}
