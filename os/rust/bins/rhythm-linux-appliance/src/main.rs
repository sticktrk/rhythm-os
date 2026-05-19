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

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
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
        s.hub_capabilities = callbacks.hub_capabilities.clone();
        s.hub_credentials_interceptor = Some(rhythm_os::hub::combined_credentials_interceptor(
            hub::INTEGRATIONS,
        ));
        s.request_hub_bootstrap_fn = Some(Arc::new(|state| {
            rhythm_os::hub::spawn_stored_hub_bootstrap(state.clone(), hub::INTEGRATIONS);
        }));
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

    {
        let periodic_state = state.clone();
        std::thread::Builder::new()
            .name("periodic".to_string())
            .spawn(move || {
                run_periodic_when_clock_ready(periodic_state);
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
        .block_on(run_server(state, args.port, provisioning))
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

            let reboot_result = Command::new("/sbin/reboot")
                .status()
                .or_else(|_| Command::new("reboot").status());

            match reboot_result {
                Ok(status) if status.success() => {}
                Ok(status) => {
                    warn!(
                        target: "sys",
                        "Appliance reboot after factory reset exited with status {:?}; falling back to process exit",
                        status.code()
                    );
                    std::process::exit(1);
                }
                Err(error) => {
                    warn!(
                        target: "sys",
                        "Failed to reboot appliance after factory reset: {:#}; falling back to process exit",
                        error
                    );
                    std::process::exit(1);
                }
            }
        });
    }));
    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StartupWifiRestoreAction {
    SkipNoStoredCredentials,
    SkipAlreadyConnected,
    RestorePersistedCredentials,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PeriodicStartupAction {
    StartImmediately,
    WaitForWifi,
    WaitForClockSync,
}

fn startup_wifi_restore_action(
    has_stored_credentials: bool,
    has_active_connection: bool,
) -> StartupWifiRestoreAction {
    if !has_stored_credentials {
        StartupWifiRestoreAction::SkipNoStoredCredentials
    } else if has_active_connection {
        StartupWifiRestoreAction::SkipAlreadyConnected
    } else {
        StartupWifiRestoreAction::RestorePersistedCredentials
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

fn run_periodic_when_clock_ready(state: SharedState) {
    let mut last_action = None;
    let mut next_sync_retry_at = Instant::now();

    loop {
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

    match startup_wifi_restore_action(stored_credentials.is_some(), wifi::has_active_connection()) {
        StartupWifiRestoreAction::SkipNoStoredCredentials
        | StartupWifiRestoreAction::SkipAlreadyConnected => return,
        StartupWifiRestoreAction::RestorePersistedCredentials => {}
    }

    let Some(creds) = stored_credentials else {
        return;
    };

    info!(
        target: "sys",
        "No active Wi-Fi IP detected at startup; restoring persisted appliance Wi-Fi credentials for SSID '{}'",
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
    info!(target: "sys", "Rhythm Linux Appliance listening on http://{}", addr);

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
    spawn_boot_success_marker(state.clone());

    axum::serve(listener, server).await?;

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum BootSuccessHealth {
    Ready,
    Waiting(&'static str),
}

fn boot_success_health(state: &SharedState) -> BootSuccessHealth {
    let Ok(s) = state.lock() else {
        return BootSuccessHealth::Waiting("state_lock_poisoned");
    };

    if s.last_check_instant.is_none() {
        return BootSuccessHealth::Waiting("periodic_not_ready");
    }
    if s.hub_bootstrap_worker_running {
        return BootSuccessHealth::Waiting("hub_bootstrap_running");
    }
    if !s.hub_sync_in_progress.is_empty() {
        return BootSuccessHealth::Waiting("hub_sync_in_progress");
    }

    for (key, credentials) in &s.hub_credentials {
        if !credentials.can_connect() {
            continue;
        }
        if !s.hubs.contains_key(key) {
            return BootSuccessHealth::Waiting("hub_not_active");
        }
        if !s.hub_is_connected(key) {
            return BootSuccessHealth::Waiting("hub_not_connected");
        }
    }

    BootSuccessHealth::Ready
}

fn spawn_boot_success_marker(state: SharedState) {
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
                match boot_success_health(&state) {
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
        beta_build_enables_matter_attestation_bypass, boot_success_health,
        dev_mode_explicitly_disabled_value, extract_serial_suffix, periodic_startup_action,
        startup_wifi_restore_action, BootSuccessHealth, PeriodicStartupAction,
        StartupWifiRestoreAction, STARTUP_WIFI_RESTORE_TIMEOUT,
    };
    use crate::time_sync::clock_is_sane_at;
    use chrono::{TimeZone, Utc};
    use rhythm_os::canonical::identity::HubKey;
    use rhythm_os::hub::{ActiveHub, HubCredentials, HubType};
    use rhythm_os::state::{AppState, SharedState};
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};
    use std::time::Instant;

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
    fn startup_wifi_restore_skips_when_no_credentials_are_stored() {
        assert_eq!(
            startup_wifi_restore_action(false, false),
            StartupWifiRestoreAction::SkipNoStoredCredentials
        );
    }

    #[test]
    fn startup_wifi_restore_skips_when_wifi_is_already_connected() {
        assert_eq!(
            startup_wifi_restore_action(true, true),
            StartupWifiRestoreAction::SkipAlreadyConnected
        );
    }

    #[test]
    fn startup_wifi_restore_uses_persisted_credentials_when_disconnected() {
        assert_eq!(
            startup_wifi_restore_action(true, false),
            StartupWifiRestoreAction::RestorePersistedCredentials
        );
        assert_eq!(STARTUP_WIFI_RESTORE_TIMEOUT.as_secs(), 30);
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
    fn boot_success_health_waits_for_periodic_loop() {
        let state = test_state();

        assert_eq!(
            boot_success_health(&state),
            BootSuccessHealth::Waiting("periodic_not_ready")
        );
    }

    #[test]
    fn boot_success_health_ready_without_connectable_hubs_after_periodic() {
        let state = test_state();
        mark_periodic_ready(&state);

        assert_eq!(boot_success_health(&state), BootSuccessHealth::Ready);
    }

    #[test]
    fn boot_success_health_waits_for_hub_bootstrap_worker() {
        let state = test_state();
        mark_periodic_ready(&state);
        state.lock().unwrap().begin_hub_bootstrap_worker();

        assert_eq!(
            boot_success_health(&state),
            BootSuccessHealth::Waiting("hub_bootstrap_running")
        );
    }

    #[test]
    fn boot_success_health_waits_for_configured_hub_to_activate() {
        let state = test_state();
        mark_periodic_ready(&state);
        let credentials = HubCredentials::new("hue", "192.0.2.10", serde_json::json!({}));
        let key = credentials.hub_key().unwrap();
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key, credentials);

        assert_eq!(
            boot_success_health(&state),
            BootSuccessHealth::Waiting("hub_not_active")
        );
    }

    #[test]
    fn boot_success_health_waits_for_active_hub_connection() {
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
        }

        assert_eq!(
            boot_success_health(&state),
            BootSuccessHealth::Waiting("hub_not_connected")
        );

        state.lock().unwrap().set_hub_connected(&key, true);
        assert_eq!(boot_success_health(&state), BootSuccessHealth::Ready);
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
