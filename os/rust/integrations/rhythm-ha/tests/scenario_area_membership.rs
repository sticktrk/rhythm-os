//! Scenario coverage for authoritative Home Assistant area assignment.

use anyhow::Result;
use async_trait::async_trait;
use rhythm_ha::area_membership::{
    reassign_entity_area_with_client, HaAreaRegistryClient, HaDeviceAreaEntry, HaEntityAreaEntry,
};

struct FakeRegistryClient {
    entity: HaEntityAreaEntry,
    devices: Vec<HaDeviceAreaEntry>,
    persist_target_update: bool,
    updates: Vec<Option<String>>,
}

impl FakeRegistryClient {
    fn new(persist_target_update: bool) -> Self {
        Self {
            entity: HaEntityAreaEntry {
                entity_id: "light.desk".to_string(),
                area_id: Some("nook".to_string()),
                device_id: Some("device-desk".to_string()),
            },
            devices: vec![HaDeviceAreaEntry {
                id: "device-desk".to_string(),
                area_id: Some("nook".to_string()),
            }],
            persist_target_update,
            updates: Vec::new(),
        }
    }
}

#[async_trait]
impl HaAreaRegistryClient for FakeRegistryClient {
    async fn area_exists(&mut self, area_id: &str) -> Result<bool> {
        Ok(matches!(area_id, "nook" | "office"))
    }

    async fn get_entity(&mut self, _entity_id: &str) -> Result<HaEntityAreaEntry> {
        Ok(self.entity.clone())
    }

    async fn list_devices(&mut self) -> Result<Vec<HaDeviceAreaEntry>> {
        Ok(self.devices.clone())
    }

    async fn update_entity_area(&mut self, _entity_id: &str, area_id: Option<&str>) -> Result<()> {
        self.updates.push(area_id.map(str::to_string));
        if self.persist_target_update || area_id == Some("nook") {
            self.entity.area_id = area_id.map(str::to_string);
        }
        Ok(())
    }
}

#[test]
fn moved_light_is_assigned_to_target_ha_area_before_success() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut client = FakeRegistryClient::new(true);

    runtime
        .block_on(reassign_entity_area_with_client(
            &mut client,
            "light.desk",
            Some("office"),
        ))
        .unwrap();

    assert_eq!(client.entity.area_id.as_deref(), Some("office"));
    assert_eq!(client.updates, vec![Some("office".to_string())]);
}

#[test]
fn verification_failure_restores_original_ha_area() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut client = FakeRegistryClient::new(false);

    let error = runtime
        .block_on(reassign_entity_area_with_client(
            &mut client,
            "light.desk",
            Some("office"),
        ))
        .expect_err("move should fail when HA does not persist the target area");

    assert!(error
        .to_string()
        .contains("Home Assistant did not persist area"));
    assert_eq!(client.entity.area_id.as_deref(), Some("nook"));
    assert_eq!(
        client.updates,
        vec![Some("office".to_string()), Some("nook".to_string())]
    );
}

#[test]
fn unassign_fails_when_the_entity_still_inherits_its_device_area() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let mut client = FakeRegistryClient::new(true);

    let error = runtime
        .block_on(reassign_entity_area_with_client(
            &mut client,
            "light.desk",
            None,
        ))
        .expect_err("device-level area inheritance must prevent false unassignment");

    assert!(error
        .to_string()
        .contains("Home Assistant did not persist area"));
    assert_eq!(client.entity.area_id.as_deref(), Some("nook"));
    assert_eq!(client.updates, vec![None, Some("nook".to_string())]);
}
