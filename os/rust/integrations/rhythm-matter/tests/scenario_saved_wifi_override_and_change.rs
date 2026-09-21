#![cfg(feature = "test-support")]
#[path = "common/mod.rs"]
mod harness;
use rhythm_os::hub::ExternalLightHubIntegration;
use rhythm_os::{wifi_change::WifiChangeCode, wifi_profiles};
use serde_json::{json, Value};
#[test]
fn saved_override_and_network_change_preserve_default_and_canonical_identity() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "FixtureDefault", "default-password");
    let catalog = wifi_profiles::handle_update(
        &rig.state,
        &json!({"revision":0,"action":"save","ssid":"FixtureAlternate","password":"alternate-password","correlation_id":"12345678-1234-4234-8234-123456789abc"}),
    );
    assert_eq!(catalog.status, 200);
    let catalog: Value = serde_json::from_str(&catalog.body).unwrap();
    let id = catalog["profiles"][1]["id"].as_str().unwrap();
    // An explicit override must never be silently ignored by phone/on-network intake.
    assert!(rhythm_matter::commissioning::MatterPairingParams::from_value(
        &json!({"setup_payload":"MT:Y.K908OC16750648G00","rendezvous":"on_network","wifi_profile_id":id})
    ).is_err());
    let session = rhythm_matter::desktop_lifecycle::INTEGRATION.start_pairing(&rig.state,&json!({"setup_payload":"MT:Y.K908OC16750648G00","network":"wifi","rendezvous":"ble","wifi_profile_id":id})).unwrap();
    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);
    assert_eq!(
        rig.transport.commission_requests()[0].wifi_credentials.ssid,
        "FixtureAlternate"
    );
    assert_eq!(
        rhythm_os::provisioning::load_accessory_wifi_credentials(&rig.state)
            .unwrap()
            .unwrap()
            .ssid,
        "FixtureDefault"
    );
    let native = session.device.unwrap().device_id;
    let before = {
        let guard = rig.state.lock().unwrap();
        serde_json::to_value(&guard.canonical_registry).unwrap()
    };
    let wifi = wifi_profiles::selected_credentials(&rig.state, id).unwrap();
    let result = rhythm_matter::desktop_lifecycle::INTEGRATION
        .change_wifi(&rig.state, &native, &wifi, u64::MAX)
        .unwrap();
    assert_eq!(result.code, WifiChangeCode::Succeeded);
    let after = {
        let guard = rig.state.lock().unwrap();
        serde_json::to_value(&guard.canonical_registry).unwrap()
    };
    assert_eq!(before, after);
    assert!(rig.transport.decommissioned().is_empty());
    assert_eq!(rig.transport.commission_requests().len(), 1);
    let unknown = rhythm_matter::desktop_lifecycle::INTEGRATION
        .change_wifi(&rig.state, "matter-999999", &wifi, u64::MAX)
        .unwrap();
    assert_eq!(unknown.code, WifiChangeCode::Unsupported);
}
