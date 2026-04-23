//! Rhythm OS Linux appliance runtime.
//!
//! Reuses the native Linux/macOS server stack, but gives appliance
//! targets like rpiz their own binary crate so board-specific provisioning,
//! networking, and packaging concerns do not accumulate in `rhythm-server`.

mod ble_provision;
mod factory_reset;
mod http_server;
mod wifi;

use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
const RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV: &str =
    "RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION";
const STARTUP_WIFI_RESTORE_TIMEOUT: Duration = Duration::from_secs(30);
const PERIODIC_WIFI_WAIT_POLL_INTERVAL: Duration = Duration::from_secs(5);

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

    set_env_default(RHYTHM_MATTER_BYPASS_DEVICE_ATTESTATION_ENV, "1")
}

fn main() -> Result<()> {
    let args = Args::parse();
    let beta_defaults_applied = apply_beta_build_defaults();
    let platform_type =
        std::env::var("RHYTHM_PLATFORM_TYPE").unwrap_or_else(|_| "appliance".to_string());
    let platform_context =
        std::env::var("RHYTHM_PLATFORM_CONTEXT").unwrap_or_else(|_| "rpiz".to_string());

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
                run_periodic_when_wifi_ready(periodic_state);
            })
            .expect("Failed to spawn periodic thread");
    }

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

fn periodic_startup_action(has_active_connection: bool) -> PeriodicStartupAction {
    if has_active_connection {
        PeriodicStartupAction::StartImmediately
    } else {
        PeriodicStartupAction::WaitForWifi
    }
}

fn run_periodic_when_wifi_ready(state: SharedState) {
    if periodic_startup_action(wifi::has_active_connection()) == PeriodicStartupAction::WaitForWifi
    {
        info!(
            target: "sys",
            "Delaying periodic loop start until appliance has an active Wi-Fi IP"
        );
        loop {
            if wifi::has_active_connection() {
                info!(
                    target: "sys",
                    "Active Wi-Fi IP detected; starting periodic loop"
                );
                break;
            }
            std::thread::sleep(PERIODIC_WIFI_WAIT_POLL_INTERVAL);
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
        Ok(ip) => info!(
            target: "sys",
            "Restored appliance Wi-Fi connectivity from persisted credentials (SSID='{}', ip={})",
            creds.ssid,
            ip
        ),
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
    spawn_boot_success_marker();

    axum::serve(listener, server).await?;

    Ok(())
}

fn spawn_boot_success_marker() {
    const BOOTSTATE_SCRIPT: &str = "/etc/init.d/S41bootstate";
    const GRACE_SECS: u64 = 30;

    if !std::path::Path::new(BOOTSTATE_SCRIPT).exists() {
        return;
    }

    info!(
        target: "sys",
        "Will mark OTA slot as last-good via {} success in {}s",
        BOOTSTATE_SCRIPT,
        GRACE_SECS
    );

    std::thread::Builder::new()
        .name("boot-success-marker".to_string())
        .spawn(|| {
            std::thread::sleep(std::time::Duration::from_secs(GRACE_SECS));
            match std::process::Command::new(BOOTSTATE_SCRIPT)
                .arg("success")
                .status()
            {
                Ok(status) if status.success() => {
                    info!(target: "sys", "OTA bootstate marked success");
                }
                Ok(status) => warn!(
                    target: "sys",
                    "{} success exited with {}",
                    BOOTSTATE_SCRIPT,
                    status
                ),
                Err(e) => warn!(
                    target: "sys",
                    "Failed to exec {} success: {}",
                    BOOTSTATE_SCRIPT,
                    e
                ),
            }
        })
        .expect("Failed to spawn boot-success marker thread");
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
        beta_build_enables_matter_attestation_bypass, extract_serial_suffix,
        periodic_startup_action, startup_wifi_restore_action, PeriodicStartupAction,
        StartupWifiRestoreAction, STARTUP_WIFI_RESTORE_TIMEOUT,
    };

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
            periodic_startup_action(true),
            PeriodicStartupAction::StartImmediately
        );
    }

    #[test]
    fn periodic_startup_waits_for_wifi_when_disconnected() {
        assert_eq!(
            periodic_startup_action(false),
            PeriodicStartupAction::WaitForWifi
        );
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
}
