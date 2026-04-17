#![cfg(all(feature = "desktop", feature = "test-support"))]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_ble_wifi_missing_wifi() {
    let rig = harness::connect_rig(None);

    let error = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi",
                "rendezvous": "ble"
            }),
        )
        .unwrap_err();

    assert!(error
        .to_string()
        .contains("requires stored appliance Wi-Fi credentials"));
    assert!(rig.transport.commission_requests().is_empty());
}
