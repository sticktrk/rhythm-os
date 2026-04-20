#![cfg(all(feature = "desktop", feature = "test-support"))]

#[path = "common/mod.rs"]
mod harness;

use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::HubType;

#[test]
fn scenario_sync_imports_roomless_commissioned_device() {
    let rig = harness::connect_rig_with_transport(None, |transport| {
        transport.add_device(200, "Vendor", "Lamp");
    });

    let report = rhythm_os::room_sync::sync_all_hubs(&rig.state).unwrap();
    assert_eq!(report.rooms_added, 0);
    assert_eq!(report.devices_synced, 0);

    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let state = rig.state.lock().unwrap();
    let canonical = state
        .canonical_registry
        .find_by_native_id(&hub_key, "matter-200")
        .expect("synced Matter device should be in canonical registry");

    assert_eq!(canonical.name, "Vendor Lamp");
    assert_eq!(canonical.manufacturer.as_deref(), Some("Vendor"));
    assert_eq!(canonical.model.as_deref(), Some("Lamp"));
    assert!(canonical.room_id.is_none());
    let canonical_id = canonical.id.clone();
    assert_eq!(
        state.canonical_registry.triage().pending_unassigned_count(),
        1
    );
    let runtime = state
        .hub_runtime()
        .expect("syncing roomless Matter devices should bootstrap the runtime");
    drop(state);

    let node = runtime
        .engine_node_snapshot(&canonical_id)
        .expect("roomless Matter canonical device should exist in the runtime");
    assert_eq!(node.parent_id, None);

    let caps = rig.hub_data.device_caps.lock().unwrap();
    let stored = caps
        .get("matter-200")
        .expect("synced Matter device should cache capabilities");
    assert!(stored.supports_color_temp());
    assert!(!stored.supports_xy_color());
}
