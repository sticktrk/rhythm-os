//! Real engine with fake HA I/O: ownership, observations and live invalidation.
use rhythm_core::runtime::{
    handle::RuntimeHandle, orchestrator::RhythmRuntime, registry::SimpleDeviceRegistry,
    scheduler::NoOpScheduler, time::MockTimeProvider, RuntimeConfig,
};
use rhythm_core::{ButtonAction, InputEvent, LightNodeKind};
use rhythm_ha::{
    controller::HaLightController,
    events::translate_ws_event,
    ha_lifecycle::connect_ha,
    hub_state::{HaEventRoutingCache, HaHubData},
    light::{HaLightCatalogEntry, HaLightIdentity, HaLightObservation, HaLightSelection},
    test_support::SpyHaTransport,
    transport::HaConnectionConfig,
};
use rhythm_os::{
    canonical::{
        identity::{DiscoveredIdentity, HardwareId, HubKey},
        registry::ResolveResult,
    },
    event_loop::{handle_hub_event, MotionTimerState},
    hub::{HubEvent, HubType},
    registry::HubDeviceRegistry,
    state::{AppState, SharedState},
};
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

fn proof(id: &str) -> HaLightIdentity {
    HaLightIdentity {
        scope: "ha-instance".into(),
        registry_id: id.into(),
        unique_id: format!("unique-{id}"),
        platform: "test".into(),
        device_id: Some(format!("device-{id}")),
        config_entry_id: Some("integration".into()),
    }
}
fn entry(id: &str) -> HaLightCatalogEntry {
    HaLightCatalogEntry {
        identity: Some(proof(id)),
        name: id.into(),
        area_id: Some("room".into()),
        capabilities: rhythm_ha::light::capabilities(
            &json!({"supported_color_modes":["color_temp"],"min_color_temp_kelvin":2200,"max_color_temp_kelvin":5000}),
        ),
        observation: HaLightObservation::parse(
            &json!({"state":"on", "attributes":{"brightness":128}, "last_updated":"2026-10-03T12:00:00+00:00"}),
        ),
    }
}
type CommandHook = Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>;
struct ContextTransport {
    spy: Arc<SpyHaTransport>,
    hook: CommandHook,
}
impl rhythm_ha::transport::HaTransport for ContextTransport {
    fn call_service(
        &self,
        domain: &str,
        service: &str,
        data: &serde_json::Value,
    ) -> anyhow::Result<()> {
        self.spy.call_service(domain, service, data)
    }
    fn call_service_contexts(
        &self,
        domain: &str,
        service: &str,
        data: &serde_json::Value,
    ) -> anyhow::Result<Vec<String>> {
        self.call_service(domain, service, data)?;
        if let Some(hook) = self.hook.lock().unwrap().as_ref() {
            hook();
        }
        Ok(vec!["rhythm-request".into()])
    }
    fn get_state(&self, id: &str) -> anyhow::Result<rhythm_ha::transport::EntityState> {
        self.spy.get_state(id)
    }
    fn get_states(&self) -> anyhow::Result<Vec<rhythm_ha::transport::EntityState>> {
        self.spy.get_states()
    }
    fn test_connection(&self) -> anyhow::Result<bool> {
        Ok(true)
    }
}
struct Fixture {
    state: SharedState,
    cache: Arc<Mutex<HaEventRoutingCache>>,
    registry: Arc<Mutex<HubDeviceRegistry>>,
    runtime: Arc<dyn RuntimeHandle>,
    spy: Arc<SpyHaTransport>,
    key: HubKey,
    node: String,
    hook: CommandHook,
}
impl Fixture {
    fn new() -> Self {
        let state = Arc::new(Mutex::new(AppState {
            platform_context: "ha_addon",
            managed_ha_lights: Some(["light.reviewed".into()].into()),
            ..AppState::default()
        }));
        let key = HubKey::new(HubType::new("homeassistant"), "ha-instance");
        let (mut hub, _rx) = connect_ha(
            &state,
            key.clone(),
            HaConnectionConfig {
                host: "ha-instance".into(),
                port: 8123,
                token: "test".into(),
                use_ssl: false,
            },
            None,
            |_, _, _, _| std::sync::mpsc::channel().1,
        )
        .unwrap();
        let registry = hub.data::<HaHubData>().unwrap().registry.clone();
        registry.lock().unwrap().upsert_room(
            "room",
            "Room",
            "room",
            &["light.reviewed".into(), "light.unreviewed".into()],
        );
        let cache = hub.data::<HaHubData>().unwrap().event_routing_cache.clone();
        {
            let mut cache = cache.lock().unwrap();
            cache.lights_ready = true;
            cache.stream_ready = true;
            cache
                .reviewed
                .insert("light.reviewed".into(), proof("entry-1"));
            cache
                .lights
                .insert("light.reviewed".into(), entry("entry-1"));
            cache
                .lights
                .insert("light.unreviewed".into(), entry("entry-2"));
        }
        let identity = DiscoveredIdentity {
            native_id: "light.reviewed".into(),
            room_id: Some("room".into()),
            room_name: Some("Room".into()),
            name: "Reviewed".into(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            hardware_ids: vec![HardwareId::serial("entry-1")],
            manufacturer: None,
            model: None,
        };
        let node = match state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 1)
        {
            ResolveResult::Created { canonical_id } => canonical_id,
            _ => panic!("fresh identity"),
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .get_mut(&node)
            .unwrap()
            .endpoints[0]
            .capabilities = Some(json!({"ha_identity":proof("entry-1")}));
        let spy = Arc::new(SpyHaTransport::new());
        spy.set_entity_state("light.reviewed", "on");
        let hook: CommandHook = Arc::new(Mutex::new(None));
        let controller = HaLightController::new(
            ContextTransport {
                spy: spy.clone(),
                hook: hook.clone(),
            },
            registry.clone(),
        )
        .with_capability_source(state.clone(), key.clone());
        let runtime: Arc<dyn RuntimeHandle> = Arc::new(RhythmRuntime::new(
            Arc::new(controller),
            MockTimeProvider::new(14.0, 172, 2026),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        runtime.add_room("room", "Room");
        runtime
            .handle_event(&InputEvent::new("room", ButtonAction::RhythmOn))
            .unwrap();
        runtime.add_node(
            &node,
            "Reviewed",
            LightNodeKind::LightDevice,
            Some("room".into()),
        );
        hub.runtime = Some(runtime.clone());
        state.lock().unwrap().hubs.insert(key.clone(), hub);
        Self {
            state,
            cache,
            registry,
            runtime,
            spy,
            key,
            node,
            hook,
        }
    }
    fn observe(&self, state: &str, brightness: u8, context: &str, second: u8) {
        let data = json!({"entity_id":"light.reviewed","new_state":{"state":state,"attributes":{"brightness":brightness,"color_temp_kelvin":3000,"supported_color_modes":["color_temp"]},"context":{"id":context,"user_id":"ha-user","parent_id":null},"last_updated":format!("2026-10-03T12:00:{second:02}+00:00")}});
        for event in translate_ws_event("state_changed", &data, &self.registry, &self.cache) {
            handle_hub_event(
                &self.state,
                event.with_hub_key(self.key.clone()),
                &mut MotionTimerState::default(),
            );
        }
    }
}

#[test]
fn engine_never_commands_unreviewed_replaced_or_stale_inventory_lights() {
    let f = Fixture::new();
    f.runtime
        .handle_event(&InputEvent::new("room", ButtonAction::Reset))
        .unwrap();
    assert_eq!(f.spy.call_count(), 1);
    assert_eq!(
        f.spy.calls()[0].data["entity_id"],
        json!(["light.reviewed"])
    );
    // Same mutable address is now backed by a different registry identity.
    f.cache
        .lock()
        .unwrap()
        .lights
        .insert("light.reviewed".into(), entry("replacement"));
    f.runtime
        .handle_event(&InputEvent::new("room", ButtonAction::Reset))
        .unwrap();
    assert_eq!(
        f.spy.call_count(),
        1,
        "replacement must not inherit route ownership"
    );
    f.cache
        .lock()
        .unwrap()
        .lights
        .insert("light.reviewed".into(), entry("entry-1"));
    let events = translate_ws_event(
        "entity_registry_updated",
        &json!({"entity_id":"light.reviewed"}),
        &f.registry,
        &f.cache,
    );
    assert!(matches!(events[0], HubEvent::TopologyChanged { .. }));
    assert!(f
        .runtime
        .handle_event(&InputEvent::new("room", ButtonAction::Reset))
        .is_err());
    assert_eq!(
        f.spy.call_count(),
        1,
        "invalidation blocks output before discovery"
    );
}

#[test]
fn own_echo_updates_authoritative_readback_external_user_pauses_only_target() {
    let f = Fixture::new();
    f.cache
        .lock()
        .unwrap()
        .own_contexts
        .push_back("rhythm-request".into());
    f.observe("on", 180, "rhythm-request", 1);
    assert!(
        f.runtime
            .engine_node_snapshot(&f.node)
            .unwrap()
            .rhythm_enabled
    );
    let observed = f.state.lock().unwrap().light_observations[&f.node].clone();
    assert!((69..=72).contains(&observed.brightness.unwrap()));
    assert_eq!(observed.kelvin, Some(3000));
    assert_eq!(observed.context_id.as_deref(), Some("rhythm-request"));
    f.observe("on", 80, "ha-user-change", 2);
    assert!(
        !f.runtime
            .engine_node_snapshot(&f.node)
            .unwrap()
            .rhythm_enabled
    );
    assert!(
        f.runtime
            .engine_node_snapshot("room")
            .unwrap()
            .rhythm_enabled
    );
    assert_eq!(
        f.spy.call_count(),
        0,
        "observations and external override produce no echo commands"
    );
    // Late packets cannot revert the newer observation or act as fresh control.
    f.observe("on", 220, "late", 1);
    assert_eq!(
        f.state.lock().unwrap().light_observations[&f.node].brightness,
        Some(31)
    );
    let event = rhythm_os::commands::build_node_state_event(
        &f.state,
        &f.runtime.engine_effective_node_snapshot(&f.node).unwrap(),
    );
    assert_eq!(event.observed_light.unwrap().brightness, Some(31));
}

#[test]
fn unavailable_remains_unknown_in_observation_and_does_not_issue_off() {
    let f = Fixture::new();
    f.observe("unavailable", 0, "offline", 1);
    let state = f.state.lock().unwrap();
    let observed = &state.light_observations[&f.node];
    assert_eq!(observed.availability, "unavailable");
    assert_eq!(observed.lights_on, None);
    assert!(!state.room_observed_power.contains_key(&f.node));
    assert_eq!(f.spy.call_count(), 0);
}

#[test]
fn persisted_review_follows_rename_but_rejects_replacement_or_other_instance() {
    let selection = HaLightSelection {
        version: 2,
        lights: BTreeMap::from([("light.old_name".into(), proof("entry-1"))]),
    };
    let mut lights = BTreeMap::from([
        ("light.new_name".into(), entry("entry-1")),
        ("light.unreviewed".into(), entry("entry-2")),
    ]);
    assert_eq!(selection.resolve(&lights), ["light.new_name".into()].into());
    lights
        .get_mut("light.new_name")
        .unwrap()
        .identity
        .as_mut()
        .unwrap()
        .scope = "other-ha".into();
    assert!(selection.resolve(&lights).is_empty());
    lights.insert("light.new_name".into(), entry("replacement"));
    assert!(selection.resolve(&lights).is_empty());
}

#[test]
fn queued_old_observation_cannot_update_replacement_at_reused_route() {
    let f = Fixture::new();
    let events = translate_ws_event(
        "state_changed",
        &json!({"entity_id":"light.reviewed","new_state":{"state":"on","attributes":{"brightness":250},"last_updated":"2026-10-03T12:00:09+00:00","context":{"id":"old-device-change","user_id":"ha-user"}}}),
        &f.registry,
        &f.cache,
    );
    assert_eq!(events.len(), 1);
    f.state
        .lock()
        .unwrap()
        .canonical_registry
        .get_mut(&f.node)
        .unwrap()
        .endpoints[0]
        .capabilities = Some(json!({"ha_identity":proof("replacement")}));
    handle_hub_event(
        &f.state,
        events[0].clone().with_hub_key(f.key.clone()),
        &mut MotionTimerState::default(),
    );
    assert!(!f
        .state
        .lock()
        .unwrap()
        .light_observations
        .contains_key(&f.node));
    assert!(
        f.runtime
            .engine_node_snapshot(&f.node)
            .unwrap()
            .rhythm_enabled
    );
}

#[test]
fn unrelated_in_flight_light_does_not_suppress_user_override_and_sync_blocks_dispatch() {
    let f = Fixture::new();
    f.cache
        .lock()
        .unwrap()
        .writes_in_flight
        .insert("light.unreviewed".into(), 1);
    f.observe("on", 30, "real-user", 1);
    assert!(
        !f.runtime
            .engine_node_snapshot(&f.node)
            .unwrap()
            .rhythm_enabled
    );
    f.state
        .lock()
        .unwrap()
        .hub_sync_in_progress
        .insert(f.key.clone());
    assert!(f
        .runtime
        .handle_event(&InputEvent::new("room", ButtonAction::Reset))
        .is_err());
    assert_eq!(f.spy.call_count(), 0);
}

#[test]
fn changed_unique_identity_has_distinct_canonical_fingerprint_even_same_registry_entry() {
    let first = proof("same-entry");
    let mut replacement = first.clone();
    replacement.unique_id = "different-device".into();
    assert_ne!(first.fingerprint(), replacement.fingerprint());
}

#[test]
fn event_during_service_response_is_classified_by_context_after_acceptance() {
    for (context, expect_pause) in [("rhythm-request", false), ("other-ha-user", true)] {
        let f = Fixture::new();
        let state = f.state.clone();
        let cache = f.cache.clone();
        let registry = f.registry.clone();
        let key = f.key.clone();
        *f.hook.lock().unwrap() = Some(Box::new(move || {
            let data = json!({"entity_id":"light.reviewed","new_state":{"state":"on","attributes":{"brightness":50},"context":{"id":context,"user_id":"ha-user","parent_id":null},"last_updated":"2026-10-03T12:00:01+00:00"}});
            for event in translate_ws_event("state_changed", &data, &registry, &cache) {
                handle_hub_event(
                    &state,
                    event.with_hub_key(key.clone()),
                    &mut MotionTimerState::default(),
                );
            }
        }));
        f.runtime
            .handle_event(&InputEvent::new("room", ButtonAction::Reset))
            .unwrap();
        assert!(
            f.runtime
                .engine_node_snapshot(&f.node)
                .unwrap()
                .rhythm_enabled,
            "in-flight attribution waits for context"
        );
        let pending: Vec<_> = f
            .state
            .lock()
            .unwrap()
            .pending_hub_event_rxs
            .iter()
            .flat_map(|rx| rx.try_iter())
            .collect();
        for event in pending {
            handle_hub_event(&f.state, event, &mut MotionTimerState::default());
        }
        assert_eq!(
            !f.runtime
                .engine_node_snapshot(&f.node)
                .unwrap()
                .rhythm_enabled,
            expect_pause
        );
        assert_eq!(
            f.spy.call_count(),
            1,
            "readback classification never echoes an extra service call"
        );
    }
}

#[test]
fn queued_observation_is_invalidated_before_reconciliation_even_when_route_still_matches() {
    let f = Fixture::new();
    let events = translate_ws_event(
        "state_changed",
        &json!({"entity_id":"light.reviewed","new_state":{"state":"off","last_updated":"2026-10-03T12:00:03+00:00"}}),
        &f.registry,
        &f.cache,
    );
    f.cache.lock().unwrap().invalidate();
    handle_hub_event(
        &f.state,
        events[0].clone().with_hub_key(f.key.clone()),
        &mut MotionTimerState::default(),
    );
    assert!(!f
        .state
        .lock()
        .unwrap()
        .light_observations
        .contains_key(&f.node));
}

#[test]
fn in_flight_user_context_resolving_during_snapshot_is_retained_until_commit() {
    for (context, expect_pause) in [("rhythm-request", false), ("other-ha-user", true)] {
        let f = Fixture::new();
        let cache = f.cache.clone();
        let registry = f.registry.clone();
        *f.hook.lock().unwrap() = Some(Box::new(move || {
            {
                let mut cache = cache.lock().unwrap();
                cache.lights_ready = false;
                cache.snapshot_generation = Some(cache.generation);
            }
            let data = json!({"entity_id":"light.reviewed","new_state":{"state":"on","attributes":{"brightness":50},"context":{"id":context,"user_id":"ha-user","parent_id":null},"last_updated":"2026-10-03T12:00:01+00:00"}});
            assert!(translate_ws_event("state_changed", &data, &registry, &cache).is_empty());
        }));
        f.runtime
            .handle_event(&InputEvent::new("room", ButtonAction::Reset))
            .unwrap();
        let mut cache = f.cache.lock().unwrap();
        assert_eq!(
            cache.snapshot_user_changes.get("light.reviewed") == Some(&proof("entry-1")),
            expect_pause
        );
        assert!(cache.pending_manual.is_empty());
        cache.invalidate();
        assert!(
            cache.snapshot_user_changes.is_empty(),
            "invalidation revokes pending override proof"
        );
    }
}

#[test]
fn invalidated_catalog_cannot_attribute_user_changes_until_fresh_inventory() {
    let f = Fixture::new();
    f.cache.lock().unwrap().invalidate();
    f.observe("on", 50, "unproven-route-user", 1);
    assert!(f.cache.lock().unwrap().snapshot_user_changes.is_empty());
    assert!(
        f.runtime
            .engine_node_snapshot(&f.node)
            .unwrap()
            .rhythm_enabled
    );
}
