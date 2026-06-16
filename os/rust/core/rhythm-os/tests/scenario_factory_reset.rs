//! Scenario regression: factory reset clears installation state, backup state,
//! and triage memory instead of only restoring profile/settings defaults.

mod harness;

use harness::{light, room, rooms_with_lights, TestHarness};
use rhythm_os::commands;

#[test]
fn factory_reset_clears_backup_installation_state() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen"), ("office", "Office")]);
    let harness = TestHarness::new().with_discovery(rooms, devices);
    harness.sync();

    let before = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    assert!(!before.installation.rooms.is_empty());
    assert!(before.installation.topology.room_count() > 0);
    assert!(before.installation.canonical_registry.device_count() > 0);

    commands::do_factory_reset(&harness.state).unwrap();

    let after = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    assert!(after.installation.rooms.is_empty());
    assert_eq!(after.installation.topology.room_count(), 0);
    assert_eq!(after.installation.canonical_registry.device_count(), 0);
    assert!(after.installation.hub_credentials.is_empty());
}

#[test]
fn factory_reset_clears_triage_queue() {
    let harness = TestHarness::new().with_discovery(
        vec![room("kitchen", "Kitchen")],
        vec![light("matter-100", "")],
    );
    harness.sync();

    assert!(
        harness.triage_pending_count() > 0,
        "roomless device should create pending triage before reset"
    );

    commands::do_factory_reset(&harness.state).unwrap();

    assert_eq!(harness.triage_pending_count(), 0);
    assert_eq!(harness.topology_room_count(), 0);
    assert_eq!(
        harness
            .state
            .lock()
            .unwrap()
            .canonical_registry
            .device_count(),
        0
    );
}
