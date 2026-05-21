#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_ble_wifi_platform_fallback() {
    let rig = harness::connect_rig(None);
    harness::set_platform_commissioning_wifi_provider(&rig.state, "RhythmNet", "secret");

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

    let requests = rig.transport.commission_requests();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].wifi_credentials.ssid, "RhythmNet");

    let storage = rig.state.lock().unwrap();
    let stored = storage
        .storage
        .as_ref()
        .unwrap()
        .load_commissioning_wifi_credentials()
        .unwrap();
    assert_eq!(
        stored,
        Some(rhythm_os::provisioning::WifiCredentials {
            ssid: "RhythmNet".into(),
            password: "secret".into(),
        })
    );
}
