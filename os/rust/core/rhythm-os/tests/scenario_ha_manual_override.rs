//! HA manual control must survive later adaptive ticks through composite routing.

use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Mutex,
};
use std::time::{Duration, Instant};

use anyhow::Result;
use rhythm_core::{
    runtime::{
        handle::RuntimeHandle, orchestrator::RhythmRuntime, registry::SimpleDeviceRegistry,
        scheduler::NoOpScheduler, time::MockTimeProvider, RuntimeConfig,
    },
    ButtonAction, CompositeController, InputEvent,
};
use rhythm_ha::{
    controller::HaLightController,
    events::translate_ws_event,
    ha_lifecycle::{connect_ha, HA_HUB_TYPE},
    hub_state::{HaEventRoutingCache, HaHubData},
    light::{HaLightCatalogEntry, HaLightIdentity, HaLightObservation},
    test_support::SpyHaTransport,
    transport::HaConnectionConfig,
};
use rhythm_os::{
    canonical::identity::{DiscoveredIdentity, HardwareId, HubKey},
    discovery::{DiscoveredDevice, DiscoveredRoom, HubDiscovery},
    event_loop::{handle_hub_event, process_work_item, MotionTimerState},
    hub::HubType,
    light_runtime::{
        register_light_runtime_modules, LightRuntimeModule, RHYTHM_ADAPTIVE_RUNTIME_ID,
    },
    registry::HubDeviceRegistry,
    room_sync::sync_from_hub_for_key,
    state::{AppState, SharedState, WorkItem},
};
use serde_json::json;

const LIGHTS: [&str; 2] = ["light.reading", "light.desk"];

fn proof(id: &str) -> HaLightIdentity {
    HaLightIdentity {
        scope: "test-installation".into(),
        registry_id: id.into(),
        unique_id: format!("unique-{id}"),
        platform: "test".into(),
        device_id: Some(format!("device-{id}")),
        config_entry_id: Some("test-entry".into()),
    }
}

struct Discovery {
    cache: Arc<Mutex<HaEventRoutingCache>>,
}
impl HubDiscovery for Discovery {
    fn discover_rooms(&self) -> Result<Vec<DiscoveredRoom>> {
        // Real HA discovery retains the area ID as its control ID, even though
        // only individual entities can honor independent manual overrides.
        Ok(vec![DiscoveredRoom {
            id: "area".into(),
            name: "Office".into(),
            grouped_light_id: "area".into(),
            device_ids: LIGHTS.map(str::to_owned).into(),
        }])
    }
    fn discover_devices(&self) -> Result<Vec<DiscoveredDevice>> {
        Ok(vec![])
    }
    fn discover_identities(&self) -> Result<Vec<DiscoveredIdentity>> {
        Ok(LIGHTS
            .iter()
            .map(|id| DiscoveredIdentity {
                native_id: (*id).into(),
                room_id: Some("area".into()),
                room_name: Some("Office".into()),
                name: (*id).into(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: vec![HardwareId::serial(&proof(id).fingerprint())],
                manufacturer: None,
                model: None,
            })
            .collect())
    }
    fn endpoint_capabilities(&self, id: &str) -> Option<serde_json::Value> {
        Some(json!({"ha_identity":proof(id)}))
    }
    fn endpoint_observation(&self, id: &str) -> Option<rhythm_os::hub::LightObservation> {
        self.cache
            .lock()
            .unwrap()
            .lights
            .get(id)
            .map(|entry| entry.observation.normalized())
    }
}

struct Fixture {
    state: SharedState,
    runtime: Arc<dyn RuntimeHandle>,
    cache: Arc<Mutex<HaEventRoutingCache>>,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    spy: Arc<SpyHaTransport>,
    pending: Arc<AtomicUsize>,
    key: HubKey,
    room: String,
    nodes: [String; 2],
    _data: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        Self::with_selection(&LIGHTS, HA_HUB_TYPE)
    }

    fn with_selection(selected: &[&str], hub_type: &str) -> Self {
        let data = tempfile::tempdir().unwrap();
        let mut app = AppState {
            platform_context: "ha_addon",
            light_breaker_enabled: true,
            managed_ha_lights: Some(selected.iter().map(|id| (*id).to_owned()).collect()),
            data_dir: data.path().to_str().unwrap().into(),
            ..AppState::default()
        };
        register_light_runtime_modules(
            &mut app,
            [LightRuntimeModule::ephemeral(
                RHYTHM_ADAPTIVE_RUNTIME_ID,
                &["rhythm", "rhythm_adaptive"],
                rhythm_adaptive::runtime_manifest,
                |runtime| Box::new(rhythm_adaptive::RuntimeHandleAdaptiveRuntime::new(runtime)),
            )],
        )
        .unwrap();
        let state = Arc::new(Mutex::new(app));
        let key = HubKey::new(HubType::new(hub_type), "test-installation");
        let (mut hub, _) = connect_ha(
            &state,
            key.clone(),
            HaConnectionConfig {
                host: "test-installation".into(),
                port: 8123,
                token: "test".into(),
                use_ssl: false,
            },
            None,
            |_, _, _, _| std::sync::mpsc::channel().1,
        )
        .unwrap();
        let ha = hub.data::<HaHubData>().unwrap();
        let registry = ha.registry.clone();
        let cache = ha.event_routing_cache.clone();
        {
            let mut cache = cache.lock().unwrap();
            cache.lights_ready = true;
            cache.stream_ready = true;
            for id in LIGHTS {
                if selected.contains(&id) {
                    cache.reviewed.insert(id.into(), proof(id));
                }
                cache.lights.insert(id.into(), HaLightCatalogEntry {
                    identity: Some(proof(id)), name: id.into(), area_id: Some("area".into()),
                    capabilities: rhythm_ha::light::capabilities(&json!({"supported_color_modes":["color_temp"]})),
                    observation: HaLightObservation::parse(&json!({"state":if selected.contains(&id) {"on"} else {"unavailable"}, "attributes":{"brightness":196}, "last_updated":"2026-01-01T12:00:00Z"})),
                });
            }
        }
        let spy = Arc::new(SpyHaTransport::new());
        for id in LIGHTS {
            spy.set_entity_state(id, "on");
        }
        let composite = Arc::new(CompositeController::new());
        let pending = Arc::new(AtomicUsize::new(0));
        let queued = pending.clone();
        composite.set_queued_listener(Arc::new(move |_| {
            queued.fetch_add(1, Ordering::SeqCst);
        }));
        let completed = pending.clone();
        composite.set_outcome_listener(Arc::new(move |outcome| {
            assert!(
                outcome.status.is_success(),
                "HA dispatch failed: {outcome:?}"
            );
            completed.fetch_sub(1, Ordering::SeqCst);
        }));
        composite.register_controller(
            &key.to_string(),
            Arc::new(
                HaLightController::new(spy.clone(), registry.clone())
                    .with_capability_source(state.clone(), key.clone()),
            ),
        );
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RhythmRuntime::new(
            composite.clone(),
            MockTimeProvider::new(14.0, 172, 2026),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        hub.runtime = Some(runtime.clone());
        hub.discovery = Some(Arc::new(Discovery {
            cache: cache.clone(),
        }));
        {
            let mut app = state.lock().unwrap();
            app.composite_controller = Some(composite);
            app.hubs.insert(key.clone(), hub);
        }
        sync_from_hub_for_key(&state, &key, true).unwrap();
        let (room, nodes) = {
            let app = state.lock().unwrap();
            let nodes = LIGHTS.map(|id| {
                app.canonical_registry
                    .find_by_native_id(&key, id)
                    .unwrap()
                    .id
                    .clone()
            });
            (
                app.topology
                    .device_parent_room_id(&nodes[0])
                    .unwrap()
                    .to_owned(),
                nodes,
            )
        };
        runtime
            .handle_event(&InputEvent::new(&room, ButtonAction::RhythmOn))
            .unwrap();
        assert_eq!(
            spy.call_count(),
            0,
            "enabling adaptation alone sends no command"
        );
        assert_eq!(pending.load(Ordering::SeqCst), 0);
        Self {
            state,
            runtime,
            cache,
            registry,
            spy,
            pending,
            key,
            room,
            nodes,
            _data: data,
        }
    }

    fn observe(&self, id: &str, context: &str, brightness: u8, second: u8) {
        self.observe_power(id, context, "on", brightness, second);
    }

    fn observe_power(&self, id: &str, context: &str, power: &str, brightness: u8, second: u8) {
        let data = json!({"entity_id": id, "new_state": {"state":power, "attributes":{"brightness":brightness},
            "last_updated":format!("2026-01-01T12:00:{second:02}Z"),
            "context":{"id":context,"user_id":"synthetic-user","parent_id":null}}});
        for event in translate_ws_event("state_changed", &data, &self.registry, &self.cache) {
            handle_hub_event(
                &self.state,
                event.with_hub_key(self.key.clone()),
                &mut MotionTimerState::default(),
            );
        }
    }

    fn tick(&self, hour: f32) {
        let (nodes, generation) = {
            let app = self.state.lock().unwrap();
            (
                app.topology.periodic_light_nodes(&app.canonical_registry),
                app.light_dispatch_generation,
            )
        };
        for node in nodes {
            process_work_item(
                &self.state,
                WorkItem::PeriodicNodeTick {
                    command_id: "test-periodic".into(),
                    node_id: node.id,
                    settings_node_id: node.source_node_id,
                    current_hour: hour,
                    emit_parent_node_id: Some(node.emit_node_id),
                    dispatch_spacing: Duration::ZERO,
                    dispatch_generation: generation,
                },
            );
        }
        self.settle();
    }

    fn settle(&self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while self.pending.load(Ordering::SeqCst) != 0 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(
            self.pending.load(Ordering::SeqCst),
            0,
            "all transport work completed"
        );
    }

    fn targets(&self) -> Vec<String> {
        self.spy
            .calls_for_service("turn_on")
            .iter()
            .flat_map(|call| {
                call.data["entity_id"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .map(|id| id.as_str().unwrap().to_owned())
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

#[test]
fn no_output_actions_preserve_observed_power_and_send_no_device_command() {
    for power in ["snapshot", "on", "off"] {
        for target_room in [false, true] {
            let f = Fixture::new();
            if power != "snapshot" {
                f.observe_power(LIGHTS[0], "physical-readback", power, 230, 1);
            }
            let target = if target_room { &f.room } else { &f.nodes[0] };
            let before = f.state.lock().unwrap().room_observed_power.clone();
            for action in ["rhythm_on", "rhythm_off"] {
                let response: serde_json::Value = serde_json::from_str(
                    &rhythm_os::commands::do_node_action(&f.state, target, action, false).unwrap(),
                )
                .unwrap();
                f.settle();
                assert_eq!(f.spy.call_count(), 0, "{action} must not write to HA");
                if let Some(prior) = before.get(target) {
                    assert_eq!(response["lights_on"], prior.lights_on);
                    assert_eq!(response["observed_power"]["source"], prior.source.as_str());
                    assert_eq!(
                        response["observed_power"]["observed_at_epoch_ms"],
                        prior.observed_at_epoch_ms
                    );
                }
                assert_eq!(
                    f.runtime.engine_node_snapshot(target).unwrap().rhythm_enabled,
                    action == "rhythm_on"
                );
                let state = f.state.lock().unwrap();
                assert_eq!(state.room_observed_power.len(), before.len());
                for (id, prior) in &before {
                    let after = &state.room_observed_power[id];
                    assert_eq!(
                        (after.lights_on, after.source, after.observed_at_epoch_ms),
                        (prior.lights_on, prior.source, prior.observed_at_epoch_ms),
                        "{action} must preserve the physical {power} observation for {id}"
                    );
                }
            }
        }
    }
}

#[test]
fn no_output_actions_do_not_invent_an_unknown_power_observation() {
    let f = Fixture::new();
    f.state.lock().unwrap().room_observed_power.clear();
    for action in ["rhythm_on", "rhythm_off"] {
        rhythm_os::commands::do_node_action(&f.state, &f.nodes[0], action, false).unwrap();
        f.settle();
        assert_eq!(f.spy.call_count(), 0);
        assert!(
            f.state.lock().unwrap().room_observed_power.is_empty(),
            "{action} cannot manufacture a power observation without a light command"
        );
    }
}

#[test]
fn power_actions_still_record_the_accepted_on_and_off_command() {
    let f = Fixture::new();
    for (action, service, lights_on) in [("off", "turn_off", false), ("on", "turn_on", true)] {
        f.spy.reset();
        rhythm_os::commands::do_node_action(&f.state, &f.nodes[0], action, false).unwrap();
        f.settle();
        let calls = f.spy.calls_for_service(service);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].data["entity_id"], json!([LIGHTS[0]]));
        let state = f.state.lock().unwrap();
        let observed = &state.room_observed_power[&f.nodes[0]];
        assert_eq!(observed.lights_on, lights_on);
        assert_eq!(
            observed.source,
            rhythm_os::state::ObservedPowerSource::Command
        );
    }
}

#[test]
fn manual_override_survives_later_periodic_tick_while_sibling_adapts() {
    let f = Fixture::new();
    f.observe(LIGHTS[0], "manual-change", 194, 1);
    assert!(
        !f.runtime
            .engine_node_snapshot(&f.nodes[0])
            .unwrap()
            .rhythm_enabled
    );
    assert!(
        f.runtime
            .engine_node_snapshot(&f.room)
            .unwrap()
            .rhythm_enabled
    );
    assert!(
        f.runtime
            .engine_node_snapshot(&f.nodes[1])
            .unwrap()
            .rhythm_enabled
    );

    // The next periodic tick is eleven seconds after the user's change, with
    // no in-flight command race. It must not reapply the area over the child.
    f.tick(14.0 + 11.0 / 3600.0);
    assert_eq!(
        f.targets(),
        vec![LIGHTS[1]],
        "manual light must retain its level while sibling adapts"
    );
    assert_eq!(
        f.state.lock().unwrap().light_observations[&f.nodes[0]].brightness,
        Some(76)
    );

    // Reconciliation rebuilds routing from the same persisted area metadata;
    // it must retain the manual pause rather than restore area-wide dispatch.
    sync_from_hub_for_key(&f.state, &f.key, true).unwrap();
    f.spy.reset();
    f.tick(14.25);
    assert_eq!(f.targets(), vec![LIGHTS[1]]);

    // Explicitly resuming just that light restores adaptation without changing
    // the sibling's mode or requiring a room-wide reset.
    f.runtime
        .handle_event(&InputEvent::new(&f.nodes[0], ButtonAction::RhythmOn))
        .unwrap();
    f.spy.reset();
    f.tick(14.5);
    let mut targets = f.targets();
    targets.sort();
    let mut expected = LIGHTS.map(str::to_owned).to_vec();
    expected.sort();
    assert_eq!(targets, expected);
}

#[test]
fn correlated_echo_keeps_both_lights_adapting_and_child_on_is_scoped() {
    let f = Fixture::new();
    f.cache
        .lock()
        .unwrap()
        .own_contexts
        .push_back("own-command".into());
    f.observe(LIGHTS[0], "own-command", 194, 1);
    assert!(
        f.runtime
            .engine_node_snapshot(&f.nodes[0])
            .unwrap()
            .rhythm_enabled
    );
    f.tick(14.0);
    assert_eq!(f.targets().len(), 2);
    f.spy.reset();
    rhythm_os::commands::do_node_action(&f.state, &f.nodes[0], "on", false).unwrap();
    f.settle();
    assert_eq!(
        f.targets(),
        vec![LIGHTS[0]],
        "explicit child action must not target its whole HA area"
    );
}

#[test]
fn one_reviewed_light_stays_manual_across_two_changed_periodic_cycles() {
    // Production discovery uses "homeassistant"; retain compatibility with old
    // "ha" keys without renaming persisted endpoints. The unavailable sibling
    // remains in the discovered area but has never been selected for control.
    for hub_type in [HA_HUB_TYPE, HubType::HA] {
        let control = Fixture::with_selection(&LIGHTS[..1], hub_type);
        let hours = [18.0, 18.05, 18.10];
        for hour in hours {
            control.tick(hour);
        }
        let calls = control.spy.calls_for_service("turn_on");
        assert_eq!(control.targets(), vec![LIGHTS[0]; 3]);
        let kelvins: Vec<_> = calls
            .iter()
            .map(|call| call.data["color_temp_kelvin"].as_u64().unwrap())
            .collect();
        assert!(kelvins.windows(2).all(|pair| pair[0].abs_diff(pair[1]) >= 2),
            "the control cycles must change color enough to exercise dispatch, not dedup: {kelvins:?}");

        let manual = Fixture::with_selection(&LIGHTS[..1], hub_type);
        manual.tick(hours[0]);
        assert_eq!(manual.targets(), vec![LIGHTS[0]]);
        manual.observe(LIGHTS[0], "manual-after-first-cycle", 227, 1);
        assert!(
            !manual
                .runtime
                .engine_node_snapshot(&manual.nodes[0])
                .unwrap()
                .rhythm_enabled
        );
        assert!(
            manual
                .runtime
                .engine_node_snapshot(&manual.room)
                .unwrap()
                .rhythm_enabled
        );
        manual.spy.reset();
        for hour in &hours[1..] {
            manual.tick(*hour);
            assert!(manual.spy.calls().is_empty(),
                "paused selected light must receive no area or entity command on either later cycle ({hub_type}, {hour})");
            assert_eq!(
                manual.state.lock().unwrap().light_observations[&manual.nodes[0]].brightness,
                Some(89)
            );
        }
        manual
            .runtime
            .handle_event(&InputEvent::new(&manual.nodes[0], ButtonAction::RhythmOn))
            .unwrap();
        manual.tick(18.15);
        assert_eq!(
            manual.targets(),
            vec![LIGHTS[0]],
            "explicit resume restores only the reviewed light"
        );
    }
}
