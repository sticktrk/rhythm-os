//! Rhythm OS — macOS/Linux CLI server.
//!
//! Native server process exposing the full API surface.
//! Connects to Hue bridges, Home Assistant, and future hubs via the
//! integration registry in `hub::INTEGRATIONS`.

use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::Parser;
use log::{info, warn};
use rhythm_os::logging;
use rhythm_os::state::{AppState, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;
use rhythm_server::{http_server, hub};

const VERSION: &str = rhythm_server::BUILD_VERSION;

/// Rhythm OS server for macOS/Linux.
#[derive(Parser, Debug)]
#[command(name = "rhythm-server", version = VERSION, about)]
struct Args {
    /// HTTP server port.
    #[arg(short, long, default_value_t = 54448)]
    port: u16,

    /// Data directory for persistent storage.
    #[arg(short, long, default_value = "~/.rhythm")]
    data_dir: String,

    /// Log level (trace, debug, info, warn, error).
    #[arg(long, default_value = "info")]
    log_level: String,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let platform_type =
        std::env::var("RHYTHM_PLATFORM_TYPE").unwrap_or_else(|_| "desktop".to_string());
    let platform_context =
        std::env::var("RHYTHM_PLATFORM_CONTEXT").unwrap_or_else(|_| "server".to_string());
    let appliance_runtime = platform_type == "appliance";

    logging::init_native_logging(&args.log_level)?;

    info!(target: "sys", "Rhythm Server v{} starting...", VERSION);

    // If a self-update was just installed, count this start attempt and roll
    // back to the previous binaries once a crash-looping build exhausts its
    // probation. Must run before anything that could crash the process.
    match rhythm_server::self_update::startup_update_health_check() {
        rhythm_server::self_update::StartupUpdateDisposition::NoPendingUpdate => {}
        rhythm_server::self_update::StartupUpdateDisposition::PendingVerification { attempt } => {
            info!(
                target: "sys",
                "Self-update awaiting verification (start attempt {})",
                attempt
            );
        }
        rhythm_server::self_update::StartupUpdateDisposition::RolledBack { restored } => {
            log::error!(
                target: "sys",
                "Self-update failed verification after repeated start attempts; restored {} previous binar{} — exiting so the supervisor restarts the previous build",
                restored.len(),
                if restored.len() == 1 { "y" } else { "ies" }
            );
            std::process::exit(1);
        }
    }

    // Expand ~ in data dir
    let data_dir = shellexpand(&args.data_dir);
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
        s.platform_type = Box::leak(platform_type.into_boxed_str());
        s.platform_context = Box::leak(platform_context.into_boxed_str());
        s.listen_port = Some(args.port);
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

        // Credentials interceptor: delegates to integrations
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

    if appliance_runtime {
        rhythm_server::liveness::spawn_periodic_watchdog(state.clone());
        rhythm_server::auto_update::spawn(state.clone());
    }

    // Start tokio runtime for the async HTTP server + mDNS
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_server(state, args.port))
}

fn install_factory_reset_hook(state: &SharedState) -> Result<()> {
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.after_factory_reset_fn = Some(Arc::new(|_| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            info!(target: "sys", "Exiting after factory reset so the supervisor can restart Rhythm Server");
            std::process::exit(1);
        });
    }));
    Ok(())
}

async fn run_server(state: SharedState, port: u16) -> Result<()> {
    rhythm_os::state::capture_tokio_runtime_handle(&state);

    // Start HTTP server
    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone());
    let listener = tokio::net::TcpListener::bind(&addr).await.map_err(|e| {
        anyhow::anyhow!(
            "Failed to bind to {} — is another process using this port? {}",
            addr,
            e
        )
    })?;
    info!(target: "sys", "Rhythm Server listening on http://{}", addr);

    // The listener is up: give the (possibly freshly updated) build its
    // post-startup grace period, then discard self-update rollback backups.
    rhythm_server::self_update::spawn_update_verification_marker();

    // Register mDNS service for auto-discovery by clients
    let _mdns = rhythm_os::mdns::register_mdns_service(port, "server", VERSION, "rhythm-server");

    // Bootstrap configured hubs on a blocking background thread after the
    // listener is up so clients can connect immediately during hub sync.
    rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);

    axum::serve(
        listener,
        server.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

/// Expand ~ to home directory.
fn shellexpand(path: &str) -> String {
    if path.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return format!("{}{}", home, &path[1..]);
        }
    }
    path.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::sync::Mutex as StdMutex;

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    struct EnvRestore {
        key: &'static str,
        value: Option<OsString>,
    }

    impl EnvRestore {
        fn new(key: &'static str) -> Self {
            Self {
                key,
                value: std::env::var_os(key),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            match &self.value {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn shellexpand_expands_home_prefix_only_when_home_is_available() {
        let _guard = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new("HOME");

        std::env::set_var("HOME", "/Users/tester");
        assert_eq!(shellexpand("~/.rhythm"), "/Users/tester/.rhythm");
        assert_eq!(shellexpand("/tmp/.rhythm"), "/tmp/.rhythm");
        assert_eq!(shellexpand("~"), "~");

        std::env::remove_var("HOME");
        assert_eq!(shellexpand("~/missing-home"), "~/missing-home");
    }

    #[test]
    fn install_factory_reset_hook_registers_restart_callback() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));

        install_factory_reset_hook(&state).unwrap();

        assert!(state.lock().unwrap().after_factory_reset_fn.is_some());
    }
}
