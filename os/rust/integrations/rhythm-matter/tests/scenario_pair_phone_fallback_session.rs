#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_matter::transport::{CommissionedDevice, MatterColorMode};
use rhythm_os::handlers::handle_pair_device;
use rhythm_os::hub::ExternalLightHubIntegration;
use rhythm_os::pairing::PairingRequest;
use std::sync::Arc;

#[test]
fn scenario_pair_phone_failure_allows_fallback_with_fresh_attempt_id() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "test-wifi", "test-password");
    rig.state.lock().unwrap().start_pairing_fn = Some(Arc::new(|state, _, params, context| {
        rhythm_matter::desktop_lifecycle::INTEGRATION
            .start_pairing_with_context(state, params, context)
    }));
    rig.transport
        .set_commission_result(Err(anyhow::anyhow!("Handoff timed out")));
    let phone = PairingRequest {
        hub_type: "matter".into(),
        session_id: Some("phone-attempt-1".into()),
        params: serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00", "network": "wifi",
            "rendezvous": "phone", "session_id": "phone-attempt-1",
            "handoff_address": "192.0.2.42", "handoff_port": 5540,
            "handoff_passcode": 20202021
        }),
    };
    let failed = handle_pair_device(&rig.state, &phone);
    assert_eq!(failed.status, 200);
    let failed_body: serde_json::Value = serde_json::from_str(&failed.body).unwrap();
    assert_eq!(failed_body["status"], "failed");
    let mut fallback = PairingRequest {
        hub_type: "matter".into(),
        session_id: phone.session_id.clone(),
        params: serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00", "network": "wifi",
            "rendezvous": "auto", "session_id": "phone-attempt-1"
        }),
    };
    assert_eq!(handle_pair_device(&rig.state, &fallback).status, 409);
    rig.transport.set_commission_result(Ok(CommissionedDevice {
        node_id: 99,
        vendor_name: "Test".into(),
        product_name: "Test bulb".into(),
        vendor_id: 0,
        product_id: 0,
        serial_number: None,
        light_endpoint: 1,
        color_modes: vec![MatterColorMode::ColorTemperature],
        min_kelvin: Some(2700),
        max_kelvin: Some(6500),
    }));
    fallback.session_id = Some("box-attempt-2".into());
    fallback.params["session_id"] = "box-attempt-2".into();
    let result = handle_pair_device(&rig.state, &fallback);
    assert_eq!(result.status, 200);
    let body: serde_json::Value = serde_json::from_str(&result.body).unwrap();
    assert_eq!(body["status"], "complete");
    assert_eq!(rig.transport.commission_requests().len(), 2);
    let recovery = rhythm_matter::setup_recovery::load_setup_payload(
        &rig.state,
        &rig.hub_data.fabric_id,
        body["device"]["device_id"].as_str().unwrap(),
    )
    .unwrap()
    .unwrap();
    assert_eq!(recovery.setup_payload, "MT:Y.K908OC16750648G00");
}
