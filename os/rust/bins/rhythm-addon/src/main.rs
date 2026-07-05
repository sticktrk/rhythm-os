//! Rhythm OS — Home Assistant add-on binary.
//!
//! Connects to Home Assistant via WebSocket for events and REST API for
//! light commands. Headless server with REST API only (no UI).

mod http_server;
mod hub;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};
use rhythm_os::logging;
use rhythm_os::state::{AppState, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;

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

    logging::init_native_logging(&log_level)?;

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
        s.storage = Some(std::sync::Arc::new(file_storage));
        rhythm_os_runtime_modules::install_default_light_runtime_modules(&mut s)?;

        // Load persisted state
        rhythm_os::storage::load_persisted_state(&mut s);

        // Set integration-driven callbacks from the static registry
        let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
        s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
        s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
        s.register_controller_fn = Some(callbacks.register_controller_fn);
        s.sync_topology_groups_fn = Some(callbacks.sync_topology_groups_fn);
        s.start_pairing_fn = Some(callbacks.start_pairing_fn);
        s.start_unpairing_fn = Some(callbacks.start_unpairing_fn);
        s.run_device_test_fn = Some(callbacks.run_device_test_fn);
        s.save_device_test_report_fn = Some(callbacks.save_device_test_report_fn);
        s.hub_capabilities = callbacks.hub_capabilities.clone();

        // Credentials interceptor: delegates to integrations (e.g., HA auto-fills SUPERVISOR_TOKEN)
        s.hub_credentials_interceptor = Some(rhythm_os::hub::combined_credentials_interceptor(
            hub::INTEGRATIONS,
        ));
        s.request_hub_bootstrap_fn = Some(Arc::new(|state| {
            rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);
        }));
        s.remote_access_controller = Some(Arc::new(
            rhythm_os::remote_access::child_process_controller_from_env(),
        ));
    }
    if let Err(error) = rhythm_os::remote_access::reconcile_remote_access_runtime(&state) {
        warn!(
            target: "sys",
            "Remote access runtime did not reconcile at startup: {:#}",
            error
        );
    }
    install_factory_reset_hook(&state)?;

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

fn install_factory_reset_hook(state: &SharedState) -> Result<()> {
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.after_factory_reset_fn = Some(Arc::new(|_| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            info!(target: "sys", "Exiting after factory reset so the add-on supervisor can restart Rhythm");
            std::process::exit(1);
        });
    }));
    Ok(())
}

async fn run_server(state: SharedState, port: u16) -> Result<()> {
    rhythm_os::state::capture_tokio_runtime_handle(&state);

    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| anyhow::anyhow!("Failed to bind to {} — {}", addr, e))?;
    info!(target: "sys", "Rhythm Addon listening on http://{}", addr);

    // Register mDNS service for auto-discovery by clients
    let _mdns = rhythm_os::mdns::register_mdns_service(port, "addon", VERSION, "rhythm-addon");

    // Bootstrap configured hubs on a blocking background thread after the
    // listener is up so clients can connect immediately during hub sync.
    rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);

    axum::serve(listener, server).await?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_factory_reset_hook_registers_restart_callback() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));

        install_factory_reset_hook(&state).unwrap();

        assert!(state.lock().unwrap().after_factory_reset_fn.is_some());
    }
}
