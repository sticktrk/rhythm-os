//! Linux appliance BLE provisioning sidecar.
//!
//! This keeps the shared provisioning session in `rhythm_os::provisioning`
//! and swaps only the transport/frontend implementation from ESP-IDF to BlueZ.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use log::{info, warn};
use rhythm_os::provisioning::{
    provisioning_device_name, run_provisioning_session, ProvisioningBackend,
    ProvisioningConnectResult, ProvisioningDeviceInfo, ProvisioningEvent, ProvisioningFrontend,
    ProvisioningSessionConfig, ProvisioningStatus, WifiCredentials,
};
#[cfg(target_os = "linux")]
use rhythm_os::provisioning::{
    PROVISIONING_AUTH_CMD_UUID, PROVISIONING_DEVICE_INFO_UUID, PROVISIONING_SERVICE_UUID,
    PROVISIONING_STATUS_UUID, PROVISIONING_WIFI_CMD_UUID,
};
use rhythm_os::state::SharedState;

use crate::time_sync;
use crate::wifi;

const FORCE_ENV: &str = "RHYTHM_BLE_PROVISION_ALWAYS";

#[derive(Clone)]
pub struct ProvisioningManager {
    inner: Arc<ProvisioningManagerInner>,
}

struct ProvisioningManagerInner {
    version: String,
    state: SharedState,
    running: Mutex<bool>,
}

impl ProvisioningManager {
    pub fn new(version: impl Into<String>, state: SharedState) -> Self {
        Self {
            inner: Arc::new(ProvisioningManagerInner {
                version: version.into(),
                state,
                running: Mutex::new(false),
            }),
        }
    }

    pub fn ensure_running_if_needed(&self, reason: &str) -> Result<bool> {
        if self.should_run_for_current_state() {
            return self.ensure_running(reason);
        }
        Ok(false)
    }

    pub fn ensure_running(&self, reason: &str) -> Result<bool> {
        let mut running = self
            .inner
            .running
            .lock()
            .map_err(|_| anyhow!("provisioning manager lock poisoned"))?;
        if *running {
            return Ok(false);
        }

        *running = true;
        let manager = self.clone();
        let reason = reason.to_string();
        let spawned = thread::Builder::new()
            .name("ble-provision".to_string())
            .spawn(move || {
                // BLE provisioning is the recovery path for an appliance with
                // no Wi-Fi; the flag must reset on every exit, including an
                // unwind, or the sidecar can never be restarted this session.
                let _running_guard = RunningFlagGuard {
                    inner: manager.inner.clone(),
                };
                info!(
                    target: "sys",
                    "Starting BLE provisioning sidecar (reason={})",
                    reason
                );

                provision_loop(
                    || run_service(&manager.inner.version, manager.inner.state.clone()),
                    || manager.should_run_for_current_state(),
                    |creds| persist_commissioning_wifi_credentials(&manager.inner.state, creds),
                    PROVISION_RETRY_DELAY,
                );
            });

        if let Err(error) = spawned {
            *running = false;
            return Err(error).context("spawning BLE provisioning thread");
        }

        Ok(true)
    }

    pub fn is_running(&self) -> bool {
        self.inner
            .running
            .lock()
            .map(|running| *running)
            .unwrap_or(false)
    }

    pub fn force_enabled(&self) -> bool {
        match std::env::var(FORCE_ENV) {
            Ok(value) => matches!(
                value.trim().to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            ),
            Err(_) => false,
        }
    }

    fn should_run_for_current_state(&self) -> bool {
        self.force_enabled() || self.api_auth_required() || !wifi::has_active_connection()
    }

    fn api_auth_required(&self) -> bool {
        self.inner
            .state
            .lock()
            .map(|state| state.require_api_auth)
            .unwrap_or(true)
    }
}

const PROVISION_RETRY_DELAY: Duration = Duration::from_secs(2);

/// Resets the manager's `running` flag when the provisioning thread exits —
/// by return or by panic — so `ensure_running` can always start a fresh
/// sidecar.
struct RunningFlagGuard {
    inner: Arc<ProvisioningManagerInner>,
}

impl Drop for RunningFlagGuard {
    fn drop(&mut self) {
        if let Ok(mut running) = self.inner.running.lock() {
            *running = false;
        }
    }
}

/// Run provisioning sessions until provisioning is no longer needed.
///
/// A panic inside a session (BlueZ/D-Bus interop) is contained and treated
/// like a failed session: logged, then retried after `retry_delay` while
/// `should_continue` holds. Injectable closures keep this testable without a
/// Bluetooth stack.
fn provision_loop<S, C, P>(
    mut run_service_once: S,
    mut should_continue: C,
    mut on_credentials: P,
    retry_delay: Duration,
) where
    S: FnMut() -> Result<WifiCredentials>,
    C: FnMut() -> bool,
    P: FnMut(&WifiCredentials),
{
    loop {
        let outcome =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(&mut run_service_once));
        match outcome {
            Ok(Ok(creds)) => {
                on_credentials(&creds);
                info!(
                    target: "sys",
                    "BLE provisioning completed for SSID '{}'",
                    creds.ssid
                );
            }
            Ok(Err(e)) => warn!(target: "sys", "BLE provisioning stopped: {:#}", e),
            Err(panic) => warn!(
                target: "sys",
                "BLE provisioning session panicked: {}",
                panic_message(panic.as_ref())
            ),
        }

        if !should_continue() {
            break;
        }

        thread::sleep(retry_delay);
        info!(
            target: "sys",
            "Restarting BLE provisioning sidecar because local provisioning remains available"
        );
    }
}

fn panic_message(panic: &(dyn std::any::Any + Send)) -> &str {
    panic
        .downcast_ref::<&'static str>()
        .copied()
        .or_else(|| panic.downcast_ref::<String>().map(String::as_str))
        .unwrap_or("non-string panic payload")
}

fn persist_commissioning_wifi_credentials(state: &SharedState, creds: &WifiCredentials) {
    let result = state
        .lock()
        .map_err(|_| anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    if let Err(e) = result {
        warn!(
            target: "sys",
            "Failed to persist commissioning Wi-Fi credentials after provisioning: {:#}",
            e
        );
    }
}

fn run_service(version: &str, state: SharedState) -> Result<WifiCredentials> {
    let identity = build_identity(version);
    let mut frontend = BluezFrontend::new()?;
    let mut backend = LinuxWifiBackend::new(Duration::from_secs(30), state);
    let config = ProvisioningSessionConfig::default();

    run_provisioning_session(&mut frontend, &mut backend, &identity, &config)
}

fn issue_ble_owner_token(state: &SharedState, label: Option<String>) -> Result<String> {
    let issued = rhythm_os::auth::issue_local_owner_token(
        state,
        label.or_else(|| Some("BLE local".to_string())),
    )?;
    Ok(issued.token)
}

fn build_identity(version: &str) -> ProvisioningDeviceInfo {
    let serial = std::fs::read("/proc/device-tree/serial-number")
        .ok()
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .map(|serial| serial.trim_end_matches('\0').trim().to_string())
        .filter(|serial| serial.len() >= 4);

    let suffix = serial
        .as_deref()
        .map(|serial| serial[serial.len() - 4..].to_ascii_uppercase())
        .unwrap_or_else(|| "RPIZ".to_string());

    ProvisioningDeviceInfo {
        name: provisioning_device_name("rpiz", &suffix),
        version: version.to_string(),
        mac: None,
    }
}

struct LinuxWifiBackend {
    state: SharedState,
    result_tx: Sender<(u64, ProvisioningConnectResult)>,
    result_rx: Receiver<(u64, ProvisioningConnectResult)>,
    connect_timeout: Duration,
    next_attempt: u64,
    active_attempt: Option<u64>,
}

impl LinuxWifiBackend {
    fn new(connect_timeout: Duration, state: SharedState) -> Self {
        let (result_tx, result_rx) = mpsc::channel();
        Self {
            state,
            result_tx,
            result_rx,
            connect_timeout,
            next_attempt: 1,
            active_attempt: None,
        }
    }
}

impl ProvisioningBackend for LinuxWifiBackend {
    fn begin_connect(&mut self, creds: WifiCredentials) -> Result<()> {
        let attempt = self.next_attempt;
        self.next_attempt += 1;
        self.active_attempt = Some(attempt);

        let result_tx = self.result_tx.clone();
        let timeout = self.connect_timeout;
        let state = self.state.clone();
        thread::Builder::new()
            .name(format!("wifi-prov-{}", attempt))
            .spawn(move || {
                let result = match wifi::connect_with_credentials(&creds, timeout) {
                    Ok(ip) => {
                        let owner_token =
                            match issue_ble_owner_token(&state, Some("BLE Wi-Fi".to_string())) {
                                Ok(token) => Some(token),
                                Err(e) => {
                                    let _ = result_tx.send((
                                        attempt,
                                        ProvisioningConnectResult::Failed {
                                            error: format!("failed to issue owner token: {e}"),
                                        },
                                    ));
                                    return;
                                }
                            };
                        let _ = result_tx.send((
                            attempt,
                            ProvisioningConnectResult::Connected { ip, owner_token },
                        ));
                        sync_clock_after_wifi_connect();
                        return;
                    }
                    Err(e) => ProvisioningConnectResult::Failed {
                        error: e.to_string(),
                    },
                };
                let _ = result_tx.send((attempt, result));
            })
            .context("spawning Wi-Fi provisioning worker")?;

        Ok(())
    }

    fn poll_result(&mut self, timeout: Duration) -> Result<Option<ProvisioningConnectResult>> {
        let Some(active_attempt) = self.active_attempt else {
            return Ok(None);
        };

        if timeout.is_zero() {
            loop {
                match self.result_rx.try_recv() {
                    Ok((attempt, result)) if attempt == active_attempt => {
                        self.active_attempt = None;
                        return Ok(Some(result));
                    }
                    Ok(_) => continue,
                    Err(mpsc::TryRecvError::Empty) => return Ok(None),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return Err(anyhow!("Wi-Fi result channel disconnected"));
                    }
                }
            }
        }

        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(None);
            }

            match self.result_rx.recv_timeout(remaining) {
                Ok((attempt, result)) if attempt == active_attempt => {
                    self.active_attempt = None;
                    return Ok(Some(result));
                }
                Ok(_) => continue,
                Err(mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("Wi-Fi result channel disconnected"));
                }
            }
        }
    }

    fn issue_local_owner_token(&mut self, label: Option<String>) -> Result<Option<String>> {
        issue_ble_owner_token(&self.state, label).map(Some)
    }
}

fn sync_clock_after_wifi_connect() {
    match std::panic::catch_unwind(|| {
        time_sync::sync_system_clock("BLE provisioning Wi-Fi connection")
    }) {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => warn!(
            target: "sys",
            "Failed to sync wall clock after BLE provisioning connected Wi-Fi: {:#}",
            error
        ),
        Err(_) => warn!(
            target: "sys",
            "Wall-clock sync panicked after BLE provisioning connected Wi-Fi; continuing because Wi-Fi credentials were already accepted"
        ),
    }
}

#[cfg(target_os = "linux")]
mod bluez {
    use std::sync::mpsc::{Receiver, RecvTimeoutError};
    use std::time::Duration;

    use anyhow::{Context, Result};
    use bluer::adv::Advertisement;
    use bluer::gatt::local::{
        Application, ApplicationHandle, Characteristic, CharacteristicNotify,
        CharacteristicNotifyMethod, CharacteristicRead, CharacteristicWrite,
        CharacteristicWriteMethod, ReqError, Service,
    };
    use bluer::{adv::AdvertisementHandle, Adapter, Session};
    use futures::FutureExt;
    use log::{info, warn};
    use tokio::runtime::Runtime;
    use tokio::sync::watch;
    use uuid::Uuid;

    use super::{
        ProvisioningDeviceInfo, ProvisioningEvent, ProvisioningFrontend, ProvisioningStatus,
        Sender as StdSender, PROVISIONING_AUTH_CMD_UUID, PROVISIONING_DEVICE_INFO_UUID,
        PROVISIONING_SERVICE_UUID, PROVISIONING_STATUS_UUID, PROVISIONING_WIFI_CMD_UUID,
    };
    /// How often an idle status-notify task probes its subscriber. Bounds the
    /// lifetime of tasks whose BLE client disconnected between status changes.
    const NOTIFY_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(60);

    pub(super) struct BluezFrontend {
        runtime: Runtime,
        event_tx: StdSender<ProvisioningEvent>,
        event_rx: Receiver<ProvisioningEvent>,
        status_tx: watch::Sender<Vec<u8>>,
        handles: Option<BluezHandles>,
    }

    struct BluezHandles {
        session: Session,
        adapter: Adapter,
        previous_alias: String,
        app: ApplicationHandle,
        adv: AdvertisementHandle,
    }

    impl BluezFrontend {
        pub(super) fn new() -> Result<Self> {
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(2)
                .thread_name("ble-provision-rt")
                .build()
                .context("building BlueZ runtime")?;
            let (event_tx, event_rx) = std::sync::mpsc::channel();
            let initial = ProvisioningStatus::Waiting.json_bytes()?;
            let (status_tx, _status_rx) = watch::channel(initial);

            Ok(Self {
                runtime,
                event_tx,
                event_rx,
                status_tx,
                handles: None,
            })
        }
    }

    impl ProvisioningFrontend for BluezFrontend {
        fn start(&mut self, info: &ProvisioningDeviceInfo) -> Result<()> {
            let handles = self.runtime.block_on(start_bluez(
                info.clone(),
                self.event_tx.clone(),
                self.status_tx.clone(),
            ))?;
            self.handles = Some(handles);
            info!("BLE provisioning active — waiting for Wi-Fi credentials...");
            Ok(())
        }

        fn poll_event(&mut self, timeout: Duration) -> Result<Option<ProvisioningEvent>> {
            match self.event_rx.recv_timeout(timeout) {
                Ok(event) => Ok(Some(event)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => {
                    Err(anyhow::anyhow!("BLE event channel disconnected"))
                }
            }
        }

        fn publish_status(&mut self, status: &ProvisioningStatus) -> Result<()> {
            let bytes = status.json_bytes()?;
            self.status_tx.send_replace(bytes);
            info!(target: "sys", "BLE provisioning status: {}", status.code());
            Ok(())
        }

        fn stop(&mut self) -> Result<()> {
            info!("Stopping BLE provisioning frontend...");
            if let Some(handles) = self.handles.take() {
                self.runtime.block_on(stop_bluez(handles))?;
            }
            Ok(())
        }
    }

    async fn start_bluez(
        mut info: ProvisioningDeviceInfo,
        event_tx: StdSender<ProvisioningEvent>,
        status_tx: watch::Sender<Vec<u8>>,
    ) -> Result<BluezHandles> {
        let session = Session::new().await.context("opening BlueZ session")?;
        let adapter = session
            .default_adapter()
            .await
            .context("finding default Bluetooth adapter")?;

        adapter
            .set_powered(true)
            .await
            .context("powering Bluetooth adapter")?;
        adapter
            .set_pairable(false)
            .await
            .context("disabling pairable mode")?;
        let previous_alias = adapter
            .alias()
            .await
            .context("reading Bluetooth adapter alias")?;
        adapter
            .set_alias(info.name.clone())
            .await
            .context("setting Bluetooth adapter alias")?;

        if info.mac.is_none() {
            info.mac = adapter.address().await.ok().map(|addr| addr.to_string());
        }

        let service_uuid = Uuid::from_u128(PROVISIONING_SERVICE_UUID);
        let wifi_cmd_uuid = Uuid::from_u128(PROVISIONING_WIFI_CMD_UUID);
        let auth_cmd_uuid = Uuid::from_u128(PROVISIONING_AUTH_CMD_UUID);
        let status_uuid = Uuid::from_u128(PROVISIONING_STATUS_UUID);
        let device_info_uuid = Uuid::from_u128(PROVISIONING_DEVICE_INFO_UUID);

        let status_read_rx = status_tx.subscribe();
        let status_notify_tx = status_tx.clone();
        let device_info_bytes = info.json_bytes()?;
        let write_event_tx = event_tx.clone();
        let auth_event_tx = event_tx.clone();

        let app = Application {
            services: vec![Service {
                uuid: service_uuid,
                primary: true,
                characteristics: vec![
                    Characteristic {
                        uuid: wifi_cmd_uuid,
                        write: Some(CharacteristicWrite {
                            write: true,
                            write_without_response: true,
                            method: CharacteristicWriteMethod::Fun(Box::new(move |value, req| {
                                let event_tx = write_event_tx.clone();
                                async move {
                                    info!(
                                        "BLE credential write from {} (mtu={}, len={})",
                                        req.device_address,
                                        req.mtu,
                                        value.len()
                                    );
                                    match parse_credentials(&value) {
                                        Ok(creds) => {
                                            event_tx
                                                .send(ProvisioningEvent::Credentials(creds))
                                                .map_err(|_| ReqError::Failed)?;
                                            Ok(())
                                        }
                                        Err(e) => {
                                            let _ = event_tx
                                                .send(ProvisioningEvent::Error(e.to_string()));
                                            Err(ReqError::Failed)
                                        }
                                    }
                                }
                                .boxed()
                            })),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: auth_cmd_uuid,
                        write: Some(CharacteristicWrite {
                            write: true,
                            write_without_response: true,
                            method: CharacteristicWriteMethod::Fun(Box::new(move |value, req| {
                                let event_tx = auth_event_tx.clone();
                                async move {
                                    info!(
                                        "BLE local auth request from {} (mtu={}, len={})",
                                        req.device_address,
                                        req.mtu,
                                        value.len()
                                    );
                                    match parse_auth_token_request(&value) {
                                        Ok(label) => {
                                            event_tx
                                                .send(ProvisioningEvent::AuthTokenRequest { label })
                                                .map_err(|_| ReqError::Failed)?;
                                            Ok(())
                                        }
                                        Err(e) => {
                                            let _ = event_tx
                                                .send(ProvisioningEvent::Error(e.to_string()));
                                            Err(ReqError::Failed)
                                        }
                                    }
                                }
                                .boxed()
                            })),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: status_uuid,
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(move |_req| {
                                let bytes = status_read_rx.borrow().clone();
                                async move { Ok(bytes) }.boxed()
                            }),
                            ..Default::default()
                        }),
                        notify: Some(CharacteristicNotify {
                            notify: true,
                            method: CharacteristicNotifyMethod::Fun(Box::new(
                                move |mut notifier| {
                                    let mut status_rx = status_notify_tx.subscribe();
                                    async move {
                                        tokio::spawn(async move {
                                            let mut first = true;
                                            loop {
                                                let payload = if first {
                                                    first = false;
                                                    status_rx.borrow().clone()
                                                } else {
                                                    // Status changes can be far apart; without a
                                                    // periodic probe, tasks for disconnected
                                                    // subscribers linger until the next change.
                                                    // The keepalive notify fails fast for dead
                                                    // sessions and ends the task.
                                                    match tokio::time::timeout(
                                                        NOTIFY_KEEPALIVE_INTERVAL,
                                                        status_rx.changed(),
                                                    )
                                                    .await
                                                    {
                                                        Ok(Ok(())) => status_rx.borrow().clone(),
                                                        Ok(Err(_)) => break,
                                                        Err(_elapsed) => status_rx.borrow().clone(),
                                                    }
                                                };

                                                if let Err(e) = notifier.notify(payload).await {
                                                    warn!("BLE status notify session ended: {}", e);
                                                    break;
                                                }
                                            }
                                        });
                                    }
                                    .boxed()
                                },
                            )),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                    Characteristic {
                        uuid: device_info_uuid,
                        read: Some(CharacteristicRead {
                            read: true,
                            fun: Box::new(move |_req| {
                                let bytes = device_info_bytes.clone();
                                async move { Ok(bytes) }.boxed()
                            }),
                            ..Default::default()
                        }),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            }],
            ..Default::default()
        };

        let app_handle = adapter
            .serve_gatt_application(app)
            .await
            .context("registering GATT application")?;

        let advertisement = Advertisement {
            advertisement_type: bluer::adv::Type::Peripheral,
            service_uuids: vec![service_uuid].into_iter().collect(),
            discoverable: Some(true),
            local_name: Some(info.name.clone()),
            ..Default::default()
        };
        let adv_handle = adapter
            .advertise(advertisement)
            .await
            .context("starting BLE advertisement")?;

        let adapter_addr = adapter.address().await.ok();
        info!(
            target: "sys",
            "BLE provisioning advertising on {}{}",
            adapter.name(),
            adapter_addr
                .map(|addr| format!(" ({})", addr))
                .unwrap_or_default()
        );

        Ok(BluezHandles {
            session,
            adapter,
            previous_alias,
            app: app_handle,
            adv: adv_handle,
        })
    }

    async fn stop_bluez(handles: BluezHandles) -> Result<()> {
        let BluezHandles {
            session,
            adapter,
            previous_alias,
            app,
            adv,
        } = handles;

        drop(adv);
        drop(app);

        adapter
            .set_alias(previous_alias)
            .await
            .context("restoring Bluetooth adapter alias")?;

        drop(adapter);
        drop(session);
        Ok(())
    }

    fn parse_credentials(bytes: &[u8]) -> Result<super::WifiCredentials> {
        let creds: super::WifiCredentials =
            serde_json::from_slice(bytes).context("parsing BLE Wi-Fi credentials JSON")?;
        if creds.ssid.trim().is_empty() {
            anyhow::bail!("Missing ssid");
        }
        Ok(creds)
    }

    fn parse_auth_token_request(bytes: &[u8]) -> Result<Option<String>> {
        if bytes.iter().all(|byte| byte.is_ascii_whitespace()) {
            return Ok(None);
        }

        let body: serde_json::Value =
            serde_json::from_slice(bytes).context("parsing BLE auth request JSON")?;
        let label = body
            .get("label")
            .or_else(|| body.get("owner_label"))
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|label| !label.is_empty())
            .map(str::to_string);
        Ok(label)
    }
}

#[cfg(target_os = "linux")]
use bluez::BluezFrontend;

#[cfg(not(target_os = "linux"))]
struct BluezFrontend;

#[cfg(not(target_os = "linux"))]
impl BluezFrontend {
    fn new() -> Result<Self> {
        Err(anyhow!("BlueZ provisioning is only supported on Linux"))
    }
}

#[cfg(not(target_os = "linux"))]
impl ProvisioningFrontend for BluezFrontend {
    fn start(&mut self, _info: &ProvisioningDeviceInfo) -> Result<()> {
        Err(anyhow!("BlueZ provisioning is only supported on Linux"))
    }

    fn poll_event(&mut self, _timeout: Duration) -> Result<Option<ProvisioningEvent>> {
        Err(anyhow!("BlueZ provisioning is only supported on Linux"))
    }

    fn publish_status(&mut self, _status: &ProvisioningStatus) -> Result<()> {
        Err(anyhow!("BlueZ provisioning is only supported on Linux"))
    }

    fn stop(&mut self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex as StdMutex;

    use rhythm_os::storage::{FileStorage, Storage};

    static ENV_LOCK: StdMutex<()> = StdMutex::new(());

    fn state() -> SharedState {
        Arc::new(Mutex::new(rhythm_os::state::AppState::default()))
    }

    fn unique_test_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-ble-provision-{name}-{nanos}"));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn with_force_env<T>(value: Option<&str>, f: impl FnOnce() -> T) -> T {
        let _guard = ENV_LOCK.lock().unwrap();
        let previous = std::env::var(FORCE_ENV).ok();
        match value {
            Some(value) => std::env::set_var(FORCE_ENV, value),
            None => std::env::remove_var(FORCE_ENV),
        }
        let result = f();
        if let Some(previous) = previous {
            std::env::set_var(FORCE_ENV, previous);
        } else {
            std::env::remove_var(FORCE_ENV);
        }
        result
    }

    fn wifi_credentials() -> WifiCredentials {
        WifiCredentials {
            ssid: "RhythmNet".to_string(),
            password: "secret".to_string(),
        }
    }

    #[test]
    fn provisioning_manager_force_flag_accepts_common_truthy_values() {
        let manager = ProvisioningManager::new("1.2.3", state());

        for value in ["1", "true", "TRUE", "yes", "on"] {
            with_force_env(Some(value), || assert!(manager.force_enabled()));
        }
        for value in ["0", "false", "no", "off", "", "maybe"] {
            with_force_env(Some(value), || assert!(!manager.force_enabled()));
        }
        with_force_env(None, || assert!(!manager.force_enabled()));
    }

    #[test]
    fn provisioning_manager_checks_auth_requirement_and_force_override() {
        let state = state();
        let manager = ProvisioningManager::new("1.2.3", state.clone());

        state.lock().unwrap().require_api_auth = true;
        assert!(manager.api_auth_required());
        with_force_env(
            Some("1"),
            || assert!(manager.should_run_for_current_state()),
        );

        state.lock().unwrap().require_api_auth = false;
        assert!(!manager.api_auth_required());
        with_force_env(
            Some("1"),
            || assert!(manager.should_run_for_current_state()),
        );
    }

    #[test]
    fn persist_commissioning_wifi_credentials_writes_storage_when_available() {
        let root = unique_test_dir("persist");
        let storage = FileStorage::new(root.to_str().unwrap()).unwrap();
        let state = state();
        state.lock().unwrap().storage = Some(Box::new(storage));

        let creds = wifi_credentials();
        persist_commissioning_wifi_credentials(&state, &creds);

        let storage = FileStorage::new(root.to_str().unwrap()).unwrap();
        assert_eq!(
            storage.load_commissioning_wifi_credentials().unwrap(),
            Some(creds)
        );
        std::fs::remove_dir_all(root).ok();
    }

    #[test]
    fn persist_commissioning_wifi_credentials_noops_without_storage() {
        let state = state();
        persist_commissioning_wifi_credentials(&state, &wifi_credentials());
    }

    #[test]
    fn build_identity_uses_rpiz_name_and_supplied_version() {
        let identity = build_identity("9.8.7-test");

        assert!(identity.name.starts_with("rhythm-rpiz-"));
        assert_eq!(identity.version, "9.8.7-test");
        assert!(identity.mac.is_none());
    }

    #[test]
    fn provision_loop_survives_a_panicking_session_and_retries() {
        let calls = std::cell::Cell::new(0u32);
        let continues = std::cell::Cell::new(0u32);

        provision_loop(
            || {
                calls.set(calls.get() + 1);
                if calls.get() == 1 {
                    panic!("simulated BlueZ panic");
                }
                Err(anyhow!("simulated session error"))
            },
            || {
                continues.set(continues.get() + 1);
                // Allow exactly one retry after the panic, then stop.
                continues.get() < 2
            },
            |_| {},
            Duration::ZERO,
        );

        assert_eq!(
            calls.get(),
            2,
            "the loop must retry after a panicking session"
        );
    }

    #[test]
    fn provision_loop_reports_credentials_on_success() {
        let received = std::cell::RefCell::new(Vec::new());

        provision_loop(
            || Ok(wifi_credentials()),
            || false,
            |creds| received.borrow_mut().push(creds.clone()),
            Duration::ZERO,
        );

        assert_eq!(received.borrow().as_slice(), &[wifi_credentials()]);
    }

    #[test]
    fn running_flag_resets_when_provisioning_thread_panics() {
        let manager = ProvisioningManager::new("1.2.3", state());
        *manager.inner.running.lock().unwrap() = true;
        assert!(manager.is_running());

        let inner = manager.inner.clone();
        let handle = thread::spawn(move || {
            let _guard = RunningFlagGuard { inner };
            panic!("simulated provisioning thread panic");
        });
        assert!(handle.join().is_err());

        assert!(
            !manager.is_running(),
            "a panicking sidecar must not leave the manager wedged as running"
        );
    }

    #[test]
    fn panic_message_extracts_str_and_string_payloads() {
        let static_payload =
            std::panic::catch_unwind(|| panic!("static payload")).unwrap_err();
        assert_eq!(panic_message(static_payload.as_ref()), "static payload");

        let string_payload = std::panic::catch_unwind(|| {
            std::panic::panic_any(format!("formatted {}", 42))
        })
        .unwrap_err();
        assert_eq!(panic_message(string_payload.as_ref()), "formatted 42");

        let opaque_payload =
            std::panic::catch_unwind(|| std::panic::panic_any(7_u64)).unwrap_err();
        assert_eq!(
            panic_message(opaque_payload.as_ref()),
            "non-string panic payload"
        );
    }

    #[test]
    fn linux_wifi_backend_poll_result_filters_stale_attempts_and_clears_active_attempt() {
        let state = state();
        let mut backend = LinuxWifiBackend::new(Duration::from_millis(1), state);

        assert!(backend.poll_result(Duration::ZERO).unwrap().is_none());

        backend.active_attempt = Some(2);
        backend
            .result_tx
            .send((
                1,
                ProvisioningConnectResult::Failed {
                    error: "stale".to_string(),
                },
            ))
            .unwrap();
        backend
            .result_tx
            .send((
                2,
                ProvisioningConnectResult::Connected {
                    ip: "192.168.1.10".to_string(),
                    owner_token: Some("owner".to_string()),
                },
            ))
            .unwrap();

        let result = backend.poll_result(Duration::ZERO).unwrap().unwrap();
        match result {
            ProvisioningConnectResult::Connected { ip, owner_token } => {
                assert_eq!(ip, "192.168.1.10");
                assert_eq!(owner_token.as_deref(), Some("owner"));
            }
            other => panic!("expected connected result, got {other:?}"),
        }
        assert_eq!(backend.active_attempt, None);
    }

    #[test]
    fn linux_wifi_backend_poll_result_times_out_when_active_attempt_has_no_result() {
        let mut backend = LinuxWifiBackend::new(Duration::from_millis(1), state());
        backend.active_attempt = Some(1);

        assert!(backend
            .poll_result(Duration::from_millis(1))
            .unwrap()
            .is_none());
        assert_eq!(backend.active_attempt, Some(1));
    }

    #[test]
    fn linux_wifi_backend_can_issue_local_owner_token() {
        let state = state();
        let mut backend = LinuxWifiBackend::new(Duration::from_millis(1), state);

        let token = backend
            .issue_local_owner_token(Some("BLE test".to_string()))
            .unwrap()
            .expect("owner token should be returned");

        assert!(!token.is_empty());
    }

    #[cfg(not(target_os = "linux"))]
    #[test]
    fn non_linux_bluez_frontend_reports_unsupported_operations() {
        let new_error = match BluezFrontend::new() {
            Ok(_) => panic!("expected BlueZ frontend creation to fail off Linux"),
            Err(error) => error,
        };
        assert!(new_error.to_string().contains("only supported on Linux"));

        let mut frontend = BluezFrontend;
        let info = build_identity("1.0.0");
        assert!(frontend
            .start(&info)
            .unwrap_err()
            .to_string()
            .contains("Linux"));
        assert!(frontend
            .poll_event(Duration::ZERO)
            .unwrap_err()
            .to_string()
            .contains("Linux"));
        assert!(frontend
            .publish_status(&ProvisioningStatus::Waiting)
            .unwrap_err()
            .to_string()
            .contains("Linux"));
        frontend.stop().unwrap();
    }
}
