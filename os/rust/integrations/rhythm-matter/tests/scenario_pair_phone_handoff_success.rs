#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_phone_handoff_success() {
    let rig = harness::connect_rig(None);

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "MT:Y.K908OC16750648G00",
                "network": "wifi",
                "rendezvous": "phone",
                "handoff_address": "192.0.2.10",
                "handoff_port": 5540,
                "handoff_passcode": 20202021
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);
    let device = session.device.unwrap();
    assert_eq!(device.device_id, "matter-100");

    let requests = rig.transport.commission_requests();
    assert_eq!(requests.len(), 1);
    let request = &requests[0];
    assert_eq!(
        request.rendezvous,
        rhythm_matter::transport::MatterCommissioningRendezvous::OnNetwork
    );
    assert!(request.wifi_credentials.ssid.is_empty());
    assert!(request.wifi_credentials.password.is_empty());
    assert_eq!(request.setup_payload, "MT:Y.K908OC16750648G00");
    let target = request
        .on_network_target
        .as_ref()
        .expect("phone handoff should supply the on-network target");
    assert_eq!(target.address, "192.0.2.10");
    assert_eq!(target.port, 5540);
    assert_eq!(target.setup_pin_code, 20202021);

    let recovery = rhythm_matter::setup_recovery::load_setup_payload(
        &rig.state,
        &rig.hub_data.fabric_id,
        &device.device_id,
    )
    .unwrap()
    .expect("the user's original code should remain the recovery authority");
    assert_eq!(recovery.setup_payload, "MT:Y.K908OC16750648G00");
}
