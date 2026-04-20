#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ExternalLightHubIntegration, HubType};

#[test]
fn scenario_pair_queues_unassigned_device() {
    let rig = harness::connect_rig(None);
    harness::store_commissioning_wifi(&rig.state, "RhythmNet", "secret");

    let session = rhythm_matter::desktop_lifecycle::INTEGRATION
        .start_pairing(
            &rig.state,
            &serde_json::json!({
                "setup_payload": "3497-011-2332",
                "network": "wifi",
                "rendezvous": "on_network"
            }),
        )
        .unwrap();

    assert_eq!(session.status, rhythm_os::pairing::PairingStatus::Complete);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = rig.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-100")
        .expect("paired Matter device should be in canonical registry");

    assert!(canonical.room_id.is_none());
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
}
