//! Home Assistant entity IDs are routes; registry proofs own canonical identity.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_ha::light::HaLightIdentity;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId, HubKey};
use rhythm_os::canonical::triage::TriageKind;
use rhythm_os::discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery};
use rhythm_os::hub::{ActiveHub, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::room_sync::sync_from_hub_for_key;
use rhythm_os::state::{AppState, SharedState};

struct SnapshotDiscovery(Vec<(&'static str, &'static str)>);

fn proof(id: &str) -> HaLightIdentity {
    HaLightIdentity {
        scope: "synthetic-installation".to_string(),
        registry_id: format!("registry-{id}"),
        unique_id: format!("unique-{id}"),
        platform: "test_light".to_string(),
        device_id: Some(format!("device-{id}")),
        config_entry_id: Some("test-entry".to_string()),
    }
}

impl HubDiscovery for SnapshotDiscovery {
    fn requires_complete_snapshot(&self) -> bool {
        true
    }

    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        Ok(vec![DiscoveredRoom {
            id: "area".to_string(),
            name: "Living Room".to_string(),
            grouped_light_id: "area".to_string(),
            device_ids: self.0.iter().map(|(route, _)| route.to_string()).collect(),
        }])
    }

    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(Vec::new())
    }

    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        Ok(self
            .0
            .iter()
            .map(|(route, id)| DiscoveredIdentity {
                native_id: route.to_string(),
                room_id: Some("area".to_string()),
                room_name: Some("Living Room".to_string()),
                name: format!("Light {id}"),
                device_type: DeviceType::Light,
                hardware_ids: vec![HardwareId::serial(&proof(id).fingerprint())],
                manufacturer: None,
                model: None,
            })
            .collect())
    }

    fn endpoint_capabilities(&self, native_id: &str) -> Option<serde_json::Value> {
        self.0
            .iter()
            .find(|(route, _)| *route == native_id)
            .map(|(_, id)| serde_json::json!({"ha_identity": proof(id)}))
    }
}

fn install() -> (SharedState, HubKey, String, String, String) {
    let key = HubKey::new(HubType::new(HubType::HA), "synthetic-ha");
    let mut app = AppState::default();
    let registry: Arc<Mutex<dyn rhythm_core::HubRegistry>> =
        Arc::new(Mutex::new(HubDeviceRegistry::with_options(false)));
    app.hubs.insert(
        key.clone(),
        ActiveHub {
            hub_type: key.hub_type.clone(),
            hub_key: key.clone(),
            runtime: None,
            hub_data: Box::new(()),
            registry: Some(registry),
            discovery: None,
            shutdown: Default::default(),
        },
    );
    let state = Arc::new(Mutex::new(app));
    sync(&state, &key, vec![("light.a", "a"), ("light.b", "b")]);
    let (a, b, room) = {
        let mut app = state.lock().unwrap();
        let a = app
            .canonical_registry
            .find_by_native_id(&key, "light.a")
            .unwrap()
            .id
            .clone();
        let b = app
            .canonical_registry
            .find_by_native_id(&key, "light.b")
            .unwrap()
            .id
            .clone();
        assert_ne!(a, b);
        assert!(app
            .canonical_registry
            .rename_device_by_user(&a, "Reading lamp"));
        assert!(app
            .canonical_registry
            .rename_device_by_user(&b, "Desk lamp"));
        assert!(app
            .canonical_registry
            .set_preferred_endpoint(&a, &key, "light.a"));
        assert!(app
            .canonical_registry
            .set_preferred_endpoint(&b, &key, "light.b"));
        let room = app.topology.device_parent_room_id(&a).unwrap().to_string();
        (a, b, room)
    };
    (state, key, a, b, room)
}

fn sync(state: &SharedState, key: &HubKey, entries: Vec<(&'static str, &'static str)>) {
    state.lock().unwrap().hubs.get_mut(key).unwrap().discovery =
        Some(Arc::new(SnapshotDiscovery(entries)));
    sync_from_hub_for_key(state, key, true).expect("complete HA discovery should synchronize");
}

fn assert_preserved(
    state: &SharedState,
    key: &HubKey,
    route: &str,
    canonical_id: &str,
    name: &str,
    room: &str,
) {
    let app = state.lock().unwrap();
    let device = app
        .canonical_registry
        .find_by_native_id(key, route)
        .expect("new entity route should address the existing physical device");
    assert_eq!(
        device.id, canonical_id,
        "route {route} must retain proven identity"
    );
    assert_eq!(
        device.name, name,
        "user-owned names must follow physical identity"
    );
    assert_eq!(device.room_id.as_deref(), Some(room));
    let endpoint = device
        .preferred_endpoint()
        .expect("preferred route remains available");
    assert_eq!(&endpoint.hub_key, key);
    assert_eq!(endpoint.native_id, route);
    assert!(
        endpoint.preferred,
        "endpoint preference must follow a renamed route"
    );
    assert_eq!(app.topology.device_parent_room_id(canonical_id), Some(room));
    assert_eq!(
        app.topology.get_device_node(canonical_id).unwrap().id,
        canonical_id
    );
}

#[test]
fn replacement_and_rename_preserve_proven_identity_in_either_discovery_order() {
    for reverse in [false, true] {
        let (state, key, a, b, room) = install();
        let mut next = vec![("light.a", "c"), ("light.b", "b"), ("light.z", "a")];
        if reverse {
            next.reverse();
        }
        for _ in 0..2 {
            sync(&state, &key, next.clone());
            assert_preserved(&state, &key, "light.z", &a, "Reading lamp", &room);
            assert_preserved(&state, &key, "light.b", &b, "Desk lamp", &room);
            let app = state.lock().unwrap();
            let replacement = app
                .canonical_registry
                .find_by_native_id(&key, "light.a")
                .unwrap();
            assert_ne!(
                replacement.id, a,
                "replacement cannot inherit the former route's identity"
            );
            assert_ne!(replacement.id, b);
            assert_eq!(replacement.name, "Light c");
            assert_eq!(app.canonical_registry.devices().count(), 3);
            assert!(
                app.canonical_registry
                    .triage()
                    .pending_by_kind(TriageKind::DeviceMerge)
                    .is_empty(),
                "exact same-hub proof must not become a new silo awaiting a merge"
            );
        }
    }
}

#[test]
fn swapped_entity_routes_preserve_both_canonical_devices_in_either_order() {
    for reverse in [false, true] {
        let (state, key, a, b, room) = install();
        let mut next = vec![("light.a", "b"), ("light.b", "a")];
        if reverse {
            next.reverse();
        }
        for _ in 0..2 {
            sync(&state, &key, next.clone());
            assert_preserved(&state, &key, "light.b", &a, "Reading lamp", &room);
            assert_preserved(&state, &key, "light.a", &b, "Desk lamp", &room);
            let app = state.lock().unwrap();
            assert_eq!(app.canonical_registry.devices().count(), 2);
            assert!(app
                .canonical_registry
                .triage()
                .pending_by_kind(TriageKind::DeviceMerge)
                .is_empty());
        }
    }
}
