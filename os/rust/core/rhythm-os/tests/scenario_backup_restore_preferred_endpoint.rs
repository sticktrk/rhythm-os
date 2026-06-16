//! Scenario regression: backup/restore must preserve preferred canonical endpoints.

mod harness;

use harness::{light, room, TestHarness};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::commands;

fn restore_reconnected_merged_device() -> (TestHarness, HubKey, String) {
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

    let (merge_entry_id, canonical_id) = {
        let state = harness.state.lock().unwrap();
        let entry = state
            .canonical_registry
            .triage()
            .pending_by_kind(TriageKind::DeviceMerge)
            .into_iter()
            .next()
            .expect("device merge triage should exist");
        (
            entry.id.clone(),
            entry.candidate_matches[0].canonical_id.clone(),
        )
    };
    commands::do_triage_merge(&harness.state, &merge_entry_id, &canonical_id).unwrap();
    commands::do_canonical_set_preferred(
        &harness.state,
        &canonical_id,
        "ha",
        "192.168.1.200",
        "lamp-1",
    )
    .unwrap();

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

    (restored, restored_ha_key, canonical_id)
}

#[test]
fn backup_restore_preserves_preferred_endpoint_after_reconnect() {
    let (restored, ha_key, canonical_id) = restore_reconnected_merged_device();

    let state = restored.state.lock().unwrap();
    let device = state
        .canonical_registry
        .get(&canonical_id)
        .expect("merged canonical device should survive restore");
    let preferred = device
        .preferred_endpoint()
        .expect("preferred endpoint should survive restore");

    assert_eq!(device.active_endpoints().count(), 2);
    assert_eq!(preferred.hub_key, ha_key);
    assert_eq!(preferred.native_id, "lamp-1");
    assert!(preferred.preferred);
    assert!(
        device
            .endpoints
            .iter()
            .any(|endpoint| endpoint.hub_key == restored.hub_key && !endpoint.preferred),
        "non-preferred endpoint should remain connected without stealing preference"
    );
}
