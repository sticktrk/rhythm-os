//! Desktop Matter transport backed by a local native CHIP controller daemon.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::chip_rpc::{
    ChipInitControllerRequest, ChipInitControllerResponse, ChipRpcAttributeReportsResponse,
    ChipRpcCommissionLightResponse, ChipRpcError, ChipRpcJsonValueResponse,
    ChipRpcListDevicesResponse, ChipRpcOperationalDiscoveryResponse, ChipRpcProbeLightResponse,
    ChipRpcReadOnOffResponse, ChipRpcRequest, ChipRpcRequestEnvelope, ChipRpcResponseEnvelope,
};
use crate::fabric::MatterFabricIdentity;
use crate::transport::{
    CommissionedDevice, MatterAttributeReport, MatterCommissionRequest, MatterDeviceInfo,
    MatterGroup, MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode,
    MatterSubscriptionTarget, MatterTransport,
};

const SOCKET_NAME: &str = "chip-controller.sock";
const STORAGE_NAME: &str = "controller-storage.json";
const CHIP_EXAMPLE_STORAGE_PREFIX: &str = "chip_tool_config";
const CHIP_EXAMPLE_STORAGE_EXT: &str = "ini";
const CHIPD_LOGFILE_ENV: &str = "RHYTHM_MATTER_LOGFILE";
const SIDECAR_START_TIMEOUT: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(120);
/// Commissioning gets its own, longer deadline: chipd's internal
/// kCommissioningTimeout is 180s, and the client socket timeout must be
/// ordered ABOVE it — otherwise a slow (BLE) commissioning that would have
/// succeeded gets its sidecar killed mid-PASE at 120s and the whole
/// CommissionLight is re-sent against a half-commissioned bulb.
const RPC_COMMISSION_TIMEOUT: Duration = Duration::from_secs(200);
#[cfg(not(test))]
const RPC_CONTROL_TIMEOUT: Duration = Duration::from_secs(8);
#[cfg(test)]
const RPC_CONTROL_TIMEOUT: Duration = Duration::from_millis(100);
const BLE_RECOVERY_COOLDOWN: Duration = Duration::from_secs(3);
const OPERATIONAL_RECOVERY_MDNS_SCAN_TIMEOUT: Duration = Duration::from_secs(15);
/// A recoverable BLE commissioning failure is retried once automatically after
/// the sidecar recovery, but only when the failed attempt itself was quick.
/// The app waits 4 minutes per pairing request and a retry can spend up to
/// chipd's 180s kCommissioningTimeout, so the first attempt must have failed
/// inside this budget for the retry to still fit the request window.
const BLE_AUTO_RETRY_FIRST_ATTEMPT_BUDGET: Duration = Duration::from_secs(45);

fn rpc_timeout_for_request(request: &ChipRpcRequest) -> Duration {
    match request {
        ChipRpcRequest::SetOnOff { .. }
        | ChipRpcRequest::SetGroupOnOff { .. }
        | ChipRpcRequest::IdentifyGroup { .. }
        | ChipRpcRequest::SetGroupBrightness { .. }
        | ChipRpcRequest::SetGroupColorTemperature { .. }
        | ChipRpcRequest::SetGroupXy { .. }
        | ChipRpcRequest::SetGroupHueSaturation { .. }
        | ChipRpcRequest::IdentifyLight { .. }
        | ChipRpcRequest::SetBrightness { .. }
        | ChipRpcRequest::RunLevelCommand { .. }
        | ChipRpcRequest::SetColorTemperature { .. }
        | ChipRpcRequest::SetXy { .. }
        | ChipRpcRequest::SetHueSaturation { .. }
        | ChipRpcRequest::ReadOnOff { .. }
        | ChipRpcRequest::ReadLightState { .. } => RPC_CONTROL_TIMEOUT,
        ChipRpcRequest::CommissionLight(_) => RPC_COMMISSION_TIMEOUT,
        _ => RPC_TIMEOUT,
    }
}

struct SidecarConfig {
    command: PathBuf,
    working_dir: PathBuf,
    log_path: Option<PathBuf>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SidecarHealth {
    Uninitialized,
    Ready,
    Recovering,
    BleCooldown,
    Unavailable,
}

/// Desktop Matter transport backed by a native CHIP daemon over a Unix socket.
pub struct ChipTransport {
    socket_path: PathBuf,
    init_request: ChipInitControllerRequest,
    sidecar: Mutex<Option<Child>>,
    sidecar_config: Option<SidecarConfig>,
    sidecar_health: Mutex<SidecarHealth>,
    initialized: AtomicBool,
    next_request_id: AtomicU64,
    commissioning_lock: Mutex<()>,
    ble_commissioning_ready_after: Mutex<Option<Instant>>,
    ble_auto_retry_first_attempt_budget: Duration,
}

impl ChipTransport {
    /// Load or create the local CHIP controller sidecar state.
    pub fn load_or_create(data_path: &str, fabric_id: &str) -> Result<Self> {
        let chip_dir = Path::new(data_path).join("chip");
        let storage_path = chip_dir.join(STORAGE_NAME);
        let storage_artifact_paths = chip_storage_artifact_paths(&chip_dir, &storage_path);
        let fabric_identity = MatterFabricIdentity::load_or_create_with_controller_storage_paths(
            data_path,
            fabric_id,
            &storage_artifact_paths,
        )?;
        fs::create_dir_all(&chip_dir)
            .with_context(|| format!("creating CHIP data dir {}", chip_dir.display()))?;

        let command = resolve_chipd_command().context("resolving rhythm-chipd helper")?;
        let ble_controller = std::env::var("RHYTHM_MATTER_BLE_CONTROLLER")
            .ok()
            .and_then(|value| value.parse::<u16>().ok());

        let transport = Self {
            socket_path: chip_dir.join(SOCKET_NAME),
            init_request: ChipInitControllerRequest {
                fabric_id: fabric_id.to_string(),
                operational_fabric_id: fabric_identity.operational_fabric_id,
                ipk_hex: fabric_identity.ipk_hex.clone(),
                storage_path: storage_path.display().to_string(),
                ble_controller,
            },
            sidecar: Mutex::new(None),
            sidecar_config: Some(SidecarConfig {
                command,
                working_dir: chip_dir.clone(),
                log_path: sidecar_log_path_from_env(std::env::var_os(CHIPD_LOGFILE_ENV)),
            }),
            sidecar_health: Mutex::new(SidecarHealth::Uninitialized),
            initialized: AtomicBool::new(false),
            next_request_id: AtomicU64::new(1),
            commissioning_lock: Mutex::new(()),
            ble_commissioning_ready_after: Mutex::new(None),
            ble_auto_retry_first_attempt_budget: BLE_AUTO_RETRY_FIRST_ATTEMPT_BUDGET,
        };

        transport.ensure_sidecar()?;
        Ok(transport)
    }

    #[cfg(test)]
    fn for_test(socket_path: PathBuf) -> Self {
        Self {
            socket_path,
            init_request: ChipInitControllerRequest {
                fabric_id: "test".to_string(),
                operational_fabric_id: 1,
                ipk_hex: "00112233445566778899aabbccddeeff".to_string(),
                storage_path: "/tmp/test-controller-storage.json".to_string(),
                ble_controller: None,
            },
            sidecar: Mutex::new(None),
            sidecar_config: None,
            sidecar_health: Mutex::new(SidecarHealth::Ready),
            initialized: AtomicBool::new(true),
            next_request_id: AtomicU64::new(1),
            commissioning_lock: Mutex::new(()),
            ble_commissioning_ready_after: Mutex::new(None),
            ble_auto_retry_first_attempt_budget: BLE_AUTO_RETRY_FIRST_ATTEMPT_BUDGET,
        }
    }

    fn sidecar_health(&self) -> SidecarHealth {
        self.sidecar_health
            .lock()
            .map(|health| *health)
            .unwrap_or(SidecarHealth::Unavailable)
    }

    fn set_sidecar_health(&self, health: SidecarHealth) {
        if let Ok(mut current) = self.sidecar_health.lock() {
            *current = health;
        }
    }

    #[cfg(test)]
    fn sidecar_health_for_test(&self) -> SidecarHealth {
        self.sidecar_health()
    }

    fn ensure_sidecar(&self) -> Result<()> {
        if self.initialized.load(Ordering::SeqCst)
            && self.socket_path.exists()
            && matches!(
                self.sidecar_health(),
                SidecarHealth::Ready | SidecarHealth::BleCooldown
            )
        {
            return Ok(());
        }

        if self.can_connect().is_err() {
            self.start_sidecar()?;
        }

        match self.initialize_controller() {
            Ok(()) => Ok(()),
            Err(first_error)
                if self.sidecar_config.is_some()
                    && is_uninitialized_controller_error(&first_error) =>
            {
                self.restart_sidecar().with_context(|| {
                    format!(
                        "restarting CHIP sidecar after controller init error: {:#}",
                        first_error
                    )
                })?;
                self.initialize_controller().with_context(|| {
                    format!(
                        "CHIP controller init retry failed after restarting sidecar: {:#}",
                        first_error
                    )
                })
            }
            Err(first_error) => {
                self.set_sidecar_health(SidecarHealth::Unavailable);
                Err(first_error)
            }
        }
    }

    fn initialize_controller(&self) -> Result<()> {
        let response: ChipInitControllerResponse = self.decode_rpc_response(
            self.send_rpc_envelope(ChipRpcRequest::InitController(self.init_request.clone()))?,
        )?;
        if response.fabric_id != self.init_request.fabric_id {
            anyhow::bail!(
                "CHIP sidecar initialized unexpected fabric '{}'",
                response.fabric_id
            );
        }
        if response.operational_fabric_id != self.init_request.operational_fabric_id {
            anyhow::bail!(
                "CHIP sidecar initialized unexpected operational fabric id {}",
                response.operational_fabric_id
            );
        }

        self.initialized.store(true, Ordering::SeqCst);
        self.set_sidecar_health(SidecarHealth::Ready);
        Ok(())
    }

    fn call<T: serde::de::DeserializeOwned>(&self, request: ChipRpcRequest) -> Result<T> {
        self.ensure_sidecar()?;
        match self.send_rpc_envelope(request.clone()) {
            Ok(response) => match self.decode_rpc_response::<T>(response) {
                Ok(value) => Ok(value),
                Err(rpc_error) if is_uninitialized_controller_error(&rpc_error) => {
                    self.recover_uninitialized_controller(request, rpc_error)
                }
                Err(rpc_error) => Err(rpc_error),
            },
            Err(first_error) if is_control_rpc_response_timeout(&request, &first_error) => {
                Err(first_error)
            }
            Err(first_error) => {
                self.initialized.store(false, Ordering::SeqCst);
                self.set_sidecar_health(SidecarHealth::Unavailable);
                if self.sidecar_config.is_some() {
                    self.restart_sidecar()?;
                    self.ensure_sidecar()?;
                    self.decode_rpc_response(self.send_rpc_envelope(request)?)
                        .with_context(|| {
                            format!(
                                "CHIP RPC retry failed after restarting sidecar: {:#}",
                                first_error
                            )
                        })
                } else {
                    Err(first_error)
                }
            }
        }
    }

    /// Recover from a chipd that responded with `Incorrect state` /
    /// `Controller not initialized`: drop the cached `initialized` flag,
    /// restart the managed sidecar when available, send `InitController`, and
    /// re-issue the original request once. Without this, a chipd that loses
    /// `mCommissioner` (e.g. after an OTA reboot) wedges every subsequent
    /// Matter command.
    fn recover_uninitialized_controller<T: serde::de::DeserializeOwned>(
        &self,
        request: ChipRpcRequest,
        first_error: anyhow::Error,
    ) -> Result<T> {
        self.initialized.store(false, Ordering::SeqCst);
        self.set_sidecar_health(SidecarHealth::Recovering);
        if self.sidecar_config.is_some() {
            self.restart_sidecar().with_context(|| {
                format!(
                    "restarting CHIP sidecar after stuck-state error: {:#}",
                    first_error
                )
            })?;
        }
        self.ensure_sidecar().with_context(|| {
            format!(
                "re-initializing CHIP controller after stuck-state error: {:#}",
                first_error
            )
        })?;
        self.decode_rpc_response(self.send_rpc_envelope(request)?)
            .with_context(|| {
                format!(
                    "CHIP RPC retry failed after re-initializing controller: {:#}",
                    first_error
                )
            })
    }

    fn decode_rpc_response<T: serde::de::DeserializeOwned>(
        &self,
        response: ChipRpcResponseEnvelope,
    ) -> Result<T> {
        response.into_result()
    }

    fn send_rpc_envelope(&self, request: ChipRpcRequest) -> Result<ChipRpcResponseEnvelope> {
        let request_id = self.next_request_id.fetch_add(1, Ordering::SeqCst);
        let envelope = ChipRpcRequestEnvelope {
            id: request_id,
            request,
        };
        let rpc_timeout = rpc_timeout_for_request(&envelope.request);

        let mut stream = UnixStream::connect(&self.socket_path)
            .with_context(|| format!("connecting to {}", self.socket_path.display()))?;
        stream
            .set_read_timeout(Some(rpc_timeout))
            .context("setting CHIP RPC read timeout")?;
        stream
            .set_write_timeout(Some(rpc_timeout))
            .context("setting CHIP RPC write timeout")?;

        serde_json::to_writer(&mut stream, &envelope).with_context(|| {
            format!(
                "encoding CHIP RPC request (chipd status: {})",
                self.chipd_status_hint()
            )
        })?;
        stream.write_all(b"\n").with_context(|| {
            format!(
                "writing CHIP RPC newline (chipd status: {})",
                self.chipd_status_hint()
            )
        })?;
        stream.flush().with_context(|| {
            format!(
                "flushing CHIP RPC request (chipd status: {})",
                self.chipd_status_hint()
            )
        })?;

        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        let bytes = reader.read_line(&mut line).with_context(|| {
            format!(
                "reading CHIP RPC response (chipd status: {})",
                self.chipd_status_hint()
            )
        })?;
        if bytes == 0 {
            anyhow::bail!(
                "CHIP sidecar closed the socket without a response (chipd status: {})",
                self.chipd_status_hint()
            );
        }

        let response: ChipRpcResponseEnvelope =
            serde_json::from_str(line.trim_end()).context("decoding CHIP RPC response")?;
        if response.id != request_id {
            anyhow::bail!(
                "CHIP RPC response id mismatch: expected {}, got {}",
                request_id,
                response.id
            );
        }
        Ok(response)
    }

    fn can_connect(&self) -> Result<()> {
        UnixStream::connect(&self.socket_path)
            .map(|_| ())
            .with_context(|| format!("connecting to {}", self.socket_path.display()))
    }

    fn prepare_ble_commissioning(&self, request: &MatterCommissionRequest) -> Result<()> {
        if !uses_ble_commissioning(request) {
            return Ok(());
        }

        self.wait_for_ble_recovery_cooldown()?;
        ensure_linux_ble_commissioning_ready(self.init_request.ble_controller.unwrap_or(0))
            .inspect_err(|_error| {
                self.set_sidecar_health(SidecarHealth::Unavailable);
            })
    }

    fn wait_for_ble_recovery_cooldown(&self) -> Result<()> {
        let (delay, cooldown_finished) = {
            let mut ready_after = self
                .ble_commissioning_ready_after
                .lock()
                .map_err(|_| anyhow::anyhow!("Matter BLE recovery cooldown lock poisoned"))?;
            let now = Instant::now();
            match *ready_after {
                Some(deadline) if deadline > now => (Some(deadline - now), false),
                Some(_) => {
                    *ready_after = None;
                    (None, true)
                }
                None => (None, false),
            }
        };

        if let Some(delay) = delay {
            // Clamp: `delay` originates from a chipd-provided retry_after_ms
            // and this sleep runs while holding the commissioning lock — an
            // unclamped value from a buggy/older sidecar would wedge all
            // future pairing attempts.
            std::thread::sleep(delay.min(Duration::from_secs(60)));
            self.set_sidecar_health(SidecarHealth::Ready);
        } else if cooldown_finished {
            self.set_sidecar_health(SidecarHealth::Ready);
        }
        Ok(())
    }

    fn mark_ble_recovery_cooldown(&self, cooldown: Duration) {
        if let Ok(mut ready_after) = self.ble_commissioning_ready_after.lock() {
            *ready_after = Some(Instant::now() + cooldown);
        }
        self.set_sidecar_health(SidecarHealth::BleCooldown);
    }

    fn recover_ble_commissioning_stack(
        &self,
        first_error: &anyhow::Error,
        cooldown: Duration,
    ) -> Result<()> {
        self.recover_commissioning_sidecar(first_error, "Matter BLE commissioning error")?;
        self.mark_ble_recovery_cooldown(cooldown);
        Ok(())
    }

    fn recover_commissioning_sidecar(
        &self,
        first_error: &anyhow::Error,
        reason: &str,
    ) -> Result<()> {
        self.initialized.store(false, Ordering::SeqCst);
        self.set_sidecar_health(SidecarHealth::Recovering);

        if self.sidecar_config.is_some() {
            self.restart_sidecar().with_context(|| {
                format!("restarting CHIP sidecar after {reason}: {:#}", first_error)
            })?;
        }

        self.ensure_sidecar().with_context(|| {
            format!(
                "re-initializing CHIP controller after {reason}: {:#}",
                first_error
            )
        })?;
        Ok(())
    }

    fn scan_operational_node(
        &self,
        node_id: u64,
        timeout: Duration,
    ) -> Result<ChipRpcOperationalDiscoveryResponse> {
        self.call(ChipRpcRequest::ScanOperationalNode {
            node_id,
            timeout_ms: timeout.as_millis().try_into().unwrap_or(u64::MAX),
        })
    }

    fn recover_operational_discovery_failure(
        &self,
        node_id: u64,
        first_error: &anyhow::Error,
    ) -> Result<Option<CommissionedDevice>> {
        self.recover_commissioning_sidecar(first_error, "Matter operational discovery error")?;
        // The sidecar restart drops chipd's BLE connection to the bulb, so a
        // follow-up BLE pairing attempt needs the same BlueZ settle time as
        // the BLE recovery path.
        self.mark_ble_recovery_cooldown(ble_recovery_cooldown(first_error));

        let discovery =
            self.scan_operational_node(node_id, OPERATIONAL_RECOVERY_MDNS_SCAN_TIMEOUT)?;
        if discovery.fabrics.is_empty() {
            return Ok(None);
        }

        let response: ChipRpcProbeLightResponse =
            self.call(ChipRpcRequest::ProbeLight { node_id })?;
        Ok(Some(response.device))
    }

    /// One commissioning attempt, including the operational-discovery
    /// recovery that can still turn a post-join mDNS timeout into a success.
    /// BLE stack failures are left for the caller, which owns sidecar
    /// recovery and the single automatic retry.
    fn commission_light_once(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        let result: Result<ChipRpcCommissionLightResponse> =
            self.call(ChipRpcRequest::CommissionLight(request.clone()));
        match result {
            Ok(response) => Ok(response.device),
            // Operational discovery timed out after the device joined the
            // network: the usual cause is chipd's mDNS sockets going stale
            // after a wlan0 address change, which only a sidecar restart
            // fixes. Applies to every rendezvous mode — mDNS resolution is
            // required for on-network commissioning too.
            Err(error) if is_recoverable_operational_discovery_error(&error) => {
                match self.recover_operational_discovery_failure(request.node_id, &error) {
                    Ok(Some(device)) => Ok(device),
                    Ok(None) => Err(error.context(
                        "Matter operational discovery failed; reset CHIP sidecar and did not observe node advertising after recovery",
                    )),
                    Err(recovery_error) => Err(error.context(format!(
                        "Matter operational discovery failed and post-recovery probe failed: {:#}",
                        recovery_error
                    ))),
                }
            }
            Err(error) => Err(error),
        }
    }

    fn start_sidecar(&self) -> Result<()> {
        self.start_sidecar_inner(false)
    }

    fn restart_sidecar(&self) -> Result<()> {
        self.start_sidecar_inner(true)
    }

    fn start_sidecar_inner(&self, force_socket_reset: bool) -> Result<()> {
        let Some(config) = &self.sidecar_config else {
            return Ok(());
        };

        self.set_sidecar_health(SidecarHealth::Recovering);

        let mut guard = self
            .sidecar
            .lock()
            .map_err(|_| anyhow::anyhow!("CHIP sidecar process lock poisoned"))?;

        if let Some(mut child) = guard.take() {
            let _ = child.kill();
            let _ = child.wait();
        }

        if self.socket_path.exists() && (force_socket_reset || self.can_connect().is_err()) {
            let _ = fs::remove_file(&self.socket_path);
        }

        let (stdout, stderr) = sidecar_stdio(config.log_path.as_deref())?;
        let child = Command::new(&config.command)
            .arg("--socket")
            .arg(&self.socket_path)
            .current_dir(&config.working_dir)
            .stdin(Stdio::null())
            .stdout(stdout)
            .stderr(stderr)
            .spawn()
            .with_context(|| {
                format!(
                    "starting CHIP controller daemon with {}",
                    config.command.display()
                )
            })
            .inspect_err(|_error| {
                self.set_sidecar_health(SidecarHealth::Unavailable);
            })?;

        *guard = Some(child);
        drop(guard);

        let deadline = Instant::now() + SIDECAR_START_TIMEOUT;
        while Instant::now() < deadline {
            if self.can_connect().is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

        self.set_sidecar_health(SidecarHealth::Unavailable);
        anyhow::bail!(
            "Timed out waiting for CHIP sidecar socket at {}",
            self.socket_path.display()
        )
    }

    /// Non-blockingly report the spawned chipd process's state so RPC errors
    /// can distinguish "socket stalled" from "daemon crashed" and identify
    /// common kill signals (OOM, SIGSEGV, SIGABRT).
    fn chipd_status_hint(&self) -> String {
        use std::os::unix::process::ExitStatusExt;

        let Ok(mut guard) = self.sidecar.lock() else {
            return "lock poisoned".to_string();
        };
        let Some(child) = guard.as_mut() else {
            return "not spawned".to_string();
        };
        match child.try_wait() {
            Ok(None) => "running".to_string(),
            Ok(Some(status)) => {
                if let Some(signal) = status.signal() {
                    let name = match signal {
                        6 => " SIGABRT (assert/abort)",
                        9 => " SIGKILL (likely OOM-kill)",
                        11 => " SIGSEGV (segfault)",
                        15 => " SIGTERM",
                        _ => "",
                    };
                    format!("killed by signal {}{}", signal, name)
                } else if let Some(code) = status.code() {
                    format!("exited with code {}", code)
                } else {
                    format!("exited: {:?}", status)
                }
            }
            Err(err) => format!("try_wait failed: {}", err),
        }
    }
}

fn resolve_chipd_command() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("RHYTHM_MATTER_CHIPD") {
        return Ok(PathBuf::from(path));
    }

    if let Ok(current_exe) = std::env::current_exe() {
        if let Some(parent) = current_exe.parent() {
            let sibling = parent.join("rhythm-chipd");
            if sibling.is_file() {
                return Ok(sibling);
            }
        }
    }

    for candidate in ["/usr/local/bin/rhythm-chipd", "/usr/bin/rhythm-chipd"] {
        let path = PathBuf::from(candidate);
        if path.is_file() {
            return Ok(path);
        }
    }

    Ok(PathBuf::from("rhythm-chipd"))
}

fn sidecar_log_path_from_env(value: Option<OsString>) -> Option<PathBuf> {
    value.filter(|path| !path.is_empty()).map(PathBuf::from)
}

fn uses_ble_commissioning(request: &MatterCommissionRequest) -> bool {
    !matches!(
        request.rendezvous,
        crate::transport::MatterCommissioningRendezvous::OnNetwork
    )
}

#[cfg(all(target_os = "linux", not(test)))]
fn ensure_linux_ble_commissioning_ready(ble_controller: u16) -> Result<()> {
    let adapter_path = PathBuf::from(format!("/sys/class/bluetooth/hci{ble_controller}"));
    if !adapter_path.exists() {
        anyhow::bail!(
            "Matter BLE commissioning requires Bluetooth adapter hci{} to be present",
            ble_controller
        );
    }

    use dbus::blocking::stdintf::org_freedesktop_dbus::Properties;
    use dbus::blocking::Connection;

    const DBUS_TIMEOUT: Duration = Duration::from_secs(2);
    let connection = Connection::new_system()
        .context("Matter BLE commissioning requires the D-Bus system bus to be reachable")?;
    let bluez_path = format!("/org/bluez/hci{ble_controller}");
    let proxy = connection.with_proxy("org.bluez", bluez_path, DBUS_TIMEOUT);
    let powered: bool = proxy
        .get("org.bluez.Adapter1", "Powered")
        .with_context(|| {
            format!(
                "Matter BLE commissioning requires bluetoothd/BlueZ adapter hci{} to be present and reachable",
                ble_controller
            )
        })?;

    if !powered {
        anyhow::bail!(
            "Matter BLE commissioning requires Bluetooth adapter hci{} to be powered on",
            ble_controller
        );
    }

    Ok(())
}

#[cfg(any(not(target_os = "linux"), test))]
fn ensure_linux_ble_commissioning_ready(_ble_controller: u16) -> Result<()> {
    Ok(())
}

fn chip_storage_artifact_paths(chip_dir: &Path, requested_storage_path: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Some(stem) = requested_storage_path
        .file_stem()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
    {
        paths.push(chip_dir.join(format!(
            "{CHIP_EXAMPLE_STORAGE_PREFIX}.{stem}.{CHIP_EXAMPLE_STORAGE_EXT}"
        )));
    }
    paths.push(requested_storage_path.to_path_buf());
    paths.push(chip_dir.join(format!(
        "{CHIP_EXAMPLE_STORAGE_PREFIX}.{CHIP_EXAMPLE_STORAGE_EXT}"
    )));
    dedup_paths(paths)
}

fn dedup_paths(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut deduped = Vec::with_capacity(paths.len());
    for path in paths {
        if !deduped.iter().any(|existing| existing == &path) {
            deduped.push(path);
        }
    }
    deduped
}

fn sidecar_stdio(log_path: Option<&Path>) -> Result<(Stdio, Stdio)> {
    match log_path {
        Some(path) => {
            let stderr = open_sidecar_log_file(path)?;
            let stdout = stderr
                .try_clone()
                .with_context(|| format!("cloning Matter sidecar log file {}", path.display()))?;
            Ok((Stdio::from(stdout), Stdio::from(stderr)))
        }
        None => Ok((Stdio::inherit(), Stdio::inherit())),
    }
}

fn open_sidecar_log_file(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating Matter log dir {}", parent.display()))?;
    }

    OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("opening Matter sidecar log file {}", path.display()))
}

/// Returns true when an RPC error indicates the chipd controller has lost its
/// `mCommissioner` and a fresh `InitController` is required to recover.
///
/// chipd surfaces two distinct shapes:
///  * the C++ bridge wraps `CHIP_ERROR_INCORRECT_STATE` (0x00000003) — emitted
///    from `chip_bridge.cc` when `mCommissioner` is null;
///  * the Rust service emits `Controller not initialized` from
///    `service::require_initialized` when state was never set.
fn is_uninitialized_controller_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ChipRpcError>()
        .map(ChipRpcError::is_controller_uninitialized)
        .unwrap_or_else(|| {
            ChipRpcError::from_message(format!("{:#}", error)).is_controller_uninitialized()
        })
}

fn is_rpc_response_timeout(error: &anyhow::Error) -> bool {
    let reading_response = error
        .chain()
        .any(|cause| cause.to_string().contains("reading CHIP RPC response"));
    if !reading_response {
        return false;
    }

    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|error| matches!(error.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock))
            .unwrap_or(false)
    })
}

fn is_control_rpc_response_timeout(request: &ChipRpcRequest, error: &anyhow::Error) -> bool {
    rpc_timeout_for_request(request) == RPC_CONTROL_TIMEOUT && is_rpc_response_timeout(error)
}

fn is_recoverable_ble_commissioning_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ChipRpcError>()
        .map(ChipRpcError::is_recoverable_ble_commissioning_stack_failure)
        .unwrap_or_else(|| {
            ChipRpcError::from_message(format!("{:#}", error))
                .is_recoverable_ble_commissioning_stack_failure()
        })
}

fn is_recoverable_operational_discovery_error(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<ChipRpcError>()
        .map(ChipRpcError::is_recoverable_operational_discovery_failure)
        .unwrap_or_else(|| {
            ChipRpcError::from_message(format!("{:#}", error))
                .is_recoverable_operational_discovery_failure()
        })
}

fn ble_recovery_cooldown(error: &anyhow::Error) -> Duration {
    error
        .downcast_ref::<ChipRpcError>()
        .and_then(|rpc_error| rpc_error.retry_after_ms)
        .map(Duration::from_millis)
        .unwrap_or(BLE_RECOVERY_COOLDOWN)
}

impl Drop for ChipTransport {
    fn drop(&mut self) {
        if let Ok(mut guard) = self.sidecar.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

impl MatterTransport for ChipTransport {
    fn commission_light(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        let _guard = self
            .commissioning_lock
            .lock()
            .map_err(|_| anyhow::anyhow!("Matter commissioning lock poisoned"))?;

        self.prepare_ble_commissioning(request)?;

        let first_attempt_started = Instant::now();
        match self.commission_light_once(request) {
            Err(error)
                if uses_ble_commissioning(request)
                    && is_recoverable_ble_commissioning_error(&error) =>
            {
                if let Err(recovery_error) =
                    self.recover_ble_commissioning_stack(&error, ble_recovery_cooldown(&error))
                {
                    return Err(error.context(format!(
                        "Matter BLE commissioning failed and CHIP sidecar recovery failed: {:#}",
                        recovery_error
                    )));
                }

                // BLE discovery can lose a race against CHIP's fixed scan
                // window: the bulb is matched right as the window expires and
                // the in-flight connect is cancelled (issue #123). The same
                // request succeeds once the sidecar is reset and BlueZ has
                // settled, so retry once here instead of making the user do
                // it — but only when the failed attempt was quick enough that
                // a full retry still fits the app's pairing-request window.
                if first_attempt_started.elapsed() > self.ble_auto_retry_first_attempt_budget {
                    return Err(error.context(
                        "Matter BLE commissioning failed; reset CHIP sidecar before next attempt",
                    ));
                }

                tracing::warn!(
                    target: "pair",
                    event = "matter_ble_commission_auto_retry",
                    node_id = request.node_id,
                    error = %format!("{:#}", error),
                    "Matter BLE commissioning failed; reset CHIP sidecar and retrying once"
                );

                if let Err(prepare_error) = self.prepare_ble_commissioning(request) {
                    return Err(error.context(format!(
                        "Matter BLE commissioning failed; reset CHIP sidecar but the BLE stack was not ready for the automatic retry: {:#}",
                        prepare_error
                    )));
                }

                match self.commission_light_once(request) {
                    Ok(device) => Ok(device),
                    Err(retry_error)
                        if is_recoverable_ble_commissioning_error(&retry_error) =>
                    {
                        match self.recover_ble_commissioning_stack(
                            &retry_error,
                            ble_recovery_cooldown(&retry_error),
                        ) {
                            Ok(()) => Err(retry_error.context(
                                "Matter BLE commissioning failed after automatic retry; reset CHIP sidecar before next attempt",
                            )),
                            Err(recovery_error) => Err(retry_error.context(format!(
                                "Matter BLE commissioning failed after automatic retry and CHIP sidecar recovery failed: {:#}",
                                recovery_error
                            ))),
                        }
                    }
                    Err(retry_error) => Err(
                        retry_error.context("Matter BLE commissioning failed after automatic retry")
                    ),
                }
            }
            other => other,
        }
    }

    fn decommission_device(&self, node_id: u64, force: bool) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty =
            self.call(ChipRpcRequest::DecommissionDevice { node_id, force })?;
        Ok(())
    }

    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        let response: ChipRpcListDevicesResponse = self.call(ChipRpcRequest::ListDevices)?;
        Ok(response.devices)
    }

    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
        let response: ChipRpcProbeLightResponse =
            self.call(ChipRpcRequest::ProbeLight { node_id })?;
        Ok(response.device)
    }

    fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetOnOff {
            node_id,
            endpoint,
            on,
        })?;
        Ok(())
    }

    fn configure_group(&self, group: &MatterGroup) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::ConfigureGroup {
            group: group.clone(),
        })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "configure_group",
            group_id = group.group_id,
            name = %group.name,
            member_count = group.members.len(),
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::RemoveGroup {
            group_id,
            members: members.to_vec(),
        })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "remove_group",
            group_id,
            member_count = members.len(),
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn set_group_on_off(&self, group_id: u16, on: bool) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty =
            self.call(ChipRpcRequest::SetGroupOnOff { group_id, on })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "set_group_on_off",
            group_id,
            on,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn identify_group(&self, group_id: u16, duration_secs: u16) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::IdentifyGroup {
            group_id,
            duration_secs,
        })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "identify_group",
            group_id,
            duration_secs,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn set_group_brightness(
        &self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetGroupBrightness {
            group_id,
            level,
            transition_ms,
        })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "set_group_brightness",
            group_id,
            level,
            transition_ms = ?transition_ms,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn set_group_color_temperature(
        &self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty =
            self.call(ChipRpcRequest::SetGroupColorTemperature {
                group_id,
                kelvin,
                transition_ms,
            })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "set_group_color_temperature",
            group_id,
            kelvin,
            transition_ms = ?transition_ms,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn set_group_xy(
        &self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetGroupXy {
            group_id,
            x,
            y,
            transition_ms,
        })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "set_group_xy",
            group_id,
            x,
            y,
            transition_ms = ?transition_ms,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn set_group_hue_saturation(
        &self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty =
            self.call(ChipRpcRequest::SetGroupHueSaturation {
                group_id,
                hue,
                saturation,
                transition_ms,
            })?;
        tracing::info!(
            target: "cmd",
            event = "matter_group_rpc",
            operation = "set_group_hue_saturation",
            group_id,
            hue,
            saturation,
            transition_ms = ?transition_ms,
            "Matter group RPC ok"
        );
        Ok(())
    }

    fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::IdentifyLight {
            node_id,
            endpoint,
            duration_secs,
        })?;
        Ok(())
    }

    fn set_brightness(
        &self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetBrightness {
            node_id,
            endpoint,
            level,
            transition_ms,
        })?;
        Ok(())
    }

    fn run_level_command(
        &self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::RunLevelCommand {
            node_id,
            endpoint,
            command,
            level_or_step,
            step_mode,
            transition_ms,
        })?;
        Ok(())
    }

    fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetColorTemperature {
            node_id,
            endpoint,
            kelvin,
            transition_ms,
        })?;
        Ok(())
    }

    fn set_xy(
        &self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetXy {
            node_id,
            endpoint,
            x,
            y,
            transition_ms,
        })?;
        Ok(())
    }

    fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SetHueSaturation {
            node_id,
            endpoint,
            hue,
            saturation,
            transition_ms,
        })?;
        Ok(())
    }

    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
        let response: ChipRpcReadOnOffResponse =
            self.call(ChipRpcRequest::ReadOnOff { node_id, endpoint })?;
        Ok(response.on)
    }

    fn read_light_capability_snapshot(
        &self,
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value> {
        let response: ChipRpcJsonValueResponse =
            self.call(ChipRpcRequest::ReadLightCapabilitySnapshot { node_id, endpoint })?;
        Ok(response.value)
    }

    fn read_light_state(&self, node_id: u64, endpoint: u16) -> Result<serde_json::Value> {
        let response: ChipRpcJsonValueResponse =
            self.call(ChipRpcRequest::ReadLightState { node_id, endpoint })?;
        Ok(response.value)
    }

    fn subscribe_on_off(
        &self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        let _: crate::chip_rpc::ChipRpcEmpty = self.call(ChipRpcRequest::SubscribeOnOff {
            targets: targets.to_vec(),
            min_interval_secs,
            max_interval_secs,
        })?;
        Ok(())
    }

    fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
        let response: ChipRpcAttributeReportsResponse =
            self.call(ChipRpcRequest::DrainAttributeReports)?;
        Ok(response.reports)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    use crate::chip_rpc::{
        ChipInitControllerResponse, ChipRpcAttributeReportsResponse,
        ChipRpcCommissionLightResponse, ChipRpcEmpty, ChipRpcError, ChipRpcErrorKind,
        ChipRpcJsonValueResponse, ChipRpcListDevicesResponse, ChipRpcOperationalDiscoveryResponse,
        ChipRpcProbeLightResponse, ChipRpcReadOnOffResponse, ChipRpcResponse,
        ChipRpcResponseEnvelope,
    };
    use crate::transport::{
        MatterAttributeValue, MatterColorMode, MatterCommissioningNetwork,
        MatterCommissioningRendezvous, MatterCommissioningWifiCredentials,
    };

    fn test_temp_root() -> PathBuf {
        std::env::var_os("CARGO_TARGET_TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/tmp"))
    }

    fn temp_socket_path(name: &str) -> PathBuf {
        test_temp_root().join(format!(
            "rct-{}-{}-{}.sock",
            std::process::id(),
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    fn ble_commission_request() -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: "MT:Y.K908OC16750648G00".to_string(),
            node_id: 100,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Ble,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    #[test]
    fn control_and_read_requests_use_short_rpc_timeout() {
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::SetOnOff {
                node_id: 42,
                endpoint: 1,
                on: true,
            }),
            RPC_CONTROL_TIMEOUT
        );
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::SetBrightness {
                node_id: 42,
                endpoint: 1,
                level: 128,
                transition_ms: None,
            }),
            RPC_CONTROL_TIMEOUT
        );
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::ReadOnOff {
                node_id: 42,
                endpoint: 1,
            }),
            RPC_CONTROL_TIMEOUT
        );
    }

    #[test]
    fn commissioning_and_probe_requests_keep_long_rpc_timeout() {
        // Commissioning must exceed chipd's internal 180s kCommissioningTimeout
        // so the client never kills the sidecar mid-commission.
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::CommissionLight(ble_commission_request())),
            RPC_COMMISSION_TIMEOUT
        );
        assert!(RPC_COMMISSION_TIMEOUT > Duration::from_secs(180));
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::ProbeLight { node_id: 42 }),
            RPC_TIMEOUT
        );
        assert_eq!(
            rpc_timeout_for_request(&ChipRpcRequest::InitController(ChipInitControllerRequest {
                fabric_id: "test".to_string(),
                operational_fabric_id: 1,
                ipk_hex: "00".repeat(16),
                storage_path: "/tmp/rhythm-test".to_string(),
                ble_controller: None,
            },)),
            RPC_TIMEOUT
        );
    }

    fn commissioned_test_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Test Vendor".to_string(),
            product_name: "Test Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
        }
    }

    fn legacy_rpc_error(id: u64, message: impl Into<String>) -> ChipRpcResponseEnvelope {
        ChipRpcResponseEnvelope {
            id,
            response: ChipRpcResponse::Error {
                error: ChipRpcError {
                    message: message.into(),
                    kind: ChipRpcErrorKind::Other,
                    recoverable: false,
                    requires_restart: false,
                    retry_after_ms: None,
                },
            },
        }
    }

    fn spawn_fake_server<F>(socket_path: PathBuf, handler: F) -> thread::JoinHandle<()>
    where
        F: Fn(ChipRpcRequestEnvelope) -> ChipRpcResponseEnvelope + Send + 'static,
    {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            ready_tx.send(()).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            let request: ChipRpcRequestEnvelope = serde_json::from_str(line.trim_end()).unwrap();
            let response = handler(request);
            let mut stream = reader.into_inner();
            serde_json::to_writer(&mut stream, &response).unwrap();
            stream.write_all(b"\n").unwrap();
            stream.flush().unwrap();
        });
        ready_rx.recv().unwrap();
        handle
    }

    fn spawn_raw_server(socket_path: PathBuf, response: Option<String>) -> thread::JoinHandle<()> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            ready_tx.send(()).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if let Some(response) = response {
                let mut stream = reader.into_inner();
                stream.write_all(response.as_bytes()).unwrap();
                stream.write_all(b"\n").unwrap();
                stream.flush().unwrap();
            }
        });
        ready_rx.recv().unwrap();
        handle
    }

    fn spawn_hanging_server(socket_path: PathBuf, hold_for: Duration) -> thread::JoinHandle<()> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            ready_tx.send(()).unwrap();
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            std::thread::sleep(hold_for);
        });
        ready_rx.recv().unwrap();
        handle
    }

    fn on_network_commission_request() -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: "12345678901".to_string(),
            node_id: 101,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::OnNetwork,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    #[test]
    fn list_devices_uses_rpc_contract() {
        let socket_path = temp_socket_path("list-devices");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            assert!(matches!(request.request, ChipRpcRequest::ListDevices));
            ChipRpcResponseEnvelope::ok(
                request.id,
                ChipRpcListDevicesResponse {
                    devices: vec![MatterDeviceInfo {
                        node_id: 42,
                        vendor_name: "Vendor".to_string(),
                        product_name: "Lamp".to_string(),
                        reachable: true,
                    }],
                },
            )
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        let devices = transport.list_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].node_id, 42);

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn rpc_errors_are_surfaceable() {
        let socket_path = temp_socket_path("error");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            assert!(matches!(request.request, ChipRpcRequest::SetOnOff { .. }));
            ChipRpcResponseEnvelope::error(request.id, "boom")
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(error.to_string().contains("boom"));

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn rpc_socket_edge_cases_surface_precise_errors() {
        let closed_socket = temp_socket_path("closed-response");
        let server = spawn_raw_server(closed_socket.clone(), None);
        let transport = ChipTransport::for_test(closed_socket.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(error
            .to_string()
            .contains("closed the socket without a response"));
        server.join().unwrap();
        let _ = fs::remove_file(closed_socket);

        let malformed_socket = temp_socket_path("malformed-response");
        let server = spawn_raw_server(malformed_socket.clone(), Some("not-json".to_string()));
        let transport = ChipTransport::for_test(malformed_socket.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(format!("{error:#}").contains("decoding CHIP RPC response"));
        server.join().unwrap();
        let _ = fs::remove_file(malformed_socket);

        let mismatch_socket = temp_socket_path("id-mismatch");
        let server = spawn_fake_server(mismatch_socket.clone(), |request| {
            ChipRpcResponseEnvelope::ok(request.id + 1, ChipRpcEmpty::new())
        });
        let transport = ChipTransport::for_test(mismatch_socket.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(error.to_string().contains("response id mismatch"));
        server.join().unwrap();
        let _ = fs::remove_file(mismatch_socket);
    }

    #[test]
    fn rpc_response_timeout_does_not_mark_sidecar_unavailable() {
        let socket_path = temp_socket_path("response-timeout");
        let server = spawn_hanging_server(
            socket_path.clone(),
            RPC_CONTROL_TIMEOUT + Duration::from_millis(50),
        );
        let transport = ChipTransport::for_test(socket_path.clone());

        let error = transport.set_on_off(1, 1, true).unwrap_err();

        assert!(
            format!("{error:#}").contains("reading CHIP RPC response"),
            "unexpected error: {error:#}"
        );
        assert!(
            is_rpc_response_timeout(&error),
            "expected timeout classifier to match: {error:#}"
        );
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::Ready,
            "an offline device response timeout should not make the CHIP sidecar look dead"
        );
        assert!(
            transport.initialized.load(Ordering::SeqCst),
            "an offline device response timeout should not drop the initialized controller flag"
        );

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn initialize_controller_validates_fabric_identity() {
        let wrong_fabric_socket = temp_socket_path("wrong-fabric");
        let server = spawn_fake_server(wrong_fabric_socket.clone(), |request| {
            assert!(matches!(request.request, ChipRpcRequest::InitController(_)));
            ChipRpcResponseEnvelope::ok(
                request.id,
                ChipInitControllerResponse {
                    fabric_id: "other".to_string(),
                    operational_fabric_id: 1,
                    compressed_fabric_id: None,
                },
            )
        });
        let transport = ChipTransport::for_test(wrong_fabric_socket.clone());
        let error = transport.initialize_controller().unwrap_err();
        assert!(error.to_string().contains("initialized unexpected fabric"));
        server.join().unwrap();
        let _ = fs::remove_file(wrong_fabric_socket);

        let wrong_operational_socket = temp_socket_path("wrong-operational-fabric");
        let server = spawn_fake_server(wrong_operational_socket.clone(), |request| {
            assert!(matches!(request.request, ChipRpcRequest::InitController(_)));
            ChipRpcResponseEnvelope::ok(
                request.id,
                ChipInitControllerResponse {
                    fabric_id: "test".to_string(),
                    operational_fabric_id: 2,
                    compressed_fabric_id: None,
                },
            )
        });
        let transport = ChipTransport::for_test(wrong_operational_socket.clone());
        let error = transport.initialize_controller().unwrap_err();
        assert!(error
            .to_string()
            .contains("unexpected operational fabric id"));
        server.join().unwrap();
        let _ = fs::remove_file(wrong_operational_socket);
    }

    #[test]
    fn unavailable_socket_paths_update_health_without_managed_sidecar() {
        let missing_socket = temp_socket_path("missing-socket");
        let transport = ChipTransport::for_test(missing_socket.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(format!("{error:#}").contains("connecting to"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::Unavailable
        );

        let stale_socket_file = temp_socket_path("stale-socket-file");
        fs::write(&stale_socket_file, b"not a socket").unwrap();
        let transport = ChipTransport::for_test(stale_socket_file.clone());
        let error = transport.set_on_off(1, 1, true).unwrap_err();
        assert!(format!("{error:#}").contains("connecting to"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::Unavailable
        );
        let _ = fs::remove_file(stale_socket_file);
    }

    #[test]
    fn decommission_maps_to_empty_response() {
        let socket_path = temp_socket_path("decommission");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            assert!(matches!(
                request.request,
                ChipRpcRequest::DecommissionDevice {
                    node_id: 77,
                    force: true
                }
            ));
            ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport.decommission_device(77, true).unwrap();

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn set_hue_saturation_uses_rpc_contract() {
        let socket_path = temp_socket_path("set-hue-saturation");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            assert!(matches!(
                request.request,
                ChipRpcRequest::SetHueSaturation {
                    node_id: 7,
                    endpoint: 1,
                    hue: 12,
                    saturation: 200,
                    transition_ms: Some(500),
                }
            ));
            ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport
            .set_hue_saturation(7, 1, 12, 200, Some(500))
            .unwrap();

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn identify_light_uses_rpc_contract() {
        let socket_path = temp_socket_path("identify-light");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            assert!(matches!(
                request.request,
                ChipRpcRequest::IdentifyLight {
                    node_id: 7,
                    endpoint: 1,
                    duration_secs: 1,
                }
            ));
            ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport.identify_light(7, 1, 1).unwrap();

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn subscribe_on_off_uses_rpc_contract() {
        let socket_path = temp_socket_path("subscribe-on-off");
        let server = spawn_fake_server(socket_path.clone(), |request| {
            match request.request {
                ChipRpcRequest::SubscribeOnOff {
                    targets,
                    min_interval_secs,
                    max_interval_secs,
                } => {
                    assert_eq!(
                        targets,
                        vec![MatterSubscriptionTarget {
                            node_id: 7,
                            endpoint: 2,
                        }]
                    );
                    assert_eq!(min_interval_secs, 1);
                    assert_eq!(max_interval_secs, 60);
                }
                other => panic!("unexpected request: {:?}", other),
            }
            ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport
            .subscribe_on_off(
                &[MatterSubscriptionTarget {
                    node_id: 7,
                    endpoint: 2,
                }],
                1,
                60,
            )
            .unwrap();

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn drain_attribute_reports_uses_rpc_contract() {
        let socket_path = temp_socket_path("drain-attr-reports");
        let expected = MatterAttributeReport {
            node_id: 7,
            endpoint: 2,
            cluster: crate::clusters::CLUSTER_ON_OFF_U32,
            attr_id: crate::clusters::ATTR_ON_OFF_U32,
            value: MatterAttributeValue::Bool(true),
        };
        let response_report = expected.clone();
        let server = spawn_fake_server(socket_path.clone(), move |request| {
            assert!(matches!(
                request.request,
                ChipRpcRequest::DrainAttributeReports
            ));
            ChipRpcResponseEnvelope::ok(
                request.id,
                ChipRpcAttributeReportsResponse {
                    reports: vec![response_report.clone()],
                },
            )
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        let reports = transport.drain_attribute_reports().unwrap();
        assert_eq!(reports, vec![expected]);

        server.join().unwrap();
        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn remaining_matter_transport_methods_map_to_rpc_contracts() {
        let socket_path = temp_socket_path("remaining-rpc-methods");
        let expected_group = MatterGroup {
            group_id: 0x8001,
            name: "Kitchen".to_string(),
            members: vec![
                MatterGroupMember {
                    node_id: 7,
                    endpoint: 1,
                },
                MatterGroupMember {
                    node_id: 8,
                    endpoint: 2,
                },
            ],
        };
        let expected_group_for_server = expected_group.clone();
        let expected_members = expected_group.members.clone();
        let expected_members_for_server = expected_members.clone();
        let server =
            spawn_fake_server_multi(socket_path.clone(), 16, move |request| {
                match &request.request {
                    ChipRpcRequest::ConfigureGroup { group } => {
                        assert_eq!(group, &expected_group_for_server);
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::RemoveGroup { group_id, members } => {
                        assert_eq!(*group_id, 0x8001);
                        assert_eq!(members, &expected_members_for_server);
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetGroupOnOff { group_id, on } => {
                        assert_eq!((*group_id, *on), (0x8001, true));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::IdentifyGroup {
                        group_id,
                        duration_secs,
                    } => {
                        assert_eq!((*group_id, *duration_secs), (0x8001, 3));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetGroupBrightness {
                        group_id,
                        level,
                        transition_ms,
                    } => {
                        assert_eq!(
                            (*group_id, *level, *transition_ms),
                            (0x8001, 128, Some(250))
                        );
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetGroupColorTemperature {
                        group_id,
                        kelvin,
                        transition_ms,
                    } => {
                        assert_eq!((*group_id, *kelvin, *transition_ms), (0x8001, 3000, None));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetGroupXy {
                        group_id,
                        x,
                        y,
                        transition_ms,
                    } => {
                        assert_eq!(*group_id, 0x8001);
                        assert!((*x - 0.31).abs() < f32::EPSILON);
                        assert!((*y - 0.42).abs() < f32::EPSILON);
                        assert_eq!(*transition_ms, Some(400));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetGroupHueSaturation {
                        group_id,
                        hue,
                        saturation,
                        transition_ms,
                    } => {
                        assert_eq!(
                            (*group_id, *hue, *saturation, *transition_ms),
                            (0x8001, 20, 210, None)
                        );
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetBrightness {
                        node_id,
                        endpoint,
                        level,
                        transition_ms,
                    } => {
                        assert_eq!(
                            (*node_id, *endpoint, *level, *transition_ms),
                            (7, 2, 180, Some(500))
                        );
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::RunLevelCommand {
                        node_id,
                        endpoint,
                        command,
                        level_or_step,
                        step_mode,
                        transition_ms,
                    } => {
                        assert_eq!((*node_id, *endpoint), (7, 2));
                        assert_eq!(*command, MatterLevelCommandVariant::StepWithOnOff);
                        assert_eq!(*level_or_step, 9);
                        assert_eq!(*step_mode, Some(MatterLevelStepMode::Up));
                        assert_eq!(*transition_ms, Some(125));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetColorTemperature {
                        node_id,
                        endpoint,
                        kelvin,
                        transition_ms,
                    } => {
                        assert_eq!(
                            (*node_id, *endpoint, *kelvin, *transition_ms),
                            (7, 2, 2700, None)
                        );
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::SetXy {
                        node_id,
                        endpoint,
                        x,
                        y,
                        transition_ms,
                    } => {
                        assert_eq!((*node_id, *endpoint), (7, 2));
                        assert!((*x - 0.12).abs() < f32::EPSILON);
                        assert!((*y - 0.34).abs() < f32::EPSILON);
                        assert_eq!(*transition_ms, Some(600));
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                    ChipRpcRequest::ReadOnOff { node_id, endpoint } => {
                        assert_eq!((*node_id, *endpoint), (7, 2));
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcReadOnOffResponse { on: true },
                        )
                    }
                    ChipRpcRequest::ReadLightCapabilitySnapshot { node_id, endpoint } => {
                        assert_eq!((*node_id, *endpoint), (7, 2));
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcJsonValueResponse {
                                value: serde_json::json!({ "capability": "snapshot" }),
                            },
                        )
                    }
                    ChipRpcRequest::ReadLightState { node_id, endpoint } => {
                        assert_eq!((*node_id, *endpoint), (7, 2));
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcJsonValueResponse {
                                value: serde_json::json!({ "on": true }),
                            },
                        )
                    }
                    ChipRpcRequest::ProbeLight { node_id } => {
                        assert_eq!(*node_id, 7);
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcProbeLightResponse {
                                device: commissioned_test_device(7),
                            },
                        )
                    }
                    other => panic!("unexpected RPC request: {:?}", other),
                }
            });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport.configure_group(&expected_group).unwrap();
        transport.remove_group(0x8001, &expected_members).unwrap();
        transport.set_group_on_off(0x8001, true).unwrap();
        transport.identify_group(0x8001, 3).unwrap();
        transport
            .set_group_brightness(0x8001, 128, Some(250))
            .unwrap();
        transport
            .set_group_color_temperature(0x8001, 3000, None)
            .unwrap();
        transport
            .set_group_xy(0x8001, 0.31, 0.42, Some(400))
            .unwrap();
        transport
            .set_group_hue_saturation(0x8001, 20, 210, None)
            .unwrap();
        transport.set_brightness(7, 2, 180, Some(500)).unwrap();
        transport
            .run_level_command(
                7,
                2,
                MatterLevelCommandVariant::StepWithOnOff,
                9,
                Some(MatterLevelStepMode::Up),
                Some(125),
            )
            .unwrap();
        transport.set_color_temperature(7, 2, 2700, None).unwrap();
        transport.set_xy(7, 2, 0.12, 0.34, Some(600)).unwrap();
        assert!(transport.read_on_off(7, 2).unwrap());
        assert_eq!(
            transport.read_light_capability_snapshot(7, 2).unwrap(),
            serde_json::json!({ "capability": "snapshot" })
        );
        assert_eq!(
            transport.read_light_state(7, 2).unwrap(),
            serde_json::json!({ "on": true })
        );
        assert_eq!(transport.probe_light(7).unwrap().node_id, 7);

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 16);
        let _ = fs::remove_file(socket_path);
    }

    fn spawn_fake_server_multi<F>(
        socket_path: PathBuf,
        connection_count: usize,
        handler: F,
    ) -> thread::JoinHandle<Vec<ChipRpcRequestEnvelope>>
    where
        F: Fn(&ChipRpcRequestEnvelope) -> ChipRpcResponseEnvelope + Send + 'static,
    {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let handle = thread::spawn(move || {
            let _ = fs::remove_file(&socket_path);
            let listener = UnixListener::bind(&socket_path).unwrap();
            ready_tx.send(()).unwrap();
            let mut requests = Vec::with_capacity(connection_count);
            while requests.len() < connection_count {
                let (stream, _) = listener.accept().unwrap();
                let mut reader = BufReader::new(stream);
                let mut line = String::new();
                let bytes = reader.read_line(&mut line).unwrap();
                // `can_connect` opens probe connections that close immediately
                // without sending a payload — skip those rather than treating
                // them as malformed RPC requests.
                if bytes == 0 {
                    continue;
                }
                let request: ChipRpcRequestEnvelope =
                    serde_json::from_str(line.trim_end()).unwrap();
                let response = handler(&request);
                let mut stream = reader.into_inner();
                serde_json::to_writer(&mut stream, &response).unwrap();
                stream.write_all(b"\n").unwrap();
                stream.flush().unwrap();
                requests.push(request);
            }
            requests
        });
        ready_rx.recv().unwrap();
        handle
    }

    /// Regression for issue #48: chipd lost `mCommissioner` after an OTA reboot
    /// and started returning `CHIP Error 0x00000003: Incorrect state`. The
    /// transport never reset its `initialized` flag, so every subsequent Matter
    /// command failed forever. After the fix, an `Incorrect state` reply
    /// triggers a re-init and a single retry.
    #[test]
    fn incorrect_state_error_triggers_reinit_and_retry() {
        let socket_path = temp_socket_path("recover-incorrect-state");
        let server =
            spawn_fake_server_multi(socket_path.clone(), 3, |request| match &request.request {
                ChipRpcRequest::SetOnOff { .. } => {
                    // First connection: report the chipd-bridge stuck state.
                    // Subsequent connections (after re-init) should succeed.
                    static CALL_COUNT: std::sync::atomic::AtomicUsize =
                        std::sync::atomic::AtomicUsize::new(0);
                    let attempt = CALL_COUNT.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if attempt == 0 {
                        ChipRpcResponseEnvelope::error(
                            request.id,
                            "setting Matter on/off: native/chip_bridge.cc:1014: \
                         CHIP Error 0x00000003: Incorrect state",
                        )
                    } else {
                        ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                    }
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            });

        let transport = ChipTransport::for_test(socket_path.clone());
        transport
            .set_on_off(102, 1, true)
            .expect("set_on_off should succeed after stuck-state recovery");

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::SetOnOff { .. } => "set_on_off",
                ChipRpcRequest::InitController(_) => "init_controller",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["set_on_off", "init_controller", "set_on_off"],
            "expected the transport to send the original request, then InitController to re-init, then retry the request"
        );

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn ble_commissioning_error_retries_once_then_reinitializes_controller() {
        const BLUEZ_ENDPOINT_ERROR: &str = concat!(
            "commissioning Matter light: ",
            "src/platform/Linux/bluez/BluezEndpoint.cpp:623: ",
            "CHIP Error 0x000000AC: Internal error"
        );

        let socket_path = temp_socket_path("recover-ble-commissioning");
        let commission_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = commission_attempts.clone();
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    observed_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ChipRpcResponseEnvelope {
                        id: request.id,
                        response: ChipRpcResponse::Error {
                            error: ChipRpcError {
                                message: BLUEZ_ENDPOINT_ERROR.to_string(),
                                kind: ChipRpcErrorKind::BleCommissioningStack,
                                recoverable: true,
                                requires_restart: true,
                                retry_after_ms: Some(1),
                            },
                        },
                    }
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                ChipRpcRequest::SetOnOff { .. } => {
                    ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                }
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 5, handler);

        let transport = ChipTransport::for_test(socket_path.clone());
        let error = transport
            .commission_light(&ble_commission_request())
            .expect_err("commissioning should surface the BLE failure after the automatic retry");
        assert!(format!("{:#}", error).contains("after automatic retry"));
        assert!(format!("{:#}", error).contains("reset CHIP sidecar"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown,
            "recoverable BLE failures should leave the sidecar initialized but gate the next BLE attempt"
        );

        transport
            .set_on_off(102, 1, true)
            .expect("subsequent commands should use the re-initialized controller");

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                ChipRpcRequest::SetOnOff { .. } => "set_on_off",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "commission_light",
                "init_controller",
                "commission_light",
                "init_controller",
                "set_on_off"
            ],
            "expected the failed commission to reset, retry once, then reset again for the next attempt"
        );
        assert_eq!(
            commission_attempts.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "commissioning should be retried exactly once after a BLE stack failure"
        );

        let _ = fs::remove_file(socket_path);
    }

    /// Regression for issue #123: the bulb was BLE-discovered with a
    /// discriminator match right as CHIP's fixed scan window expired, so the
    /// in-flight connect was cancelled and the attempt surfaced
    /// `BLEManagerImpl.cpp:887: CHIP Error 0x00000032: Timeout`. The user's
    /// manual second attempt succeeded ~35s later. The transport now runs
    /// that second attempt itself after the sidecar recovery.
    #[test]
    fn ble_discovery_timeout_auto_retry_recovers_commissioning() {
        const BLE_SCAN_TIMEOUT_ERROR: &str = concat!(
            "commissioning Matter light: ",
            "src/platform/Linux/BLEManagerImpl.cpp:887: ",
            "CHIP Error 0x00000032: Timeout"
        );

        let socket_path = temp_socket_path("ble-scan-timeout-auto-retry");
        let commission_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = commission_attempts.clone();
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    let attempt =
                        observed_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if attempt == 0 {
                        // Legacy string-shaped error, exactly as chipd
                        // surfaced it in the issue #123 debug bundle.
                        ChipRpcResponseEnvelope::error(request.id, BLE_SCAN_TIMEOUT_ERROR)
                    } else {
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcCommissionLightResponse {
                                device: commissioned_test_device(106),
                            },
                        )
                    }
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                other => panic!("unexpected RPC during auto-retry test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 3, handler);

        let transport = ChipTransport::for_test(socket_path.clone());
        let device = transport
            .commission_light(&ble_commission_request())
            .expect("a transient BLE discovery timeout should be recovered by the automatic retry");
        assert_eq!(device.node_id, 106);
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::Ready,
            "a successful automatic retry should leave the sidecar ready"
        );

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["commission_light", "init_controller", "commission_light"],
            "expected the failed commission to reset the controller and retry once"
        );
        assert_eq!(
            commission_attempts.load(std::sync::atomic::Ordering::SeqCst),
            2
        );

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn slow_ble_commissioning_failure_is_not_auto_retried() {
        let socket_path = temp_socket_path("slow-ble-failure-no-retry");
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    std::thread::sleep(Duration::from_millis(50));
                    ChipRpcResponseEnvelope {
                        id: request.id,
                        response: ChipRpcResponse::Error {
                            error: ChipRpcError {
                                message: "commissioning failed".to_string(),
                                kind: ChipRpcErrorKind::BleCommissioningStack,
                                recoverable: true,
                                requires_restart: true,
                                retry_after_ms: Some(1),
                            },
                        },
                    }
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                other => panic!("unexpected RPC during budget test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 2, handler);

        let mut transport = ChipTransport::for_test(socket_path.clone());
        transport.ble_auto_retry_first_attempt_budget = Duration::from_millis(10);
        let error = transport
            .commission_light(&ble_commission_request())
            .expect_err("a slow BLE failure should be surfaced without an automatic retry");
        assert!(format!("{:#}", error).contains("reset CHIP sidecar before next attempt"));
        assert!(!format!("{:#}", error).contains("after automatic retry"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown
        );

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["commission_light", "init_controller"],
            "a first attempt past the retry budget should only reset the controller"
        );

        let _ = fs::remove_file(socket_path);
    }

    /// Regression for issue #117: a wlan0 address change left chipd's mDNS
    /// sockets bound to a stale address, so commissioning succeeded through
    /// WiFiNetworkEnable but timed out at operational discovery
    /// (AddressResolve). The transport treated the error as non-recoverable
    /// and never restarted the wedged sidecar, so every retry failed the same
    /// way until a manual reboot.
    #[test]
    fn operational_discovery_timeout_reinitializes_controller_without_retrying_commission() {
        const ADDRESS_RESOLVE_TIMEOUT_ERROR: &str = concat!(
            "commissioning Matter light: ",
            "src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: ",
            "CHIP Error 0x00000032: Timeout"
        );

        let socket_path = temp_socket_path("recover-operational-discovery");
        let commission_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = commission_attempts.clone();
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    observed_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ChipRpcResponseEnvelope::error(request.id, ADDRESS_RESOLVE_TIMEOUT_ERROR)
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                ChipRpcRequest::ScanOperationalNode {
                    node_id,
                    timeout_ms,
                } => {
                    assert_eq!(*node_id, 100);
                    assert_eq!(
                        *timeout_ms,
                        OPERATIONAL_RECOVERY_MDNS_SCAN_TIMEOUT.as_millis() as u64
                    );
                    ChipRpcResponseEnvelope::ok(
                        request.id,
                        ChipRpcOperationalDiscoveryResponse {
                            node_id: *node_id,
                            fabrics: Vec::new(),
                        },
                    )
                }
                ChipRpcRequest::SetOnOff { .. } => {
                    ChipRpcResponseEnvelope::ok(request.id, ChipRpcEmpty::new())
                }
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 4, handler);

        let transport = ChipTransport::for_test(socket_path.clone());
        let error = transport
            .commission_light(&ble_commission_request())
            .expect_err("commissioning should surface the original discovery failure");
        assert!(format!("{:#}", error).contains("reset CHIP sidecar"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown,
            "operational discovery recovery should arm the BLE settle cooldown"
        );

        transport
            .set_on_off(102, 1, true)
            .expect("subsequent commands should use the re-initialized controller");

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                ChipRpcRequest::ScanOperationalNode { .. } => "scan_operational_node",
                ChipRpcRequest::SetOnOff { .. } => "set_on_off",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "commission_light",
                "init_controller",
                "scan_operational_node",
                "set_on_off"
            ],
            "expected the failed commission to reset and scan for the node without retrying the commission"
        );
        assert_eq!(
            commission_attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "commissioning should not be retried automatically after an operational discovery failure"
        );

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn operational_discovery_timeout_completes_when_recovered_node_advertises_and_probes() {
        const ADDRESS_RESOLVE_TIMEOUT_ERROR: &str = concat!(
            "commissioning Matter light: ",
            "src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: ",
            "CHIP Error 0x00000032: Timeout"
        );

        let socket_path = temp_socket_path("recover-operational-discovery-probe");
        let commission_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = commission_attempts.clone();
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    observed_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    ChipRpcResponseEnvelope::error(request.id, ADDRESS_RESOLVE_TIMEOUT_ERROR)
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: Some("F800F5FBD9C145CD".to_string()),
                    },
                ),
                ChipRpcRequest::ScanOperationalNode {
                    node_id,
                    timeout_ms,
                } => {
                    assert_eq!(*node_id, 100);
                    assert_eq!(
                        *timeout_ms,
                        OPERATIONAL_RECOVERY_MDNS_SCAN_TIMEOUT.as_millis() as u64
                    );
                    ChipRpcResponseEnvelope::ok(
                        request.id,
                        ChipRpcOperationalDiscoveryResponse {
                            node_id: *node_id,
                            fabrics: vec!["F800F5FBD9C145CD".to_string()],
                        },
                    )
                }
                ChipRpcRequest::ProbeLight { node_id } => {
                    assert_eq!(*node_id, 100);
                    ChipRpcResponseEnvelope::ok(
                        request.id,
                        ChipRpcProbeLightResponse {
                            device: commissioned_test_device(*node_id),
                        },
                    )
                }
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 4, handler);

        let transport = ChipTransport::for_test(socket_path.clone());
        let device = transport
            .commission_light(&ble_commission_request())
            .expect("advertising recovered node should be probed and returned");
        assert_eq!(device.node_id, 100);
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown,
            "operational discovery recovery should arm the BLE settle cooldown"
        );

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                ChipRpcRequest::ScanOperationalNode { .. } => "scan_operational_node",
                ChipRpcRequest::ProbeLight { .. } => "probe_light",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec![
                "commission_light",
                "init_controller",
                "scan_operational_node",
                "probe_light"
            ],
            "expected the failed commission to reset, scan, and probe without retrying the commission"
        );
        assert_eq!(
            commission_attempts.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "commissioning should not be retried automatically after an operational discovery failure"
        );

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn legacy_ble_error_response_without_metadata_still_triggers_recovery() {
        const BLUEZ_OBJECT_MANAGER_ERROR: &str = concat!(
            "commissioning Matter light: ",
            "src/platform/Linux/bluez/BluezObjectManager.cpp:118: ",
            "CHIP Error 0x000000AC: Internal error"
        );

        let socket_path = temp_socket_path("legacy-ble-recovery");
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    legacy_rpc_error(request.id, BLUEZ_OBJECT_MANAGER_ERROR)
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 2, handler);

        // Zero retry budget: this test pins legacy-error classification and
        // recovery, not the automatic retry (covered elsewhere) — and the
        // legacy error's implied 3s cooldown would slow the retry path down.
        let mut transport = ChipTransport::for_test(socket_path.clone());
        transport.ble_auto_retry_first_attempt_budget = Duration::ZERO;
        let error = transport
            .commission_light(&ble_commission_request())
            .expect_err("legacy BLE error response should surface the original failure");
        assert!(format!("{:#}", error).contains("reset CHIP sidecar"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown
        );

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                _ => "other",
            })
            .collect();
        assert_eq!(kinds, vec!["commission_light", "init_controller"]);

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn ble_recovery_cooldown_uses_rpc_retry_after_metadata() {
        let typed_error = anyhow::Error::new(ChipRpcError {
            message: "commissioning failed".to_string(),
            kind: ChipRpcErrorKind::BleCommissioningStack,
            recoverable: true,
            requires_restart: true,
            retry_after_ms: Some(125),
        });
        assert_eq!(
            ble_recovery_cooldown(&typed_error),
            Duration::from_millis(125)
        );

        let legacy_error = anyhow::anyhow!(
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error"
        );
        assert_eq!(ble_recovery_cooldown(&legacy_error), BLE_RECOVERY_COOLDOWN);
    }

    #[test]
    fn typed_ble_error_requires_recoverable_restart_metadata() {
        for error in [
            ChipRpcError {
                message: "commissioning failed".to_string(),
                kind: ChipRpcErrorKind::BleCommissioningStack,
                recoverable: false,
                requires_restart: true,
                retry_after_ms: Some(1),
            },
            ChipRpcError {
                message: "commissioning failed".to_string(),
                kind: ChipRpcErrorKind::BleCommissioningStack,
                recoverable: true,
                requires_restart: false,
                retry_after_ms: Some(1),
            },
        ] {
            assert!(
                !is_recoverable_ble_commissioning_error(&anyhow::Error::new(error)),
                "BLE stack kind without both recovery flags must not reset the sidecar"
            );
        }
    }

    #[test]
    fn non_ble_commissioning_error_does_not_reinitialize_or_enter_cooldown() {
        let socket_path = temp_socket_path("non-ble-commission-error");
        let server = spawn_fake_server_multi(socket_path.clone(), 1, |request| {
            assert!(matches!(
                request.request,
                ChipRpcRequest::CommissionLight(_)
            ));
            ChipRpcResponseEnvelope::error(
                request.id,
                "commissioning Matter light: CHIP Error 0x00000032: Timeout",
            )
        });

        let transport = ChipTransport::for_test(socket_path.clone());
        let error = transport
            .commission_light(&ble_commission_request())
            .expect_err(
                "generic commissioning timeout should be surfaced without sidecar recovery",
            );
        assert!(!format!("{:#}", error).contains("reset CHIP sidecar"));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::Ready,
            "non-BLE failures should leave sidecar health unchanged"
        );

        let requests = server.join().unwrap();
        assert_eq!(requests.len(), 1);
        assert!(matches!(
            requests[0].request,
            ChipRpcRequest::CommissionLight(_)
        ));

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn typed_ble_commissioning_error_uses_retry_after_for_automatic_retry() {
        let socket_path = temp_socket_path("typed-ble-recovery-cooldown");
        let commission_attempts = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let observed_attempts = commission_attempts.clone();
        let handler = move |request: &ChipRpcRequestEnvelope| -> ChipRpcResponseEnvelope {
            match &request.request {
                ChipRpcRequest::CommissionLight(_) => {
                    let attempt =
                        observed_attempts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    if attempt == 0 {
                        ChipRpcResponseEnvelope {
                            id: request.id,
                            response: ChipRpcResponse::Error {
                                error: ChipRpcError {
                                    message: "commissioning failed".to_string(),
                                    kind: ChipRpcErrorKind::BleCommissioningStack,
                                    recoverable: true,
                                    requires_restart: true,
                                    retry_after_ms: Some(1),
                                },
                            },
                        }
                    } else {
                        ChipRpcResponseEnvelope::ok(
                            request.id,
                            ChipRpcCommissionLightResponse {
                                device: commissioned_test_device(100),
                            },
                        )
                    }
                }
                ChipRpcRequest::InitController(_) => ChipRpcResponseEnvelope::ok(
                    request.id,
                    ChipInitControllerResponse {
                        fabric_id: "test".to_string(),
                        operational_fabric_id: 1,
                        compressed_fabric_id: None,
                    },
                ),
                other => panic!("unexpected RPC during recovery test: {:?}", other),
            }
        };
        let server = spawn_fake_server_multi(socket_path.clone(), 3, handler);

        let transport = ChipTransport::for_test(socket_path.clone());
        let device = transport
            .commission_light(&ble_commission_request())
            .expect("the automatic retry should honor the RPC retry_after cooldown and succeed");
        assert_eq!(device.node_id, 100);
        assert_eq!(transport.sidecar_health_for_test(), SidecarHealth::Ready);

        let requests = server.join().unwrap();
        let kinds: Vec<&'static str> = requests
            .iter()
            .map(|envelope| match envelope.request {
                ChipRpcRequest::CommissionLight(_) => "commission_light",
                ChipRpcRequest::InitController(_) => "init_controller",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            vec!["commission_light", "init_controller", "commission_light"],
            "expected failed BLE commission, controller re-init, then the automatic retry"
        );
        assert_eq!(
            commission_attempts.load(std::sync::atomic::Ordering::SeqCst),
            2
        );

        let _ = fs::remove_file(socket_path);
    }

    #[test]
    fn ble_preparation_skips_on_network_and_clears_expired_cooldown() {
        let transport = ChipTransport::for_test(temp_socket_path("ble-prep"));
        transport
            .prepare_ble_commissioning(&on_network_commission_request())
            .unwrap();
        assert_eq!(transport.sidecar_health_for_test(), SidecarHealth::Ready);

        transport.mark_ble_recovery_cooldown(Duration::from_millis(0));
        assert_eq!(
            transport.sidecar_health_for_test(),
            SidecarHealth::BleCooldown
        );
        transport.wait_for_ble_recovery_cooldown().unwrap();
        assert_eq!(transport.sidecar_health_for_test(), SidecarHealth::Ready);
    }

    #[test]
    fn recoverable_ble_commissioning_errors_match_bundle_failures() {
        for message in [
            "commissioning Matter light: src/protocols/secure_channel/PASESession.cpp:310: CHIP Error 0x00000032: Timeout",
            "commissioning Matter light: src/platform/Linux/BLEManagerImpl.cpp:887: CHIP Error 0x00000032: Timeout",
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error",
            "Disabling CHIPoBLE service due to error: src/platform/Linux/BLEManagerImpl.cpp:245: Ble Error 0x00000401: BLE adapter unavailable",
            "FAIL: Get D-Bus system bus: Could not connect: Connection refused",
            "commissioning Matter light: src/platform/Linux/bluez/BluezObjectManager.cpp:118: CHIP Error 0x000000AC: Internal error",
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:493: Operation was cancelled",
        ] {
            assert!(
                is_recoverable_ble_commissioning_error(&anyhow::anyhow!(message)),
                "expected bundle error to trigger BLE recovery: {message}"
            );
        }

        assert!(!is_recoverable_ble_commissioning_error(&anyhow::anyhow!(
            "Controller not initialized"
        )));
    }

    #[test]
    fn sidecar_log_path_from_env_ignores_empty_values() {
        assert_eq!(sidecar_log_path_from_env(None), None);
        assert_eq!(sidecar_log_path_from_env(Some(OsString::from(""))), None);
        assert_eq!(
            sidecar_log_path_from_env(Some(OsString::from("/tmp/rhythm-matter.log"))),
            Some(PathBuf::from("/tmp/rhythm-matter.log"))
        );
    }

    #[test]
    fn command_resolution_stdio_and_status_helpers_cover_process_branches() {
        let previous = std::env::var_os("RHYTHM_MATTER_CHIPD");
        std::env::set_var("RHYTHM_MATTER_CHIPD", "/tmp/rhythm-chipd-test");
        assert_eq!(
            resolve_chipd_command().unwrap(),
            PathBuf::from("/tmp/rhythm-chipd-test")
        );
        match previous {
            Some(previous) => std::env::set_var("RHYTHM_MATTER_CHIPD", previous),
            None => std::env::remove_var("RHYTHM_MATTER_CHIPD"),
        }
        assert!(!resolve_chipd_command().unwrap().as_os_str().is_empty());

        let (_stdout, _stderr) = sidecar_stdio(None).unwrap();
        let root = test_temp_root().join(format!(
            "rhythm-matter-stdio-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log_path = root.join("nested").join("chipd.log");
        let (_stdout, _stderr) = sidecar_stdio(Some(&log_path)).unwrap();
        assert!(log_path.exists());

        let transport = ChipTransport::for_test(temp_socket_path("status-no-child"));
        assert_eq!(transport.chipd_status_hint(), "not spawned");

        let running_transport = ChipTransport::for_test(temp_socket_path("status-running"));
        let running = std::process::Command::new("sleep")
            .arg("1")
            .spawn()
            .unwrap();
        *running_transport.sidecar.lock().unwrap() = Some(running);
        assert_eq!(running_transport.chipd_status_hint(), "running");

        let exited_transport = ChipTransport::for_test(temp_socket_path("status-exited"));
        let exited = std::process::Command::new("sh")
            .arg("-c")
            .arg("exit 7")
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        *exited_transport.sidecar.lock().unwrap() = Some(exited);
        assert_eq!(exited_transport.chipd_status_hint(), "exited with code 7");

        let drop_transport = ChipTransport::for_test(temp_socket_path("drop-kills-child"));
        let child = std::process::Command::new("sleep")
            .arg("2")
            .spawn()
            .unwrap();
        *drop_transport.sidecar.lock().unwrap() = Some(child);
        drop(drop_transport);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn chip_storage_artifact_paths_include_chip_ini_storage() {
        let chip_dir = PathBuf::from("/data/matter/chip");
        let requested = chip_dir.join("controller-storage.json");

        let paths = chip_storage_artifact_paths(&chip_dir, &requested);

        assert_eq!(
            paths,
            vec![
                chip_dir.join("chip_tool_config.controller-storage.ini"),
                requested,
                chip_dir.join("chip_tool_config.ini"),
            ]
        );
    }

    #[test]
    fn open_sidecar_log_file_creates_parent_dirs_and_appends() {
        let root = test_temp_root().join(format!(
            "rhythm-matter-log-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let log_path = root.join("logs").join("chipd.log");

        {
            let mut first = open_sidecar_log_file(&log_path).unwrap();
            writeln!(&mut first, "[DMG] first").unwrap();
        }
        {
            let mut second = open_sidecar_log_file(&log_path).unwrap();
            writeln!(&mut second, "[DMG] second").unwrap();
        }

        let contents = fs::read_to_string(&log_path).unwrap();
        assert!(contents.contains("[DMG] first"));
        assert!(contents.contains("[DMG] second"));

        let _ = fs::remove_dir_all(root);
    }
}
