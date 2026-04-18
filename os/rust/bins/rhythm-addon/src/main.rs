//! Rhythm OS — Home Assistant add-on binary.
//!
//! Connects to Home Assistant via WebSocket for events and REST API for
//! light commands. Headless server with REST API only (no UI).

mod http_server;
mod hub;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};
use rhythm_os::state::{AppState, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;

/// Format tracing timestamps in local time instead of UTC.
struct LocalTimer;
impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f")
        )
    }
}

const VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> Result<()> {
    // Read config from environment
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|p| p.parse().ok())
        .unwrap_or(54448);

    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    // Init logging
    let base_filter = std::env::var("RUST_LOG").unwrap_or(log_level);
    tracing_subscriber::fmt()
        .with_timer(LocalTimer)
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "{},mdns_sd=off",
            base_filter
        )))
        .init();

    info!(target: "sys", "Rhythm Addon v{} starting...", VERSION);

    // Detect addon mode vs standalone
    let is_supervisor = std::env::var("SUPERVISOR_TOKEN").is_ok();
    if is_supervisor {
        info!(target: "sys", "Running in HA Supervisor mode");
    } else {
        info!(target: "sys", "Running in standalone mode");
    }

    // Data directory: /data/ for addon, or RHYTHM_STATE_PATH for standalone
    let data_dir = std::env::var("RHYTHM_STATE_PATH").unwrap_or_else(|_| "/data".to_string());
    std::fs::create_dir_all(&data_dir)?;

    // Create storage backend
    let file_storage = FileStorage::new(&data_dir)?;

    // Build shared state
    let (event_tx, _) = tokio::sync::broadcast::channel(64);
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.event_tx = Some(event_tx);
        s.firmware_version = Box::leak(VERSION.to_string().into_boxed_str());
        s.platform_type = "desktop";
        s.platform_context = "ha_addon";
        s.listen_port = Some(port);
        s.data_dir = data_dir.clone();
        s.storage = Some(Box::new(file_storage));

        // Load persisted state
        rhythm_os::storage::load_persisted_state(&mut s);

        // Set integration-driven callbacks from the static registry
        let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
        s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
        s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
        s.register_controller_fn = Some(callbacks.register_controller_fn);
        s.start_pairing_fn = Some(callbacks.start_pairing_fn);
        s.start_unpairing_fn = Some(callbacks.start_unpairing_fn);
        s.hub_capabilities = callbacks.hub_capabilities.clone();

        // Credentials interceptor: delegates to integrations (e.g., HA auto-fills SUPERVISOR_TOKEN)
        s.hub_credentials_interceptor = Some(rhythm_os::hub::combined_credentials_interceptor(
            hub::INTEGRATIONS,
        ));
    }

    let (work_tx, work_rx) = std::sync::mpsc::sync_channel::<WorkItem>(64);
    let (periodic_tx, periodic_rx) = std::sync::mpsc::sync_channel::<WorkItem>(64);
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.work_tx = Some(work_tx);
        s.periodic_work_tx = Some(periodic_tx);
    }

    {
        let worker_state = state.clone();
        std::thread::Builder::new()
            .name("cmd-worker".to_string())
            .spawn(move || {
                info!(target: "sys", "cmd-worker started");
                loop {
                    match work_rx.recv() {
                        Ok(item) => rhythm_os::event_loop::process_work_item(&worker_state, item),
                        Err(_) => {
                            warn!(target: "sys", "cmd-worker: channel disconnected");
                            return;
                        }
                    }
                }
            })
            .expect("Failed to spawn cmd-worker thread");
    }

    {
        let periodic_worker_state = state.clone();
        std::thread::Builder::new()
            .name("periodic-worker".to_string())
            .spawn(move || {
                info!(target: "sys", "periodic-worker started");
                loop {
                    match periodic_rx.recv() {
                        Ok(item) => {
                            rhythm_os::event_loop::process_work_item(&periodic_worker_state, item)
                        }
                        Err(_) => {
                            warn!(target: "sys", "periodic-worker: channel disconnected");
                            return;
                        }
                    }
                }
            })
            .expect("Failed to spawn periodic-worker thread");
    }

    info!(target: "sys", "Data directory: {}", data_dir);

    // Spawn event loop on a dedicated std::thread (not tokio)
    {
        let event_state = state.clone();
        std::thread::Builder::new()
            .name("event-loop".to_string())
            .spawn(move || {
                rhythm_os::event_loop::run_event_loop(event_state, Vec::new());
            })
            .expect("Failed to spawn event loop thread");
    }

    // Spawn periodic updater on a dedicated std::thread
    {
        let periodic_state = state.clone();
        std::thread::Builder::new()
            .name("periodic".to_string())
            .spawn(move || {
                rhythm_os::periodic::run_periodic_loop(periodic_state, None::<fn()>);
            })
            .expect("Failed to spawn periodic thread");
    }

    // Start tokio runtime for the async HTTP server
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(state, port))
}

async fn run_server(state: SharedState, port: u16) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| panic!("Failed to bind to {} — {}", addr, e));
    info!(target: "sys", "Rhythm Addon listening on http://{}", addr);

    // Register mDNS service for auto-discovery by clients
    let _mdns = rhythm_os::mdns::register_mdns_service(port, "addon", VERSION, "rhythm-addon");

    // Bootstrap configured hubs on a blocking background thread after the
    // listener is up so clients can connect immediately during hub sync.
    spawn_hub_bootstrap(state.clone());

    axum::serve(listener, server).await?;

    Ok(())
}

fn spawn_hub_bootstrap(state: SharedState) {
    info!(
        target: "sys",
        "Starting hub bootstrap in background; HTTP startup will not wait for hub sync"
    );

    std::thread::Builder::new()
        .name("hub-bootstrap".to_string())
        .spawn(move || bootstrap_hubs(&state))
        .expect("Failed to spawn hub bootstrap thread");
}

fn bootstrap_hubs(state: &SharedState) {
    let all_creds: Vec<(rhythm_os::canonical::identity::HubKey, String)> = match state.lock() {
        Ok(s) => s
            .hub_credentials
            .iter()
            .filter_map(|(key, creds)| {
                creds
                    .hub_type
                    .as_ref()
                    .map(|ht| (key.clone(), ht.as_str().to_string()))
            })
            .collect(),
        Err(_) => {
            warn!(target: "sys", "Hub bootstrap aborted: state lock poisoned");
            return;
        }
    };

    if all_creds.is_empty() {
        info!(target: "sys", "No hub configured, waiting for credentials via HTTP");
        return;
    }

    for (key, hub_type_str) in &all_creds {
        if let Some(integration) = hub::find_integration(hub::INTEGRATIONS, hub_type_str) {
            integration.refresh_credentials(state, key);
        }
    }

    for (key, hub_type_str) in &all_creds {
        if let Some(integration) = hub::find_integration(hub::INTEGRATIONS, hub_type_str) {
            info!(target: "sys", "Connecting hub {} (type={})...", key, hub_type_str);
            match integration.connect_and_start(state.clone(), key) {
                Ok(rx) => {
                    if let Ok(mut s) = state.lock() {
                        s.pending_hub_event_rxs.push(rx);
                    } else {
                        warn!(
                            target: "sys",
                            "Connected hub {} but failed to register its event stream",
                            key
                        );
                    }
                }
                Err(e) => {
                    warn!(
                        target: "sys",
                        "Failed to connect hub {}: {} (will retry on credential push)",
                        key,
                        e
                    );
                }
            }
        } else {
            warn!(
                target: "sys",
                "No integration for hub type '{}', skipping {}",
                hub_type_str,
                key
            );
        }
    }

    let has_hub = state.lock().map(|s| s.has_any_hub()).unwrap_or(false);
    if !has_hub {
        return;
    }

    if let Err(e) = rhythm_os::room_sync::sync_all_hubs(state) {
        warn!(target: "sys", "Initial room sync failed: {}", e);
    }
    rhythm_os::room_sync::poll_initial_light_state(state);

    let hub_entries: Vec<(String, rhythm_os::canonical::identity::HubKey)> = match state.lock() {
        Ok(s) => s
            .hubs
            .iter()
            .map(|(key, hub)| (hub.hub_type.as_str().to_string(), key.clone()))
            .collect(),
        Err(_) => {
            warn!(target: "sys", "Skipping post-connect hooks: state lock poisoned");
            return;
        }
    };

    for (hub_type_str, key) in &hub_entries {
        if let Some(integration) = hub::find_integration(hub::INTEGRATIONS, hub_type_str) {
            integration.post_connect(state, key);
        }
    }
}
