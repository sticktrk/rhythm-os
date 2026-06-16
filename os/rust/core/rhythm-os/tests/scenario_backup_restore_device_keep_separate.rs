//! Scenario regression: device-merge "keep separate" should survive restore without drift.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

#[test]
fn backup_restore_keep_separate_device_merge_stays_resolved_on_first_sync() {
    let harness = TestHarness::new().with_discovery(
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    harness.sync();

    let mut harness = harness;
    let ha_key = harness.add_hub("ha", "192.168.1.200");
    harness.set_hub_discovery(
        &ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );
    harness.sync_hub(&ha_key);

    let (room_entry_id, _, _) = harness
        .triage_room_binding(0)
        .expect("room binding triage should exist");
    harness.triage_bind(&room_entry_id).unwrap();

    let device_entry_id = {
        let state = harness.state.lock().unwrap();
        state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .into_iter()
            .next()
            .expect("device merge triage should exist")
            .id
            .clone()
    };
    let result = harness.triage_new(&device_entry_id).unwrap();
    assert!(
        result.contains("canonical_id"),
        "device merge keep-separate should resolve to a canonical device"
    );
    assert_eq!(
        harness
            .state
            .lock()
            .unwrap()
            .canonical_registry
            .device_count(),
        2,
        "keep-separate should preserve the existing silo device instead of creating drift"
    );
    assert_eq!(harness.triage_pending_device_count(), 0);

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let mut restored = restored;
    let primary_key = restored.add_hub("mock", "192.168.1.100");
    let restored_ha_key = restored.add_hub("ha", "192.168.1.200");
    restored.set_hub_discovery(
        &primary_key,
        vec![room("mock-kitchen", "Kitchen")],
        vec![light("lamp-1", "mock-kitchen")],
    );
    restored.set_hub_discovery(
        &restored_ha_key,
        vec![room("ha-kitchen", "Kitchen")],
        vec![light("lamp-1", "ha-kitchen")],
    );

    restored.sync_all();

    let state = restored.state.lock().unwrap();
    assert_eq!(state.canonical_registry.device_count(), 2);
    assert_eq!(state.canonical_registry.triage().pending_device_count(), 0);
}
