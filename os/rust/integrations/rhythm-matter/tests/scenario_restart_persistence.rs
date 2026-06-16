#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_matter::transport::MatterTransport;
use rhythm_os::hub::ExternalLightHubIntegration;

#[test]
fn scenario_restart_persistence() {
    let initial = harness::connect_rig(None);
    harness::store_commissioning_wifi(&initial.state, "RhythmNet", "secret");

    let first_session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &initial.state,
            &serde_json::json!({
                "setup_payload": "MT:Y.K908OC16750648G00",
                "network": "wifi",
                "rendezvous": "auto"
            }),
        )
        .unwrap();
    assert_eq!(first_session.device.unwrap().device_id, "matter-100");

    let prior_devices = initial.transport.list_devices().unwrap();
    let restarted =
        harness::connect_rig_with_transport(Some(initial.data_dir.clone()), |transport| {
            for device in &prior_devices {
                transport.add_device(device.node_id, &device.vendor_name, &device.product_name);
            }
        });
    harness::store_commissioning_wifi(&restarted.state, "RhythmNet", "secret");

    let cached_caps = restarted.hub_data.device_caps.lock().unwrap();
    let restored_caps = cached_caps
        .get("matter-100")
        .expect("restarted Matter hub should warm capability cache");
    assert!(restored_caps.supports_color_temp());
    assert!(!restored_caps.supports_xy_color());
    drop(cached_caps);

    let second_session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &restarted.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi",
                "rendezvous": "on_network"
            }),
        )
        .unwrap();

    assert_eq!(
        second_session.status,
        rhythm_os::pairing::PairingStatus::Complete
    );
    assert_eq!(second_session.device.unwrap().device_id, "matter-101");
    assert_eq!(restarted.hub_data.reserve_node_id(), 102);
}
