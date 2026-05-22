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
    PROVISIONING_DEVICE_INFO_UUID, PROVISIONING_SERVICE_UUID, PROVISIONING_STATUS_UUID,
    PROVISIONING_WIFI_CMD_UUID,
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
        if self.force_enabled() || !wifi::has_active_connection() {
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
        thread::Builder::new()
            .name("ble-provision".to_string())
            .spawn(move || {
                info!(
                    target: "sys",
                    "Starting BLE provisioning sidecar (reason={})",
                    reason
                );

                let result = run_service(&manager.inner.version, manager.inner.state.clone());
                match result {
                    Ok(creds) => {
                        persist_commissioning_wifi_credentials(&manager.inner.state, &creds);
                        info!(
                            target: "sys",
                            "BLE provisioning completed for SSID '{}'",
                            creds.ssid
                        );
                    }
                    Err(e) => warn!(target: "sys", "BLE provisioning stopped: {:#}", e),
                }

                if let Ok(mut running) = manager.inner.running.lock() {
                    *running = false;
                }
            })
            .context("spawning BLE provisioning thread")?;

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
                        let owner_token = match rhythm_os::auth::issue_owner_token(
                            &state,
                            Some("BLE owner".to_string()),
                        ) {
                            Ok(rhythm_os::auth::IssueOwnerTokenResult::Issued(issued)) => {
                                Some(issued.token)
                            }
                            Ok(rhythm_os::auth::IssueOwnerTokenResult::AlreadyConfigured) => None,
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
        Sender as StdSender, PROVISIONING_DEVICE_INFO_UUID, PROVISIONING_SERVICE_UUID,
        PROVISIONING_STATUS_UUID, PROVISIONING_WIFI_CMD_UUID,
    };
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
        let status_uuid = Uuid::from_u128(PROVISIONING_STATUS_UUID);
        let device_info_uuid = Uuid::from_u128(PROVISIONING_DEVICE_INFO_UUID);

        let status_read_rx = status_tx.subscribe();
        let status_notify_tx = status_tx.clone();
        let device_info_bytes = info.json_bytes()?;
        let write_event_tx = event_tx.clone();

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
                                                    if status_rx.changed().await.is_err() {
                                                        break;
                                                    }
                                                    status_rx.borrow().clone()
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
