#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_ble_wifi_success() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "MT:Y.K908OC16750648G00",
                "network": "wifi",
                "rendezvous": "ble"
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);
    assert_eq!(session.device.unwrap().device_id, "matter-100");

    let requests = rig.transport.commission_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].setup_payload, "MT:Y.K908OC16750648G00");
    assert_eq!(
        requests[0].rendezvous,
        rhythm_matter::transport::MatterCommissioningRendezvous::Ble
    );
    assert_eq!(requests[0].wifi_credentials.ssid, "RhythmNet");

    let commissioned = rig.hub_data.commissioned.lock().unwrap();
    assert_eq!(commissioned.len(), 1);
    assert_eq!(commissioned[0].node_id, 100);
}
