//! Scenario regression: hub-configured conflicts should be queued once and auto-resolve cleanly.

mod harness;

use std::sync::Arc;

use anyhow::Result;
use harness::TestHarness;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::DiscoveredIdentity;
use rhythm_os::canonical::triage::{TriageKind, TriageStatus};
use rhythm_os::commands;
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};

#[allow(clippy::type_complexity)]
struct ConfiguredDiscovery {
    rooms: Vec<(String, String, String, Vec<String>)>,
    devices: Vec<(String, String, DeviceType)>,
    configured_devices: Vec<(String, String)>,
}

impl ConfiguredDiscovery {
    fn kitchen_light(configured_devices: Vec<(String, String)>) -> Self {
        Self {
            rooms: vec![(
                "kitchen".to_string(),
                "Kitchen".to_string(),
                "kitchen_grouped".to_string(),
                vec!["light-kitchen".to_string()],
            )],
            devices: vec![(
                "light-kitchen".to_string(),
                "kitchen".to_string(),
                DeviceType::Light,
            )],
            configured_devices,
        }
    }
}

impl HubDiscovery for ConfiguredDiscovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(self
            .rooms
            .iter()
            .map(|(id, name, grouped_light_id, device_ids)| DiscoveredRoom {
                id: id.clone(),
                name: name.clone(),
                grouped_light_id: grouped_light_id.clone(),
                device_ids: device_ids.clone(),
            })
            .collect())
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(self
            .devices
            .iter()
            .map(|(device_id, room_id, device_type)| DiscoveredDevice {
                device_id: device_id.clone(),
                room_id: room_id.clone(),
                buttons: vec![],
                device_type: device_type.clone(),
            })
            .collect())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        Ok(self
            .devices
            .iter()
            .map(|(device_id, room_id, device_type)| DiscoveredIdentity {
                native_id: device_id.clone(),
                room_id: room_id.clone(),
                room_name: "Kitchen".to_string(),
                name: device_id.clone(),
                device_type: device_type.clone(),
                hardware_ids: vec![],
                manufacturer: None,
                model: None,
            })
            .collect())
    }

    fn discover_configured_devices(&self) -> Result<Vec<(String, String)>> {
        Ok(self.configured_devices.clone())
    }
}

fn install_primary_discovery(harness: &TestHarness, discovery: ConfiguredDiscovery) {
    let mut state = harness.state.lock().unwrap();
    state
        .hubs
        .get_mut(&harness.hub_key)
        .expect("primary hub should exist")
        .discovery = Some(Arc::new(discovery));
}

#[test]
fn hub_configured_triage_is_queued_once_across_repeated_syncs() {
    let harness = TestHarness::new();
    install_primary_discovery(
        &harness,
        ConfiguredDiscovery::kitchen_light(vec![(
            "behavior-1".to_string(),
            "light-kitchen".to_string(),
        )]),
    );

    harness.sync();
    harness.sync();

    let state = harness.state.lock().unwrap();
    let hub_configured_entries: Vec<_> = state
        .canonical_registry
        .triage()
        .all()
        .iter()
        .filter(|entry| entry.kind == TriageKind::HubConfigured)
        .collect();

    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_hub_configured_count(),
        1
    );
    assert_eq!(hub_configured_entries.len(), 1);
    assert_eq!(
        hub_configured_entries[0].discovered.native_id,
        "light-kitchen"
    );
}

#[test]
fn backup_restore_preserves_pending_hub_configured_triage() {
    let harness = TestHarness::new();
    install_primary_discovery(
        &harness,
        ConfiguredDiscovery::kitchen_light(vec![(
            "behavior-1".to_string(),
            "light-kitchen".to_string(),
        )]),
    );

    harness.sync();

    let bundle = commands::build_backup_bundle_dto(&harness.state, false).unwrap();
    let restored = TestHarness::new();
    commands::do_backup_restore(&restored.state, bundle).unwrap();

    let state = restored.state.lock().unwrap();
    let hub_configured_entries: Vec<_> = state
        .canonical_registry
        .triage()
        .all()
        .iter()
        .filter(|entry| entry.kind == TriageKind::HubConfigured)
        .collect();

    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_hub_configured_count(),
        1
    );
    assert_eq!(hub_configured_entries.len(), 1);
    assert_eq!(
        hub_configured_entries[0].discovered.native_id,
        "light-kitchen"
    );
}

#[test]
fn hub_configured_triage_auto_resolves_when_native_automation_disappears() {
    let harness = TestHarness::new();
    install_primary_discovery(
        &harness,
        ConfiguredDiscovery::kitchen_light(vec![(
            "behavior-1".to_string(),
            "light-kitchen".to_string(),
        )]),
    );

    harness.sync();

    install_primary_discovery(&harness, ConfiguredDiscovery::kitchen_light(vec![]));
    harness.sync();

    let state = harness.state.lock().unwrap();
    let hub_configured_entry = state
        .canonical_registry
        .triage()
        .all()
        .iter()
        .find(|entry| entry.kind == TriageKind::HubConfigured)
        .expect("hub-configured triage entry should still exist for audit history");

    assert_eq!(
        state
            .canonical_registry
            .triage()
            .pending_hub_configured_count(),
        0
    );
    assert_eq!(hub_configured_entry.status, TriageStatus::Confirmed);
    assert_eq!(hub_configured_entry.resolved_by.as_deref(), Some("auto"));
}
