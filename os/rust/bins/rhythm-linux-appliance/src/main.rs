//! Rhythm OS Linux appliance runtime.
//!
//! Reuses the native Linux/macOS server stack, but gives appliance
//! targets like rpiz their own binary crate so board-specific provisioning,
//! networking, and packaging concerns do not accumulate in `rhythm-server`.

mod ble_provision;
mod factory_reset;
mod http_server;
mod time_sync;
mod wifi;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use clap::Parser;
use log::{info, warn};
use rhythm_os::logging;
use rhythm_os::state::{AppState, PlatformConfig, SharedState, WorkItem};
use rhythm_os::storage::FileStorage;
use rhythm_server::hub;

const VERSION: &str = match option_env!("RHYTHM_BUILD_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};
const RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV: &str = "RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION";
const RHYTHM_DEV_MODE_ENV: &str = "RHYTHM_DEV_MODE";
const STARTUP_WIFI_RESTORE_TIMEOUT: Duration = Duration::from_secs(30);
const PERIODIC_WIFI_WAIT_POLL_INTERVAL: Duration = Duration::from_secs(5);
const CLOCK_SYNC_RETRY_INTERVAL: Duration = Duration::from_secs(15);
const CLOUDFLARED_BIN: &str = "/usr/bin/cloudflared";
const CLOUDFLARED_INIT: &str = "/etc/init.d/rhythm-cloudflared";
const CLOUDFLARED_PIDFILE: &str = "/var/run/rhythm-cloudflared.pid";
const CLOUDFLARED_CHILD_PIDFILE: &str = "/var/run/rhythm-cloudflared-child.pid";
const REMOTE_ACCESS_STARTUP_RECONCILE_DELAY: Duration = Duration::from_secs(60);

/// Rhythm OS Linux appliance.
#[derive(Parser, Debug)]
#[command(name = "rhythm-server", version = VERSION, about)]
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

fn beta_build_enables_matter_attestation_bypass(version: &str) -> bool {
    version.contains("-beta")
}

fn env_value_is_falsey(value: &str) -> bool {
    let value = value.trim();
    value == "0"
        || value.eq_ignore_ascii_case("false")
        || value.eq_ignore_ascii_case("no")
        || value.eq_ignore_ascii_case("off")
}

fn dev_mode_explicitly_disabled_value(value: Option<&str>) -> bool {
    value.is_some_and(env_value_is_falsey)
}

fn dev_mode_explicitly_disabled() -> bool {
    dev_mode_explicitly_disabled_value(std::env::var(RHYTHM_DEV_MODE_ENV).ok().as_deref())
}

fn set_env_default(name: &str, value: &str) -> bool {
    match std::env::var_os(name) {
        Some(existing) if !existing.is_empty() => false,
        _ => {
            std::env::set_var(name, value);
            true
        }
    }
}

fn apply_beta_build_defaults() -> bool {
    if !beta_build_enables_matter_attestation_bypass(VERSION) {
        return false;
    }
    if dev_mode_explicitly_disabled() {
        return false;
    }

    set_env_default(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV, "1")
}

fn main() -> Result<()> {
    let args = Args::parse();
    let beta_defaults_applied = apply_beta_build_defaults();
    let platform_type =
        std::env::var("RHYTHM_PLATFORM_TYPE").unwrap_or_else(|_| "appliance".to_string());
    let platform_context =
        std::env::var("RHYTHM_PLATFORM_CONTEXT").unwrap_or_else(|_| "rpiz".to_string());
    std::env::set_var("RHYTHM_PLATFORM_TYPE", &platform_type);
    std::env::set_var("RHYTHM_PLATFORM_CONTEXT", &platform_context);

    logging::init_native_logging(&args.log_level)?;

    info!(target: "sys", "Rhythm Linux Appliance v{} starting...", VERSION);
    if beta_defaults_applied {
        info!(
            target: "sys",
            "Applied beta appliance defaults: Matter device attestation bypass enabled"
        );
    }

    std::fs::create_dir_all(&args.data_dir)?;

    // Appliance bundle-only updates (drift repair, bundle releases without a
    // rootfs image) install over the running rootfs without an A/B slot
    // switch. Count this start attempt and restore the previous binaries if a
    // crash-looping build exhausts its probation; BusyBox init respawns us.
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
                "Self-update failed verification after repeated start attempts; restored {} previous binar{} — exiting so init restarts the previous build",
                restored.len(),
                if restored.len() == 1 { "y" } else { "ies" }
            );
            std::process::exit(1);
        }
    }

    // Sweep partial OTA artefacts left behind by a crashed prior run before
    // anything else touches the OTA staging directory. Safe to call even when
    // the directory doesn't exist yet.
    rhythm_server::self_update::cleanup_stale_downloads(std::path::Path::new(
        rhythm_server::self_update::APPLIANCE_OTA_STAGING_DIR_PATH,
    ));

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
        // Linux appliances are active platforms and should match the other
        // desktop/server-class runtimes for bootstrap behavior.
        s.platform = PlatformConfig::desktop();

        rhythm_os::storage::load_persisted_state(&mut s);

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
        s.hub_credentials_interceptor = Some(rhythm_os::hub::combined_credentials_interceptor(
            hub::INTEGRATIONS,
        ));
        s.request_hub_bootstrap_fn = Some(Arc::new(|state| {
            rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);
        }));
        s.commissioning_wifi_credentials_provider =
            Some(Arc::new(wifi::load_configured_credentials));
        s.remote_access_controller = Some(Arc::new(
            rhythm_os::remote_access::InitScriptRemoteAccessController::new(
                CLOUDFLARED_BIN,
                CLOUDFLARED_INIT,
                CLOUDFLARED_PIDFILE,
                CLOUDFLARED_CHILD_PIDFILE,
            ),
        ));
    }
    install_factory_reset_hook(&state)?;
    hydrate_persisted_wifi_credentials(&state);

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

    let periodic_gate_heartbeat = PeriodicGateHeartbeat::default();
    {
        let periodic_state = state.clone();
        let gate_heartbeat = periodic_gate_heartbeat.clone();
        std::thread::Builder::new()
            .name("periodic".to_string())
            .spawn(move || {
                run_periodic_when_clock_ready(periodic_state, gate_heartbeat);
            })
            .expect("Failed to spawn periodic thread");
    }
    rhythm_server::liveness::spawn_periodic_watchdog(state.clone());
    rhythm_server::auto_update::spawn(state.clone());

    let provisioning = ble_provision::ProvisioningManager::new(VERSION.to_string(), state.clone());
    if let Err(e) = provisioning.ensure_running_if_needed("startup") {
        warn!(
            target: "sys",
            "BLE provisioning sidecar did not start cleanly: {:#}",
            e
        );
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("rhythm-main-rt")
        .build()?
        .block_on(run_server(
            state,
            args.port,
            provisioning,
            periodic_gate_heartbeat,
        ))
}

fn install_factory_reset_hook(state: &SharedState) -> Result<()> {
    let (data_dir, firmware_version) = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        (state.data_dir.clone(), state.firmware_version.to_string())
    };

    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.after_factory_reset_fn = Some(Arc::new(move |_| {
        let data_dir = data_dir.clone();
        let firmware_version = firmware_version.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(1));

            if let Err(error) = wifi::clear_credentials() {
                warn!(
                    target: "sys",
                    "Failed to clear appliance Wi-Fi credentials during factory reset: {:#}",
                    error
                );
            }

            if let Err(error) =
                factory_reset::scrub_appliance_factory_reset_state(&data_dir, &firmware_version)
            {
                warn!(
                    target: "sys",
                    "Failed to scrub appliance OTA/log state during factory reset: {:#}",
                    error
                );
            }

            info!(target: "sys", "Rebooting appliance after factory reset...");
            rhythm_server::self_update::schedule_factory_reset_restart();
        });
    }));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupWifiRestoreAction {
    SkipNoKnownCredentials,
    SkipAlreadyConnected,
    RestorePersistedCredentials,
    StoreSystemConfigCredentials,
    RestoreSystemConfigCredentials,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeriodicStartupAction {
    StartImmediately,
    WaitForWifi,
    WaitForClockSync,
}

/// Liveness signal from the periodic startup gate.
///
/// The periodic loop is intentionally gated on Wi-Fi and clock sync, both of
/// which are environment conditions rather than image health. The gate thread
/// beats this heartbeat on every poll so the boot-success marker can tell
/// "periodic loop alive but waiting on the environment" apart from "periodic
/// thread died".
#[derive(Clone, Default)]
struct PeriodicGateHeartbeat(Arc<Mutex<Option<Instant>>>);

impl PeriodicGateHeartbeat {
    fn beat(&self) {
        if let Ok(mut last) = self.0.lock() {
            *last = Some(Instant::now());
        }
    }

    fn is_recent(&self, within: Duration) -> bool {
        self.0
            .lock()
            .ok()
            .and_then(|last| *last)
            .map(|at| at.elapsed() <= within)
            .unwrap_or(false)
    }

    #[cfg(test)]
    fn set_beat_at(&self, at: Instant) {
        *self.0.lock().unwrap() = Some(at);
    }
}

fn startup_wifi_restore_action(
    has_stored_credentials: bool,
    has_system_config_credentials: bool,
    has_active_connection: bool,
) -> StartupWifiRestoreAction {
    if has_stored_credentials && has_active_connection {
        StartupWifiRestoreAction::SkipAlreadyConnected
    } else if has_stored_credentials {
        StartupWifiRestoreAction::RestorePersistedCredentials
    } else if has_system_config_credentials && has_active_connection {
        StartupWifiRestoreAction::StoreSystemConfigCredentials
    } else if has_system_config_credentials {
        StartupWifiRestoreAction::RestoreSystemConfigCredentials
    } else {
        StartupWifiRestoreAction::SkipNoKnownCredentials
    }
}

fn periodic_startup_action(
    has_active_connection: bool,
    clock_ready: bool,
) -> PeriodicStartupAction {
    if !has_active_connection {
        PeriodicStartupAction::WaitForWifi
    } else if !clock_ready {
        PeriodicStartupAction::WaitForClockSync
    } else {
        PeriodicStartupAction::StartImmediately
    }
}

fn run_periodic_when_clock_ready(state: SharedState, gate_heartbeat: PeriodicGateHeartbeat) {
    let mut last_action = None;
    let mut next_sync_retry_at = Instant::now();

    loop {
        gate_heartbeat.beat();
        let action = periodic_startup_action(
            wifi::has_active_connection(),
            time_sync::system_clock_is_sane(),
        );

        if last_action != Some(action) {
            match action {
                PeriodicStartupAction::StartImmediately => info!(
                    target: "sys",
                    "Startup prerequisites satisfied; starting periodic loop"
                ),
                PeriodicStartupAction::WaitForWifi => info!(
                    target: "sys",
                    "Delaying periodic loop start until appliance has an active Wi-Fi IP"
                ),
                PeriodicStartupAction::WaitForClockSync => info!(
                    target: "sys",
                    "Delaying periodic loop start until appliance wall clock is synchronized"
                ),
            }
            last_action = Some(action);
        }

        match action {
            PeriodicStartupAction::StartImmediately => break,
            PeriodicStartupAction::WaitForWifi => {
                std::thread::sleep(PERIODIC_WIFI_WAIT_POLL_INTERVAL);
            }
            PeriodicStartupAction::WaitForClockSync => {
                let now = Instant::now();
                if now >= next_sync_retry_at {
                    if let Err(error) = time_sync::sync_system_clock("periodic startup gate") {
                        warn!(
                            target: "sys",
                            "Explicit wall-clock sync attempt failed: {:#}",
                            error
                        );
                    }
                    next_sync_retry_at = Instant::now() + CLOCK_SYNC_RETRY_INTERVAL;
                    continue;
                }

                std::thread::sleep(PERIODIC_WIFI_WAIT_POLL_INTERVAL);
            }
        }
    }

    gate_heartbeat.beat();
    rhythm_os::periodic::run_periodic_loop(state, None::<fn()>);
}

fn hydrate_persisted_wifi_credentials(state: &SharedState) {
    let stored_credentials = match state.lock() {
        Ok(state) => match state
            .storage
            .as_ref()
            .map(|storage| storage.load_commissioning_wifi_credentials())
        {
            Some(Ok(creds)) => creds,
            Some(Err(error)) => {
                warn!(
                    target: "sys",
                    "Failed to load persisted appliance Wi-Fi credentials at startup: {:#}",
                    error
                );
                return;
            }
            None => return,
        },
        Err(_) => {
            warn!(
                target: "sys",
                "Skipping appliance Wi-Fi hydration at startup: state lock poisoned"
            );
            return;
        }
    };

    let system_config_credentials = if stored_credentials.is_none() {
        match wifi::load_configured_credentials() {
            Ok(creds) => creds,
            Err(error) => {
                warn!(
                    target: "sys",
                    "Failed to load appliance Wi-Fi credentials from system config at startup: {:#}",
                    error
                );
                None
            }
        }
    } else {
        None
    };

    let action = startup_wifi_restore_action(
        stored_credentials.is_some(),
        system_config_credentials.is_some(),
        wifi::has_active_connection(),
    );

    let creds = match action {
        StartupWifiRestoreAction::SkipNoKnownCredentials
        | StartupWifiRestoreAction::SkipAlreadyConnected => return,
        StartupWifiRestoreAction::StoreSystemConfigCredentials => {
            let Some(creds) = system_config_credentials else {
                return;
            };
            save_commissioning_wifi_credentials(state, &creds, "system Wi-Fi config");
            return;
        }
        StartupWifiRestoreAction::RestorePersistedCredentials => {
            let Some(creds) = stored_credentials else {
                return;
            };
            creds
        }
        StartupWifiRestoreAction::RestoreSystemConfigCredentials => {
            let Some(creds) = system_config_credentials else {
                return;
            };
            save_commissioning_wifi_credentials(state, &creds, "system Wi-Fi config");
            creds
        }
    };

    info!(
        target: "sys",
        "No active Wi-Fi IP detected at startup; restoring appliance Wi-Fi credentials for SSID '{}'",
        creds.ssid
    );

    match wifi::connect_with_credentials(&creds, STARTUP_WIFI_RESTORE_TIMEOUT) {
        Ok(ip) => {
            info!(
                target: "sys",
                "Restored appliance Wi-Fi connectivity from persisted credentials (SSID='{}', ip={})",
                creds.ssid,
                ip
            );
            if let Err(error) =
                time_sync::sync_system_clock("restored persisted appliance Wi-Fi connectivity")
            {
                warn!(
                    target: "sys",
                    "Failed to sync wall clock after restoring persisted Wi-Fi credentials: {:#}",
                    error
                );
            }
        }
        Err(error) => warn!(
            target: "sys",
            "Failed to restore appliance Wi-Fi from persisted credentials for SSID '{}': {:#}",
            creds.ssid,
            error
        ),
    }
}

fn save_commissioning_wifi_credentials(
    state: &SharedState,
    creds: &rhythm_os::provisioning::WifiCredentials,
    source: &str,
) {
    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    match result {
        Ok(()) => info!(
            target: "sys",
            "Persisted Matter commissioning Wi-Fi credentials from {} for SSID '{}'",
            source,
            creds.ssid
        ),
        Err(error) => warn!(
            target: "sys",
            "Failed to persist Matter commissioning Wi-Fi credentials from {} for SSID '{}': {:#}",
            source,
            creds.ssid,
            error
        ),
    }
}

async fn run_server(
    state: SharedState,
    port: u16,
    provisioning: ble_provision::ProvisioningManager,
    periodic_gate_heartbeat: PeriodicGateHeartbeat,
) -> Result<()> {
    let addr = format!("0.0.0.0:{}", port);
    info!(target: "sys", "Starting HTTP server on {}", addr);

    let server = http_server::create_router(state.clone(), provisioning);
    let listener = tokio::net::TcpListener::bind(&addr).await.with_context(|| {
        format!(
            "Failed to bind to {} — is another process using this port?",
            addr
        )
    })?;
    info!(target: "sys", "Rhythm Linux Appliance listening on http://{}", addr);

    // The listener is up: give the (possibly freshly updated) build its
    // post-startup grace period, then discard self-update rollback backups.
    rhythm_server::self_update::spawn_update_verification_marker();

    // Keep the existing server-style mDNS identity but tack on the rpiz serial
    // number so two units on the same LAN never collide on the IP-derived
    // suffix.
    let device_id = read_rpiz_serial_suffix();
    let _mdns = rhythm_os::mdns::register_mdns_service_with_id(
        port,
        "server",
        device_id.as_deref(),
        VERSION,
        "rhythm-server",
    );

    rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);
    spawn_boot_success_marker(state.clone(), periodic_gate_heartbeat);
    spawn_remote_access_startup_reconcile(state.clone());

    axum::serve(
        listener,
        server.into_make_service_with_connect_info::<std::net::SocketAddr>(),
    )
    .await?;

    Ok(())
}

fn spawn_remote_access_startup_reconcile(state: SharedState) {
    std::thread::Builder::new()
        .name("remote-access-startup".to_string())
        .spawn(move || {
            std::thread::sleep(REMOTE_ACCESS_STARTUP_RECONCILE_DELAY);
            match rhythm_os::remote_access::reconcile_remote_access_runtime(&state) {
                Ok(()) => info!(
                    target: "sys",
                    "Remote access runtime reconciled after appliance startup delay"
                ),
                Err(error) => warn!(
                    target: "sys",
                    "Remote access runtime did not reconcile after appliance startup delay: {:#}",
                    error
                ),
            }
        })
        .expect("Failed to spawn remote-access startup reconcile thread");
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BootSuccessHealth {
    Ready,
    Waiting(&'static str),
}

/// How recently the periodic startup gate must have beaten its heartbeat for
/// the periodic thread to count as alive. The gate polls every 5s, so 60s of
/// silence means the thread is gone, not just slow.
const PERIODIC_GATE_STALE_AFTER: Duration = Duration::from_secs(60);

/// Boot success means *device* success, not environment success.
///
/// A freshly flashed OTA slot is healthy when the process is serving (the
/// marker thread only exists after the HTTP listener bound) and the periodic
/// machinery is alive — either actually ticking, or deliberately gated on
/// Wi-Fi/clock sync. Hub reachability and Wi-Fi state are environment
/// conditions: rolling back a good image because the user's bridge or router
/// is offline strands the appliance on old firmware and burns a full rootfs
/// flash per release.
fn boot_success_health(
    state: &SharedState,
    gate_heartbeat: &PeriodicGateHeartbeat,
) -> BootSuccessHealth {
    let Ok(s) = state.lock() else {
        return BootSuccessHealth::Waiting("state_lock_poisoned");
    };

    if s.last_check_instant.is_some() {
        return BootSuccessHealth::Ready;
    }
    drop(s);

    if gate_heartbeat.is_recent(PERIODIC_GATE_STALE_AFTER) {
        return BootSuccessHealth::Ready;
    }

    BootSuccessHealth::Waiting("periodic_not_ready")
}

fn spawn_boot_success_marker(state: SharedState, gate_heartbeat: PeriodicGateHeartbeat) {
    const BOOTSTATE_SCRIPT: &str = "/etc/init.d/S41bootstate";
    const GRACE_SECS: u64 = 30;
    const POLL_SECS: u64 = 5;
    const TIMEOUT_SECS: u64 = 10 * 60;

    if !std::path::Path::new(BOOTSTATE_SCRIPT).exists() {
        return;
    }

    info!(
        target: "sys",
        "Will mark OTA slot as last-good via {} success after startup health is ready (grace={}s, timeout={}s)",
        BOOTSTATE_SCRIPT,
        GRACE_SECS,
        TIMEOUT_SECS
    );

    std::thread::Builder::new()
        .name("boot-success-marker".to_string())
        .spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(GRACE_SECS));
            let started = Instant::now();
            let mut last_reason: Option<&'static str> = None;
            loop {
                match boot_success_health(&state, &gate_heartbeat) {
                    BootSuccessHealth::Ready => {
                        run_bootstate_script_action(BOOTSTATE_SCRIPT, "success");
                        return;
                    }
                    BootSuccessHealth::Waiting(reason) => {
                        if last_reason != Some(reason) {
                            info!(
                                target: "sys",
                                "Waiting to mark OTA boot success: {}",
                                reason
                            );
                            last_reason = Some(reason);
                        }
                        if started.elapsed() >= std::time::Duration::from_secs(TIMEOUT_SECS) {
                            warn!(
                                target: "sys",
                                "OTA boot health did not become ready within {}s; requesting bootstate rollback (last reason: {})",
                                TIMEOUT_SECS,
                                reason
                            );
                            run_bootstate_script_action(BOOTSTATE_SCRIPT, "fail");
                            return;
                        }
                    }
                }
                std::thread::sleep(std::time::Duration::from_secs(POLL_SECS));
            }
        })
        .expect("Failed to spawn boot-success marker thread");
}

fn run_bootstate_script_action(script: &str, action: &str) {
    match std::process::Command::new(script).arg(action).status() {
        Ok(status) if status.success() => {
            info!(target: "sys", "OTA bootstate action '{}' succeeded", action);
        }
        Ok(status) => warn!(
            target: "sys",
            "{} {} exited with {}",
            script,
            action,
            status
        ),
        Err(e) => warn!(
            target: "sys",
            "Failed to exec {} {}: {}",
            script,
            action,
            e
        ),
    }
}

/// Read the rpiz board serial number and extract a short alphanumeric suffix
/// suitable for mDNS hostnames. `None` if the serial file is missing or
/// yields an empty / non-ASCII result — callers should fall back to the
/// IP-derived suffix in that case.
fn read_rpiz_serial_suffix() -> Option<String> {
    let raw = std::fs::read_to_string("/proc/device-tree/serial-number").ok()?;
    extract_serial_suffix(&raw)
}

fn extract_serial_suffix(raw: &str) -> Option<String> {
    let cleaned: String = raw.chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    if cleaned.len() < 4 {
        return None;
    }
    // Use the last 8 hex chars — short enough to keep the hostname readable,
    // long enough that collisions are astronomically unlikely for units
    // shipped with distinct Pi serials.
    let tail = &cleaned[cleaned.len().saturating_sub(8)..];
    Some(tail.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        apply_beta_build_defaults, beta_build_enables_matter_attestation_bypass,
        boot_success_health, dev_mode_explicitly_disabled, dev_mode_explicitly_disabled_value,
        env_value_is_falsey, extract_serial_suffix, install_factory_reset_hook,
        periodic_startup_action, run_bootstate_script_action, save_commissioning_wifi_credentials,
        set_env_default, startup_wifi_restore_action, BootSuccessHealth, PeriodicStartupAction,
        StartupWifiRestoreAction, RHYTHM_DEV_MODE_ENV, RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV,
        STARTUP_WIFI_RESTORE_TIMEOUT, VERSION,
    };
    use crate::time_sync::clock_is_sane_at;
    use chrono::{TimeZone, Utc};
    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::hub::{ActiveHub, HubCredentials, HubType};
    use rhythm_os::provisioning::WifiCredentials;
    use rhythm_os::state::{AppState, SharedState};
    use rhythm_os::storage::{FileStorage, Storage};
    use std::ffi::OsString;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvRestore {
        values: Vec<(&'static str, Option<OsString>)>,
    }

    impl EnvRestore {
        fn new(names: &[&'static str]) -> Self {
            Self {
                values: names
                    .iter()
                    .map(|name| (*name, std::env::var_os(name)))
                    .collect(),
            }
        }
    }

    impl Drop for EnvRestore {
        fn drop(&mut self) {
            for (name, value) in &self.values {
                match value {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-appliance-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn extract_serial_suffix_takes_last_eight_hex_chars() {
        assert_eq!(
            extract_serial_suffix("10000000abcdef01\0").as_deref(),
            Some("abcdef01")
        );
    }

    #[test]
    fn extract_serial_suffix_strips_nulls_and_whitespace() {
        assert_eq!(
            extract_serial_suffix("  1000000012345678\n\0").as_deref(),
            Some("12345678")
        );
    }

    #[test]
    fn extract_serial_suffix_returns_none_when_too_short() {
        assert_eq!(extract_serial_suffix(""), None);
        assert_eq!(extract_serial_suffix("abc"), None);
        assert_eq!(extract_serial_suffix("\0\0\0"), None);
    }

    #[test]
    fn extract_serial_suffix_handles_short_but_valid_serial() {
        assert_eq!(extract_serial_suffix("abcd").as_deref(), Some("abcd"));
        assert_eq!(extract_serial_suffix("abcdef").as_deref(), Some("abcdef"));
    }

    #[test]
    fn extract_serial_suffix_ignores_non_ascii_characters() {
        // Non-ASCII (`ç`, `é`) is dropped before length check; the remaining
        // ASCII chars are "a123456789f" (11 chars), so the last-8 suffix is
        // "3456789f".
        assert_eq!(
            extract_serial_suffix("ça123456789fé"),
            Some("3456789f".to_string())
        );
    }

    #[test]
    fn env_default_helpers_treat_empty_values_as_unset_and_preserve_existing_values() {
        let _lock = ENV_LOCK.lock().unwrap();
        let key = "RHYTHM_TEST_APPLIANCE_ENV_DEFAULT";
        let _restore = EnvRestore::new(&[key]);

        std::env::remove_var(key);
        assert!(set_env_default(key, "one"));
        assert_eq!(std::env::var(key).as_deref(), Ok("one"));

        std::env::set_var(key, "already-set");
        assert!(!set_env_default(key, "two"));
        assert_eq!(std::env::var(key).as_deref(), Ok("already-set"));

        std::env::set_var(key, "");
        assert!(set_env_default(key, "filled"));
        assert_eq!(std::env::var(key).as_deref(), Ok("filled"));
    }

    #[test]
    fn dev_mode_falsey_helpers_trim_and_ignore_case() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new(&[RHYTHM_DEV_MODE_ENV]);

        for value in ["0", " false ", "NO", "off"] {
            assert!(env_value_is_falsey(value), "{value:?} should be falsey");
            std::env::set_var(RHYTHM_DEV_MODE_ENV, value);
            assert!(dev_mode_explicitly_disabled());
        }

        for value in ["", "1", "true", "dev"] {
            assert!(!env_value_is_falsey(value), "{value:?} should stay truthy");
            std::env::set_var(RHYTHM_DEV_MODE_ENV, value);
            assert!(!dev_mode_explicitly_disabled());
        }
    }

    #[test]
    fn beta_default_application_respects_version_env_and_existing_values() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _restore = EnvRestore::new(&[
            RHYTHM_DEV_MODE_ENV,
            RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV,
        ]);

        std::env::remove_var(RHYTHM_DEV_MODE_ENV);
        std::env::remove_var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV);
        let applied = apply_beta_build_defaults();
        if beta_build_enables_matter_attestation_bypass(VERSION) {
            assert!(applied);
            assert_eq!(
                std::env::var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV).as_deref(),
                Ok("1")
            );
        } else {
            assert!(!applied);
            assert!(std::env::var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV).is_err());
        }

        std::env::set_var(RHYTHM_DEV_MODE_ENV, "0");
        std::env::remove_var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV);
        assert!(!apply_beta_build_defaults());
        assert!(std::env::var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV).is_err());

        std::env::remove_var(RHYTHM_DEV_MODE_ENV);
        std::env::set_var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV, "manual");
        assert!(!apply_beta_build_defaults());
        assert_eq!(
            std::env::var(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV).as_deref(),
            Ok("manual")
        );
    }

    #[test]
    fn startup_wifi_restore_skips_when_no_credentials_are_stored() {
        assert_eq!(
            startup_wifi_restore_action(false, false, false),
            StartupWifiRestoreAction::SkipNoKnownCredentials
        );
    }

    #[test]
    fn startup_wifi_restore_skips_when_wifi_is_already_connected() {
        assert_eq!(
            startup_wifi_restore_action(true, false, true),
            StartupWifiRestoreAction::SkipAlreadyConnected
        );
    }

    #[test]
    fn startup_wifi_restore_uses_persisted_credentials_when_disconnected() {
        assert_eq!(
            startup_wifi_restore_action(true, false, false),
            StartupWifiRestoreAction::RestorePersistedCredentials
        );
        assert_eq!(STARTUP_WIFI_RESTORE_TIMEOUT.as_secs(), 30);
    }

    #[test]
    fn startup_wifi_restore_stores_system_credentials_when_already_connected() {
        assert_eq!(
            startup_wifi_restore_action(false, true, true),
            StartupWifiRestoreAction::StoreSystemConfigCredentials
        );
    }

    #[test]
    fn startup_wifi_restore_uses_system_credentials_when_disconnected() {
        assert_eq!(
            startup_wifi_restore_action(false, true, false),
            StartupWifiRestoreAction::RestoreSystemConfigCredentials
        );
    }

    #[test]
    fn periodic_startup_starts_immediately_when_wifi_is_connected() {
        assert_eq!(
            periodic_startup_action(true, true),
            PeriodicStartupAction::StartImmediately
        );
    }

    #[test]
    fn periodic_startup_waits_for_wifi_when_disconnected() {
        assert_eq!(
            periodic_startup_action(false, true),
            PeriodicStartupAction::WaitForWifi
        );
    }

    #[test]
    fn periodic_startup_waits_for_clock_sync_when_wifi_is_connected_but_clock_is_unsynced() {
        assert_eq!(
            periodic_startup_action(true, false),
            PeriodicStartupAction::WaitForClockSync
        );
    }

    #[test]
    fn appliance_clock_sanity_matches_expected_boot_vs_real_time_behavior() {
        let boot_epoch = Utc.timestamp_opt(15, 0).single().unwrap();
        let synced_time = Utc
            .with_ymd_and_hms(2026, 4, 23, 19, 41, 2)
            .single()
            .unwrap();
        assert!(!clock_is_sane_at(boot_epoch));
        assert!(clock_is_sane_at(synced_time));
    }

    fn test_state() -> SharedState {
        Arc::new(Mutex::new(AppState::default()))
    }

    fn mark_periodic_ready(state: &SharedState) {
        state.lock().unwrap().last_check_instant = Some(Instant::now());
    }

    #[test]
    fn install_factory_reset_hook_captures_state_and_registers_callback() {
        let root = unique_test_dir("factory-reset-hook");
        let state = test_state();
        {
            let mut s = state.lock().unwrap();
            s.data_dir = root.display().to_string();
            s.firmware_version = "0.4.251-beta";
        }

        install_factory_reset_hook(&state).unwrap();

        assert!(state.lock().unwrap().after_factory_reset_fn.is_some());
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn save_commissioning_wifi_credentials_persists_when_storage_is_configured() {
        let root = unique_test_dir("wifi-save");
        let state = test_state();
        state.lock().unwrap().storage =
            Some(Box::new(FileStorage::new(root.to_str().unwrap()).unwrap()));
        let creds = WifiCredentials {
            ssid: "Kitchen AP".to_string(),
            password: "correct horse battery staple".to_string(),
        };

        save_commissioning_wifi_credentials(&state, &creds, "test");

        let loaded = FileStorage::new(root.to_str().unwrap())
            .unwrap()
            .load_commissioning_wifi_credentials()
            .unwrap();
        assert_eq!(loaded, Some(creds));
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn save_commissioning_wifi_credentials_handles_missing_storage_without_panicking() {
        let state = test_state();
        let creds = WifiCredentials {
            ssid: "No Storage".to_string(),
            password: "password".to_string(),
        };

        save_commissioning_wifi_credentials(&state, &creds, "test");
        assert!(state.lock().unwrap().storage.is_none());
    }

    fn idle_heartbeat() -> super::PeriodicGateHeartbeat {
        super::PeriodicGateHeartbeat::default()
    }

    #[test]
    fn boot_success_health_waits_when_periodic_never_started() {
        let state = test_state();

        assert_eq!(
            boot_success_health(&state, &idle_heartbeat()),
            BootSuccessHealth::Waiting("periodic_not_ready")
        );
    }

    #[test]
    fn boot_success_health_ready_once_periodic_loop_ticks() {
        let state = test_state();
        mark_periodic_ready(&state);

        assert_eq!(
            boot_success_health(&state, &idle_heartbeat()),
            BootSuccessHealth::Ready
        );
    }

    #[test]
    fn boot_success_health_ready_while_periodic_gate_waits_on_environment() {
        // The periodic loop is gated on Wi-Fi/clock sync. A live gate thread
        // means the image is healthy even though nothing has ticked yet.
        let state = test_state();
        let heartbeat = idle_heartbeat();
        heartbeat.beat();

        assert_eq!(
            boot_success_health(&state, &heartbeat),
            BootSuccessHealth::Ready
        );
    }

    #[test]
    fn boot_success_health_waits_when_gate_heartbeat_is_stale() {
        let state = test_state();
        let heartbeat = idle_heartbeat();
        heartbeat
            .set_beat_at(Instant::now() - super::PERIODIC_GATE_STALE_AFTER - Duration::from_secs(1));

        assert_eq!(
            boot_success_health(&state, &heartbeat),
            BootSuccessHealth::Waiting("periodic_not_ready")
        );
    }

    #[test]
    fn boot_success_health_ignores_hub_connectivity() {
        // Boot success is device success: a configured-but-unreachable hub
        // (offline bridge, retired hardware) must not roll back a healthy
        // image. Regression guard for the rollback-per-release failure mode.
        let state = test_state();
        mark_periodic_ready(&state);
        let hub_type = HubType::new("hue");
        let key = HubKey::new(hub_type.clone(), "192.0.2.10");
        let credentials = HubCredentials::new("hue", "192.0.2.10", serde_json::json!({}));
        let hub = ActiveHub {
            hub_type,
            hub_key: key.clone(),
            runtime: None,
            hub_data: Box::new(()),
            registry: None,
            discovery: None,
            shutdown: Arc::new(AtomicBool::new(false)),
        };
        {
            let mut s = state.lock().unwrap();
            s.hub_credentials.insert(key.clone(), credentials);
            s.hubs.insert(key.clone(), hub);
            s.begin_hub_bootstrap_worker();
            s.begin_hub_sync(&key);
            // Hub explicitly disconnected, bootstrap and sync still running.
            s.set_hub_connected(&key, false);
        }

        assert_eq!(
            boot_success_health(&state, &idle_heartbeat()),
            BootSuccessHealth::Ready
        );
    }

    #[test]
    fn periodic_gate_heartbeat_recency() {
        let heartbeat = idle_heartbeat();
        assert!(!heartbeat.is_recent(Duration::from_secs(60)));

        heartbeat.beat();
        assert!(heartbeat.is_recent(Duration::from_secs(60)));

        heartbeat.set_beat_at(Instant::now() - Duration::from_secs(120));
        assert!(!heartbeat.is_recent(Duration::from_secs(60)));
    }

    #[test]
    fn run_bootstate_script_action_handles_success_failure_and_missing_script() {
        let root = unique_test_dir("bootstate-script");
        let script = root.join("bootstate.sh");
        std::fs::write(
            &script,
            "#!/bin/sh\nif [ \"$1\" = success ]; then exit 0; fi\nexit 7\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();
        let script = script.to_str().unwrap();

        run_bootstate_script_action(script, "success");
        run_bootstate_script_action(script, "fail");
        run_bootstate_script_action("/tmp/rhythm-definitely-missing-bootstate", "success");

        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn beta_builds_enable_matter_attestation_bypass_defaults() {
        assert!(beta_build_enables_matter_attestation_bypass("0.4.160-beta"));
        assert!(beta_build_enables_matter_attestation_bypass(
            "0.4.160-beta.dev.5.gabcdef"
        ));
    }

    #[test]
    fn stable_builds_do_not_enable_matter_attestation_bypass_defaults() {
        assert!(!beta_build_enables_matter_attestation_bypass("0.4.160"));
    }

    #[test]
    fn explicit_prod_mode_disables_beta_matter_attestation_bypass_defaults() {
        assert!(dev_mode_explicitly_disabled_value(Some("0")));
        assert!(dev_mode_explicitly_disabled_value(Some("false")));
        assert!(dev_mode_explicitly_disabled_value(Some("OFF")));
        assert!(!dev_mode_explicitly_disabled_value(None));
        assert!(!dev_mode_explicitly_disabled_value(Some("1")));
    }
}
