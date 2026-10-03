//! Rhythm OS — Home Assistant add-on binary.
//!
//! Connects to Home Assistant via WebSocket for events and REST API for
//! light commands. Headless server with REST API only (no UI).

mod http_server;
mod hub;
mod mobile_access;
mod policy;
mod selection;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::{info, warn};
use rhythm_os::logging;
use rhythm_os::remote_access::ChildProcessRemoteAccessController;
use rhythm_os::state::{AppState, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;

const MOBILE_PORT: u16 = 54448;
const ADMIN_PORT: u16 = 54449;

const VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

fn main() -> Result<()> {
    let log_level = std::env::var("LOG_LEVEL").unwrap_or_else(|_| "info".to_string());

    logging::init_native_logging(&log_level)?;

    info!(target: "sys", "Rhythm Addon v{} starting...", VERSION);

    anyhow::ensure!(
        std::env::var("SUPERVISOR_TOKEN").is_ok_and(|token| !token.trim().is_empty()),
        "Home Assistant Supervisor credentials are required"
    );
    let api_token_path = std::env::var("RHYTHM_ADDON_API_TOKEN_FILE")
        .unwrap_or_else(|_| "/run/rhythm/api-token".into());
    let api_token = std::fs::read_to_string(api_token_path)?;
    let access = Arc::new(mobile_access::MobileAccess::new(api_token.trim())?);
    let controller = Arc::new(
        rhythm_os::remote_access::child_process_controller_from_env()
            .with_metrics_addr("127.0.0.1:54450"),
    );

    // Data directory: /data/ for addon, or RHYTHM_STATE_PATH for standalone
    let data_dir =
        std::env::var("RHYTHM_STATE_PATH").unwrap_or_else(|_| "/data/rhythm".to_string());
    std::fs::create_dir_all(&data_dir)?;

    // Create storage backend
    let file_storage = FileStorage::new(&data_dir)?;
    validate_security_stores(&data_dir)?;

    // Build shared state
    let (event_tx, _) = tokio::sync::broadcast::channel(64);
    let state: SharedState = Arc::new(Mutex::new(AppState::default()));
    {
        let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        s.event_tx = Some(event_tx);
        s.firmware_version = Box::leak(VERSION.to_string().into_boxed_str());
        s.platform_type = "desktop";
        s.platform_context = "ha_addon";
        s.listen_port = Some(MOBILE_PORT);
        s.data_dir = data_dir.clone();
        s.storage = Some(std::sync::Arc::new(file_storage));
        rhythm_os_runtime_modules::install_default_light_runtime_modules(&mut s)?;

        // Load persisted state
        rhythm_os::storage::load_persisted_state(&mut s);
        anyhow::ensure!(
            read_installation_identity(&data_dir)?.as_deref()
                == Some(s.server_instance_id.as_str()),
            "Installation identity is not durable; recovery review required"
        );
        // Phone credentials and installation identity survive ordinary restart.
        // The separate loopback admin credential is never installed in this store.
        s.require_api_auth = true;
        s.api_auth.require_api_auth = Some(true);
        restore_managed_selection(&mut s, &data_dir)?;
        s.hub_credentials.clear();
        let credentials = rhythm_ha::provider::supervisor_credentials();
        s.hub_credentials
            .insert(credentials.hub_key().expect("HA key"), credentials);
        if let Some(storage) = &s.storage {
            storage.save_all_hub_credentials(
                &s.hub_credentials.values().cloned().collect::<Vec<_>>(),
            )?;
        }

        // Set integration-driven callbacks from the static registry
        let callbacks = rhythm_os::hub::integration_callbacks(hub::INTEGRATIONS);
        s.ensure_runtime_fn = Some(callbacks.ensure_runtime_fn);
        s.get_hub_provider_fn = Some(callbacks.get_hub_provider_fn);
        s.register_controller_fn = Some(callbacks.register_controller_fn);
        s.sync_topology_groups_fn = Some(callbacks.sync_topology_groups_fn);
        s.sync_required_topology_groups_fn = Some(callbacks.sync_required_topology_groups_fn);
        s.reconcile_external_controller_authority_fn =
            Some(callbacks.reconcile_external_controller_authority_fn);
        s.release_external_controller_authority_fn =
            Some(callbacks.release_external_controller_authority_fn);
        s.finalize_external_controller_release_fn =
            Some(callbacks.finalize_external_controller_release_fn);
        s.prepare_hub_device_room_assignment_fn =
            Some(callbacks.prepare_hub_device_room_assignment_fn);
        s.delete_source_room_fn = Some(callbacks.delete_source_room_fn);
        s.rename_hub_device_fn = Some(callbacks.rename_hub_device_fn);
        s.start_pairing_fn = Some(callbacks.start_pairing_fn);
        s.reconcile_pairing_results_fn = Some(callbacks.reconcile_pairing_results_fn);
        s.start_unpairing_fn = Some(callbacks.start_unpairing_fn);
        s.read_wifi_network_fn = Some(callbacks.read_wifi_network_fn);
        s.change_wifi_fn = Some(callbacks.change_wifi_fn);
        s.load_pairing_recovery_fn = Some(callbacks.load_pairing_recovery_fn);
        s.purge_pairing_recovery_fn = Some(callbacks.purge_pairing_recovery_fn);
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
        s.remote_access_controller = Some(controller.clone());
    }
    rhythm_os::commands::reconcile_device_health(&state);
    install_factory_reset_hook(&state, access.clone(), controller.clone())?;

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
        .block_on(run_server(state, access, controller))
}

fn restore_managed_selection(state: &mut AppState, data_dir: &str) -> Result<()> {
    state.auto_update = false;
    // Runtime routing starts empty until the fresh HA snapshot verifies proof.
    // Preserve saved enabled intent only when reviewed identities survive.
    state.managed_ha_lights = Some(selection::load(data_dir)?);
    if !std::path::Path::new(data_dir)
        .join("settings.json")
        .exists()
        || !selection::has_reviewed_lights(data_dir)?
    {
        state.light_breaker_enabled = false;
    }
    Ok(())
}

fn validate_security_stores(data_dir: &str) -> Result<()> {
    let root = std::path::Path::new(data_dir);
    let identity = read_installation_identity(data_dir)?;
    anyhow::ensure!(
        identity.is_some()
            || !["auth.json", "remote_access.json", "activity_cloud.json"]
                .iter()
                .any(|name| root.join(name).exists()),
        "Installation identity missing beside retained credentials; recovery review required"
    );
    for filename in ["auth.json", "remote_access.json"] {
        let path = std::path::Path::new(data_dir).join(filename);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        let version = if filename == "auth.json" {
            serde_json::from_slice::<rhythm_os::auth::StoredApiAuth>(&bytes)?.schema_version
        } else {
            serde_json::from_slice::<rhythm_os::remote_access::StoredRemoteAccessConfig>(&bytes)?
                .schema_version
        };
        anyhow::ensure!(
            version == 1,
            "Unsupported security store schema in {filename}"
        );
    }
    Ok(())
}

fn read_installation_identity(data_dir: &str) -> Result<Option<String>> {
    let path = std::path::Path::new(data_dir).join("server_metadata.json");
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let metadata: rhythm_os::storage::StoredServerMetadata = serde_json::from_slice(&bytes)?;
    anyhow::ensure!(
        (1..=2).contains(&metadata.schema_version),
        "Unsupported installation identity schema; recovery review required"
    );
    anyhow::ensure!(
        !metadata.server_instance_id.trim().is_empty()
            && metadata.server_instance_id.trim() == metadata.server_instance_id
            && metadata.server_instance_id.len() <= 256,
        "Invalid installation identity; recovery review required"
    );
    Ok(Some(metadata.server_instance_id))
}

fn install_factory_reset_hook(
    state: &SharedState,
    access: Arc<mobile_access::MobileAccess>,
    controller: Arc<ChildProcessRemoteAccessController>,
) -> Result<()> {
    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.before_factory_reset_fn = Some(Arc::new(move |state| {
        access.invalidate_enrollment();
        let caches = {
            let mut s = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
            s.light_breaker_enabled = false;
            s.managed_ha_lights = Some(std::collections::BTreeSet::new());
            s.invalidate_queued_light_dispatches();
            s.hubs
                .values()
                .filter_map(|hub| {
                    hub.data::<rhythm_ha::hub_state::HaHubData>()
                        .map(|ha| ha.event_routing_cache.clone())
                })
                .collect::<Vec<_>>()
        };
        for cache in caches {
            let mut cache = cache
                .lock()
                .map_err(|_| anyhow::anyhow!("HA routing lock"))?;
            cache.lights_ready = false;
            cache.reviewed.clear();
        }
        controller.shutdown()?;
        Ok(())
    }));
    state.after_factory_reset_fn = Some(Arc::new(|_| {
        std::thread::spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(1));
            info!(target: "sys", "Exiting after factory reset so the add-on supervisor can restart Rhythm");
            std::process::exit(1);
        });
        Ok(())
    }));
    Ok(())
}

async fn run_server(
    state: SharedState,
    access: Arc<mobile_access::MobileAccess>,
    controller: Arc<ChildProcessRemoteAccessController>,
) -> Result<()> {
    rhythm_os::state::capture_tokio_runtime_handle(&state);
    let mobile =
        tokio::net::TcpListener::bind((std::net::Ipv4Addr::UNSPECIFIED, MOBILE_PORT)).await?;
    let admin = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, ADMIN_PORT)).await?;
    info!(target: "sys", "Mobile API listening on port {}; internal admin on loopback port {}", MOBILE_PORT, ADMIN_PORT);
    rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);
    let remote_state = state.clone();
    tokio::task::spawn_blocking(move || {
        if let Err(error) = rhythm_os::remote_access::reconcile_remote_access_runtime(&remote_state)
        {
            warn!(target: "sys", "Remote access startup reconciliation failed: {error:#}");
        }
    });
    let mobile_server = http_server::create_mobile_router(state.clone(), access.clone());
    let admin_server = http_server::create_admin_router(state, access);
    let result = tokio::select! {
        result = axum::serve(mobile, mobile_server.into_make_service_with_connect_info::<std::net::SocketAddr>()) => result,
        result = axum::serve(admin, admin_server.into_make_service_with_connect_info::<std::net::SocketAddr>()) => result,
        _ = shutdown_signal() => Ok(()),
    };
    tokio::task::spawn_blocking(move || controller.shutdown()).await??;
    result?;
    Ok(())
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("SIGTERM handler");
        tokio::select! { _ = tokio::signal::ctrl_c() => {}, _ = terminate.recv() => {} }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_factory_reset_hook_registers_restart_callback() {
        let state: SharedState = Arc::new(Mutex::new(AppState::default()));

        let access = Arc::new(mobile_access::MobileAccess::new(&"a".repeat(64)).unwrap());
        let controller = Arc::new(ChildProcessRemoteAccessController::new("/missing"));
        install_factory_reset_hook(&state, access, controller).unwrap();

        assert!(state.lock().unwrap().after_factory_reset_fn.is_some());
    }

    #[test]
    fn reviewed_selection_preserves_enabled_intent_but_runtime_routing_starts_empty() {
        let root = std::env::temp_dir().join(format!(
            "rhythm-addon-selection-startup-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("settings.json"), "{}").unwrap();
        let mut state = AppState::default();
        state.light_breaker_enabled = true;
        restore_managed_selection(&mut state, root.to_str().unwrap()).unwrap();
        assert!(!state.light_breaker_enabled);
        std::fs::write(root.join("managed-ha-lights-v2.json"), serde_json::json!({
            "version":2,"lights":{"light.fixture":{"scope":"fixture-ha", "registry_id":"registry", "unique_id":"unique", "platform":"hue", "device_id":"device", "config_entry_id":"integration"}}
        }).to_string()).unwrap();
        state.light_breaker_enabled = true;
        restore_managed_selection(&mut state, root.to_str().unwrap()).unwrap();
        assert!(state.light_breaker_enabled);
        assert!(state.managed_ha_lights.as_ref().unwrap().is_empty());
        state.light_breaker_enabled = false;
        restore_managed_selection(&mut state, root.to_str().unwrap()).unwrap();
        assert!(!state.light_breaker_enabled);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn retained_credentials_require_the_original_valid_installation_identity() {
        let root =
            std::env::temp_dir().join(format!("rhythm-addon-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.to_str().unwrap();
        assert!(validate_security_stores(path).is_ok());
        std::fs::write(
            root.join("auth.json"),
            r#"{"schema_version":1,"tokens":[]}"#,
        )
        .unwrap();
        assert!(validate_security_stores(path).is_err());
        std::fs::write(root.join("server_metadata.json"), "{").unwrap();
        assert!(validate_security_stores(path).is_err());
        std::fs::write(
            root.join("server_metadata.json"),
            r#"{"schema_version":2,"server_instance_id":"original-installation"}"#,
        )
        .unwrap();
        assert!(validate_security_stores(path).is_ok());
        std::fs::write(root.join("auth.json"), "{").unwrap();
        assert!(validate_security_stores(path).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reset_barrier_revokes_enrollment_and_pauses_managed_lights() {
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().managed_ha_lights = Some(["light.fixture".into()].into());
        let access = Arc::new(mobile_access::MobileAccess::new(&"a".repeat(64)).unwrap());
        let controller = Arc::new(ChildProcessRemoteAccessController::new("/missing"));
        install_factory_reset_hook(&state, access.clone(), controller).unwrap();
        let reset = state
            .lock()
            .unwrap()
            .before_factory_reset_fn
            .clone()
            .unwrap();
        reset(&state).unwrap();
        assert!(access.is_resetting());
        assert!(!state.lock().unwrap().light_breaker_enabled);
        assert!(state
            .lock()
            .unwrap()
            .managed_ha_lights
            .as_ref()
            .unwrap()
            .is_empty());
    }
}
