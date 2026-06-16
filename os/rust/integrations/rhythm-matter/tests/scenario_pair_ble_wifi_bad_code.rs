#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use anyhow::anyhow;
use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_pair_ble_wifi_bad_code() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");
    rig.transport
        .set_commission_result(Err(anyhow!("invalid setup payload")));

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "bad-code",
                "network": "wifi",
                "rendezvous": "ble"
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Failed);
    assert!(session.error.unwrap().contains("invalid setup payload"));
}
