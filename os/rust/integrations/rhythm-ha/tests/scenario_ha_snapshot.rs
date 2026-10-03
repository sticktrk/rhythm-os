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
                ws.send(Message::Text(json!({"id":request["id"],"type":"result","success":!matches!(reply,Reply::Rejected),"result":[]}).to_string())).await.unwrap();
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
