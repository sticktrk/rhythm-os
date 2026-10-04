//! Snapshot transactions keep old inventory through failures and fence output.
use futures_util::{SinkExt, StreamExt};
use rhythm_ha::{
    area_sync::HaDiscovery,
    hub_state::HaEventRoutingCache,
    light::{HaLightCatalogEntry, HaLightObservation},
    transport::HaConnectionConfig,
};
use rhythm_os::{
    discovery::HubDiscovery,
    state::{AppState, SharedState},
};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};
use tokio_tungstenite::tungstenite::Message;

#[derive(Clone, Copy)]
enum Reply {
    Empty,
    Partial,
    Rejected,
    Invalidated,
    Light,
}
fn fake_ha(
    replies: Vec<Reply>,
    cache: Arc<Mutex<HaEventRoutingCache>>,
) -> (HaConnectionConfig, std::thread::JoinHandle<()>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async move {
        let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap(); tx.send(listener.local_addr().unwrap().port()).unwrap();
        for reply in replies {
            let (stream,_)=listener.accept().await.unwrap();
            let mut ws=tokio_tungstenite::accept_async(stream).await.unwrap();
            ws.send(Message::Text(json!({"type":"auth_required"}).to_string())).await.unwrap();
            ws.next().await.unwrap().unwrap();
            ws.send(Message::Text(json!({"type":"auth_ok"}).to_string())).await.unwrap();
            let mut requests=Vec::new();
            for _ in 0..4 { requests.push(serde_json::from_str::<Value>(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap()); }
            if matches!(reply,Reply::Invalidated) { cache.lock().unwrap().generation+=1; }
            for request in requests {
                if matches!(reply,Reply::Partial) && request["type"]=="get_states" { break; }
                let result = if matches!(reply, Reply::Light) {
                    match request["type"].as_str().unwrap() {
                        "config/entity_registry/list" => json!([{"id":"fixture-entry","unique_id":"fixture-unique","platform":"test","entity_id":"light.fixture"}]),
                        "get_states" => json!([{"entity_id":"light.fixture","state":"off","last_updated":"2026-10-03T12:00:00+00:00"}]),
                        _ => json!([]),
                    }
                } else { json!([]) };
                ws.send(Message::Text(json!({"id":request["id"],"type":"result","success":!matches!(reply,Reply::Rejected),"result":result}).to_string())).await.unwrap();
                if matches!(reply,Reply::Rejected) { break; }
            }
            let _=ws.close(None).await;
        }
    })
    });
    (
        HaConnectionConfig {
            host: "127.0.0.1".into(),
            port: rx.recv().unwrap(),
            token: "test".into(),
            use_ssl: false,
        },
        handle,
    )
}
fn cache_and_state() -> (Arc<Mutex<HaEventRoutingCache>>, SharedState) {
    let mut initial = HaEventRoutingCache::default();
    initial.stream_ready = true;
    initial.lights_ready = true;
    let cache = Arc::new(Mutex::new(initial));
    cache.lock().unwrap().lights.insert(
        "light.previous".into(),
        HaLightCatalogEntry {
            identity: None,
            name: "previous".into(),
            area_id: None,
            capabilities: rhythm_ha::light::capabilities(&json!({})),
            observation: HaLightObservation::parse(&json!({"state":"off"})),
        },
    );
    let state = Arc::new(Mutex::new(AppState {
        platform_context: "ha_addon",
        data_dir: std::env::temp_dir()
            .join(format!("ha-no-persisted-selection-{}", std::process::id()))
            .to_string_lossy()
            .to_string(),
        ..AppState::default()
    }));
    (cache, state)
}
#[test]
fn complete_empty_inventory_is_empty_and_not_ready_until_runtime_commit() {
    let (cache, state) = cache_and_state();
    let (config, server) = fake_ha(vec![Reply::Empty], cache.clone());
    let discovery = HaDiscovery::new(config).with_live_state(&state, cache.clone());
    assert!(discovery.discover_rooms().unwrap().is_empty());
    assert!(cache.lock().unwrap().lights.is_empty());
    assert!(!cache.lock().unwrap().lights_ready);
    discovery.discover_devices().unwrap();
    discovery.discover_identities().unwrap();
    discovery.release_resources();
    assert!(
        !cache.lock().unwrap().lights_ready,
        "transport release must not authorize before runtime reconciliation"
    );
    server.join().unwrap();
    discovery.complete_sync().unwrap();
    assert!(cache.lock().unwrap().lights_ready);
}
#[test]
fn partial_and_rejected_inventory_preserve_previous_and_remain_fenced() {
    for reply in [Reply::Partial, Reply::Rejected] {
        let (cache, state) = cache_and_state();
        let (config, server) = fake_ha(vec![reply], cache.clone());
        let discovery = HaDiscovery::new(config).with_live_state(&state, cache.clone());
        assert!(discovery.discover_rooms().is_err());
        assert!(cache.lock().unwrap().lights.contains_key("light.previous"));
        assert!(!cache.lock().unwrap().lights_ready);
        assert!(discovery.complete_sync().is_err());
        server.join().unwrap();
    }
}
#[test]
fn generation_change_retries_same_connection_lifecycle_and_commit_rechecks_generation() {
    let (cache, state) = cache_and_state();
    let (config, server) = fake_ha(vec![Reply::Invalidated, Reply::Empty], cache.clone());
    let discovery = HaDiscovery::new(config).with_live_state(&state, cache.clone());
    assert!(discovery.discover_rooms().unwrap().is_empty());
    cache.lock().unwrap().generation += 1;
    assert!(
        discovery.discover_devices().is_err(),
        "no mixed-generation devices after rooms applied"
    );
    server.join().unwrap();
    assert!(discovery.complete_sync().is_err());
    assert!(!cache.lock().unwrap().lights_ready);
}

#[test]
fn live_change_after_inventory_sampling_reaches_core_at_snapshot_commit() {
    assert_snapshot_handoff(None, false);
}

#[test]
fn explicit_user_change_during_snapshot_pauses_only_that_light_after_commit() {
    assert_snapshot_handoff(Some("external-user"), true);
    assert_snapshot_handoff(Some("own-call"), false);
}

fn assert_snapshot_handoff(context: Option<&str>, should_pause: bool) {
    use rhythm_core::{
        runtime::{
            handle::RuntimeHandle, orchestrator::RhythmRuntime, registry::SimpleDeviceRegistry,
            scheduler::NoOpScheduler, time::MockTimeProvider, RuntimeConfig,
        },
        ButtonAction, InputEvent, LightNodeKind, NoOpController,
    };
    use rhythm_ha::{ha_lifecycle::connect_ha, hub_state::HaHubData};
    use rhythm_os::{
        canonical::identity::HubKey,
        event_loop::{handle_hub_event, MotionTimerState},
        hub::HubType,
    };
    let (cache, state) = cache_and_state();
    let (config, server) = fake_ha(vec![Reply::Light], cache.clone());
    let key = HubKey::new(HubType::new("homeassistant"), "fixture-ha");
    let (mut hub, _) = connect_ha(&state, key.clone(), config.clone(), None, |_, _, _, _| {
        std::sync::mpsc::channel().1
    })
    .unwrap();
    let registry = hub.data::<HaHubData>().unwrap().registry.clone();
    hub.hub_data
        .downcast_mut::<HaHubData>()
        .unwrap()
        .event_routing_cache = cache.clone();
    state.lock().unwrap().hubs.insert(key.clone(), hub);
    let discovery = HaDiscovery::new(config).with_live_state(&state, cache.clone());
    discovery.discover_rooms().unwrap();
    let identity = discovery.discover_identities().unwrap().remove(0);
    let initial = discovery.endpoint_observation(&identity.native_id).unwrap();
    let capabilities = discovery
        .endpoint_capabilities(&identity.native_id)
        .unwrap();
    let node_id = {
        let mut state = state.lock().unwrap();
        state.canonical_registry.resolve(&identity, &key, 1);
        let node_id = state
            .canonical_registry
            .find_by_native_id(&key, &identity.native_id)
            .unwrap()
            .id
            .clone();
        state
            .canonical_registry
            .get_mut(&node_id)
            .unwrap()
            .endpoints[0]
            .capabilities = Some(capabilities);
        state.light_observations.insert(node_id.clone(), initial);
        node_id
    };
    let runtime: Arc<dyn RuntimeHandle> = Arc::new(RhythmRuntime::new(
        Arc::new(NoOpController::new()),
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    ));
    runtime.add_room("fixture-room", "Fixture room");
    runtime
        .handle_event(&InputEvent::new("fixture-room", ButtonAction::RhythmOn))
        .unwrap();
    runtime.add_node(
        &node_id,
        "Fixture",
        LightNodeKind::LightDevice,
        Some("fixture-room".into()),
    );
    state.lock().unwrap().hubs.get_mut(&key).unwrap().runtime = Some(runtime.clone());
    cache
        .lock()
        .unwrap()
        .own_contexts
        .push_back("own-call".into());
    let update = json!({"entity_id":"light.fixture","new_state":{"state":"on","attributes":{"brightness":204},"last_updated":"2026-10-03T12:00:01+00:00","context":{"id":context,"user_id":"user","parent_id":null}}});
    assert!(
        rhythm_ha::events::translate_ws_event("state_changed", &update, &registry, &cache)
            .is_empty()
    );
    assert_eq!(
        state.lock().unwrap().light_observations[&node_id].lights_on,
        Some(false)
    );
    server.join().unwrap();
    discovery.complete_sync().unwrap();
    let pending: Vec<_> = state
        .lock()
        .unwrap()
        .pending_hub_event_rxs
        .iter()
        .flat_map(|rx| rx.try_iter())
        .collect();
    assert_eq!(
        pending.len(),
        1,
        "commit must deliver the latest snapshot through the event pipeline"
    );
    handle_hub_event(&state, pending[0].clone(), &mut MotionTimerState::default());
    let observed = state.lock().unwrap().light_observations[&node_id].clone();
    assert_eq!(observed.lights_on, Some(true));
    assert_eq!(observed.brightness, Some(80));
    assert_eq!(
        !runtime
            .engine_node_snapshot(&node_id)
            .unwrap()
            .rhythm_enabled,
        should_pause
    );
    assert!(
        runtime
            .engine_node_snapshot("fixture-room")
            .unwrap()
            .rhythm_enabled
    );

    // A live event can overtake queued snapshot delivery without being reverted.
    let newer = json!({"entity_id":"light.fixture","new_state":{"state":"off","last_updated":"2026-10-03T12:00:02+00:00"}});
    for event in rhythm_ha::events::translate_ws_event("state_changed", &newer, &registry, &cache) {
        handle_hub_event(
            &state,
            event.with_hub_key(key.clone()),
            &mut MotionTimerState::default(),
        );
    }
    handle_hub_event(&state, pending[0].clone(), &mut MotionTimerState::default());
    assert_eq!(
        state.lock().unwrap().light_observations[&node_id].lights_on,
        Some(false)
    );
    state.lock().unwrap().light_observations.remove(&node_id);
    cache.lock().unwrap().invalidate();
    handle_hub_event(&state, pending[0].clone(), &mut MotionTimerState::default());
    assert!(
        !state
            .lock()
            .unwrap()
            .light_observations
            .contains_key(&node_id),
        "a later invalidation revokes queued snapshot observations"
    );
}
