#![cfg(feature = "test-support")]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::hub::HubType;

#[test]
fn scenario_restart_roomless_device_preserves_single_unassigned_review() {
    let initial = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    rhythm_os::room_sync::sync_all_hubs(&initial.state).unwrap();

    let restarted = harness::reconnect_rig_with_transport(initial.data_dir.clone(), |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    let report = rhythm_os::room_sync::sync_all_hubs(&restarted.state).unwrap();
    assert_eq!(report.rooms_added, 0);
    assert_eq!(report.devices_synced, 0);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = restarted.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-200")
        .expect("roomless Matter device should still exist after restart");

    assert_eq!(state.canonical_registry.device_count(), 1);
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::UnassignedDevice)
            .len(),
        1
    );
    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::UnassignedDevice)[0]
            .canonical_id
            .as_deref(),
        Some(canonical.id.as_str())
    );
}
