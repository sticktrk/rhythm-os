//! Direct actions keep empty room settings usable without claiming light output.

mod harness;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use harness::TestHarness;
use rhythm_core::{
    runtime::{registry::SimpleDeviceRegistry, scheduler::NoOpScheduler, time::MockTimeProvider},
    CompositeController, HubDispatchTarget, HubLightController, LightControlResult,
    LightingCommand, RestoredNodeState, RhythmRuntime, Room, RuntimeConfig, RuntimeHandle,
};
use rhythm_os::{
    axum_router,
    canonical::{identity::DiscoveredIdentity, registry::ResolveResult},
    commands,
    storage::{FileStorage, Storage},
    topology::{DevicePlacement, HubRoomBinding, RoomDevice},
};
use serde_json::{json, Value};
use tower::ServiceExt;

#[derive(Default)]
struct RecordingHub(Mutex<Vec<HubDispatchTarget>>);

#[async_trait]
impl HubLightController for RecordingHub {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        _: LightingCommand,
    ) -> LightControlResult<()> {
        self.0.lock().unwrap().push(target.clone());
        Ok(())
    }
    async fn turn_off_target(
        &self,
        _: &HubDispatchTarget,
        _: Option<u32>,
    ) -> LightControlResult<()> {
        Ok(())
    }
    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(vec![])
    }
    async fn is_connected(&self) -> bool {
        true
    }
    async fn any_lights_on_target(&self, _: &HubDispatchTarget) -> LightControlResult<bool> {
        Ok(false)
    }
    fn name(&self) -> &str {
        "synthetic-hub"
    }
}

struct Fixture {
    harness: TestHarness,
    runtime: Arc<dyn RuntimeHandle>,
    composite: Arc<CompositeController>,
    hub: Arc<RecordingHub>,
    room_id: String,
}

impl Fixture {
    fn new() -> Self {
        let harness = TestHarness::new();
        let composite = Arc::new(CompositeController::new());
        let hub = Arc::new(RecordingHub::default());
        composite.register_controller(&harness.hub_key.to_string(), hub.clone());
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RhythmRuntime::new(
            composite.clone(),
            MockTimeProvider::new(12.0, 172, 2026),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        {
            let mut app = harness.state.lock().unwrap();
            app.hubs.get_mut(&harness.hub_key).unwrap().runtime = Some(runtime.clone());
            app.composite_controller = Some(composite.clone());
        }
        let created: Value = serde_json::from_str(
            &commands::do_topology_create_room(&harness.state, "Empty room").unwrap(),
        )
        .unwrap();
        Self {
            harness,
            runtime,
            composite,
            hub,
            room_id: created["id"].as_str().unwrap().to_owned(),
        }
    }

    fn bind_room(&self) {
        let routing = {
            let mut app = self.harness.state.lock().unwrap();
            let identity = DiscoveredIdentity {
                native_id: "native-light".into(),
                room_id: Some("native-room".into()),
                room_name: Some("Populated room".into()),
                name: "Synthetic lamp".into(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: vec![],
                manufacturer: None,
                model: None,
            };
            let ResolveResult::Created { canonical_id } =
                app.canonical_registry
                    .resolve(&identity, &self.harness.hub_key, 1_000)
            else {
                panic!("fixture light must be new");
            };
            assert!(app
                .topology
                .attach_device_user_override(&self.room_id, &canonical_id));
            app.topology
                .get_mut(&self.room_id)
                .unwrap()
                .upsert_hub_room_binding(HubRoomBinding {
                    hub_key: self.harness.hub_key.clone(),
                    hub_room_id: "native-room".into(),
                    control_id: "native-group".into(),
                    light_device_ids: vec!["native-light".into()],
                });
            app.topology.composite_routing(&app.canonical_registry)
        };
        self.composite.update_routing(routing);
    }

    async fn action(&self, action: &str) -> (StatusCode, Value) {
        let response = axum_router::api_routes()
            .with_state(self.harness.state.clone())
            .oneshot(
                Request::builder()
                    .method("PUT")
                    .uri("/api/nodes/action")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        json!({"node_id": self.room_id, "action": action}).to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        let status = response.status();
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = serde_json::from_slice(&body)
            .unwrap_or_else(|_| Value::String(String::from_utf8(body.to_vec()).unwrap()));
        (status, body)
    }
}

#[tokio::test]
async fn empty_room_reset_updates_and_persists_settings_without_physical_output() {
    let fixture = Fixture::new();
    let dir = tempfile::tempdir().unwrap();
    let storage = Arc::new(FileStorage::new(dir.path().to_str().unwrap()).unwrap());
    fixture.harness.state.lock().unwrap().storage = Some(storage.clone());
    let before = fixture
        .runtime
        .engine_node_snapshot(&fixture.room_id)
        .unwrap();
    let mut restored = RestoredNodeState::from(&before);
    restored.time_offset_minutes = 90.0;
    restored.brightness_offset = -20.0;
    restored.hard_off = true;
    restored.rhythm_enabled = false;
    restored.standby_enabled = true;
    restored.profile_settings.motion_activation_enabled = Some(false);
    fixture
        .runtime
        .restore_node_state(&fixture.room_id, restored.clone());

    for _ in 0..2 {
        let (status, body) = fixture.action("reset").await;
        assert_eq!(status, StatusCode::OK, "{body}");
        let returned = &body["nodes"][0];
        assert_eq!(returned["id"], fixture.room_id);
        assert_eq!(returned["rhythm_enabled"], true);
        assert_eq!(returned["time_offset"].as_f64(), Some(0.0));
        assert_eq!(returned["brightness_offset"].as_f64(), Some(0.0));
        assert_eq!(returned["lights_on"], false);
        assert_ne!(returned["observed_power"]["source"], "command");
        let after = fixture
            .runtime
            .engine_node_snapshot(&fixture.room_id)
            .unwrap();
        assert_eq!(after.time_offset_minutes, 0.0);
        assert_eq!(after.brightness_offset, 0.0);
        assert!(after.rhythm_enabled);
        assert!(!after.hard_off && !after.soft_off && !after.mood_active);
        assert!(after.standby_enabled);
        assert_eq!(after.profile_settings, restored.profile_settings);
        assert!(
            !fixture
                .harness
                .state
                .lock()
                .unwrap()
                .room_observed_power
                .get(&fixture.room_id)
                .is_some_and(|power| power.lights_on),
            "a settings-only Reset must not fabricate an accepted light command"
        );
        let persisted = storage.load_rooms().unwrap();
        let room = persisted.get(&fixture.room_id).unwrap();
        assert_eq!(room.time_offset_minutes, 0.0);
        assert_eq!(room.brightness_offset, 0.0);
        assert!(room.rhythm_enabled && !room.hard_off);
        assert_eq!(room.profile_settings, restored.profile_settings);
    }
    assert!(fixture.hub.0.lock().unwrap().is_empty());

    // Rebuild the runtime node from its saved settings, as during startup.
    fixture.runtime.remove_room(&fixture.room_id);
    commands::reconcile_runtime_from_state(&fixture.harness.state).unwrap();
    let recovered = fixture
        .runtime
        .engine_node_snapshot(&fixture.room_id)
        .unwrap();
    assert_eq!(recovered.time_offset_minutes, 0.0);
    assert_eq!(recovered.brightness_offset, 0.0);
    assert!(recovered.rhythm_enabled && !recovered.hard_off);
    assert_eq!(recovered.profile_settings, restored.profile_settings);
    let backup = commands::build_backup_bundle_dto(&fixture.harness.state, false).unwrap();
    let exported = backup.installation.rooms.get(&fixture.room_id).unwrap();
    assert_eq!(exported.time_offset_minutes, 0.0);
    assert_eq!(exported.brightness_offset, 0.0);
    assert!(exported.rhythm_enabled && !exported.hard_off);
    assert_eq!(exported.profile_settings, restored.profile_settings);
    let topology = backup.installation.topology.get(&fixture.room_id).unwrap();
    assert!(!topology.has_devices() && !topology.has_bindings());

    // Reusing the same identity after attaching a physical binding must dispatch.
    fixture.bind_room();
    assert!(fixture.composite.has_active_route(&fixture.room_id));
    let (status, body) = fixture.action("reset").await;
    assert_eq!(status, StatusCode::OK, "{body}");
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while fixture.hub.0.lock().unwrap().is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        *fixture.hub.0.lock().unwrap(),
        vec![HubDispatchTarget::Group {
            room_id: "native-room".into(),
            control_id: "native-group".into(),
        }]
    );
}

#[tokio::test]
async fn empty_room_other_direct_actions_preserve_settings_and_do_not_query_lights() {
    let fixture = Fixture::new();
    // Toggle needs a power sample on a normal room. An empty room has no
    // physical power to query; its other actions must also stay settings-only.
    for action in [
        "toggle",
        "off",
        "on",
        "reset_to_mode_default",
        "rhythm_off",
        "rhythm_on",
    ] {
        let (status, body) = fixture.action(action).await;
        assert_eq!(status, StatusCode::OK, "{action}: {body}");
        assert!(
            fixture
                .harness
                .state
                .lock()
                .unwrap()
                .room_observed_power
                .get(&fixture.room_id)
                .is_none(),
            "{action} fabricated power evidence"
        );
    }
    assert!(fixture.hub.0.lock().unwrap().is_empty());
    let (status, _) = fixture.action("invalid-action").await;
    assert_ne!(status, StatusCode::OK);
}

#[tokio::test]
async fn missing_controller_for_bound_or_populated_room_is_not_a_successful_noop() {
    for bound in [false, true] {
        let fixture = Fixture::new();
        if bound {
            fixture.bind_room();
            fixture
                .harness
                .state
                .lock()
                .unwrap()
                .topology
                .get_mut(&fixture.room_id)
                .unwrap()
                .devices
                .clear();
            fixture.composite.update_routing(Default::default());
        } else {
            fixture
                .harness
                .state
                .lock()
                .unwrap()
                .topology
                .get_mut(&fixture.room_id)
                .unwrap()
                .devices
                .push(RoomDevice {
                    device_id: "synthetic-light".into(),
                    placement: DevicePlacement::UserOverride,
                });
        }
        let (status, body) = fixture.action("reset").await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR, "{body}");
        assert!(body.to_string().contains("No controllers"));
        assert!(fixture.hub.0.lock().unwrap().is_empty());
    }
}
