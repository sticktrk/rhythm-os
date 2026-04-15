//! Rhythm OS Linux embedded appliance runtime.
//!
//! Reuses the native Linux/macOS server stack, but gives embedded-appliance
//! targets like rpiz their own binary crate so board-specific provisioning,
//! networking, and packaging concerns do not accumulate in `rhythm-server`.

mod ble_provision;
mod http_server;
mod wifi;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use log::{info, warn};
use rhythm_os::state::{AppState, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;
use rhythm_server::hub;

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

/// Rhythm OS Linux embedded appliance.
#[derive(Parser, Debug)]
#[command(name = "rhythm-linux-embedded", version, about)]
struct Args {
    /// HTTP server port.
    #[arg(short, long, default_value_t = 54448)]
    port: u16,

    /// Data directory for persistent storage.
    #[arg(short, long, default_value = "/data")]
    data_dir: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,
}

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> Result<()> {
    let args = Args::parse();
    let platform_type =
        std::env::var("RHYTHM_PLATFORM_TYPE").unwrap_or_else(|_| "embedded".to_string());
    let platform_context =
        std::env::var("RHYTHM_PLATFORM_CONTEXT").unwrap_or_else(|_| "linux-embedded".to_string());

    // Init logging — always suppress noisy mdns-sd AAAA errors
    let base_filter = std::env::var("RUST_LOG").unwrap_or_else(|_| args.log_level.clone());
    tracing_subscriber::fmt()
        .with_timer(LocalTimer)
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "{},mdns_sd=off",
            base_filter
        )))
        .init();

    info!(target: "sys", "Rhythm Linux Embedded v{} starting...", VERSION);

    std::fs::create_dir_all(&args.data_dir)?;

    let file_storage = FileStorage::new(&args.data_dir)?;

    let (event_tx, _) = tokio::sync::broadcast::channel(64);
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.event_tx = Some(event_tx);
        s.firmware_version = Box::leak(VERSION.to_string().into_boxed_str());
        s.platform_type = Box::leak(platform_type.into_boxed_str());
        s.platform_context = Box::leak(platform_context.into_boxed_str());
        s.listen_port = Some(args.port);
        s.data_dir = args.data_dir.clone();
        s.storage = Some(Box::new(file_storage));

        rhythm_os::storage::load_persisted_state(&mut s);

        let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
        s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
        s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
        s.register_controller_fn = Some(callbacks.register_controller_fn);
        s.start_pairing_fn = Some(callbacks.start_pairing_fn);
        s.start_unpairing_fn = Some(callbacks.start_unpairing_fn);
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

    info!(target: "sys", "Data directory: {}", args.data_dir);

    {
        let event_state = state.clone();
        std::thread::Builder::new()
            .name("event-loop".to_string())
            .spawn(move || {
                rhythm_os::event_loop::run_event_loop(event_state, Vec::new());
            })
            .expect("Failed to spawn event loop thread");
    }

    {
        let periodic_state = state.clone();
        std::thread::Builder::new()
            .name("periodic".to_string())
            .spawn(move || {
                rhythm_os::periodic::run_periodic_loop(periodic_state, None::<fn()>);
            })
            .expect("Failed to spawn periodic thread");
    }

    let provisioning = ble_provision::ProvisioningManager::new(VERSION.to_string());
    if let Err(e) = provisioning.ensure_running_if_needed("startup") {
        warn!(
            target: "sys",
            "BLE provisioning sidecar did not start cleanly: {:#}",
            e
        );
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(state, args.port, provisioning))
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

async fn run_server(
    state: SharedState,
    port: u16,
    provisioning: ble_provision::ProvisioningManager,
) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone(), provisioning);
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .unwrap_or_else(|e| {
            panic!(
                "Failed to bind to {} — is another process using this port? {}",
                addr, e
            )
        });
    info!(target: "sys", "Rhythm Linux Embedded listening on http://{}", addr);

    // Keep the existing server-style mDNS identity for now so the rpiz split
    // does not also change discovery semantics.
    let _mdns = rhythm_os::mdns::register_mdns_service(port, "server", VERSION, "rhythm-server");

    spawn_hub_bootstrap(state.clone());

    axum::serve(listener, server).await?;

    Ok(())
}
