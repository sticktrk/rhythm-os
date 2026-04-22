//! Desktop Matter transport backed by a local native CHIP controller daemon.

use std::ffi::OsString;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use crate::chip_rpc::{
    ChipInitControllerRequest, ChipInitControllerResponse, ChipRpcCommissionLightResponse,
    ChipRpcListDevicesResponse, ChipRpcProbeLightResponse, ChipRpcReadOnOffResponse,
    ChipRpcRequest, ChipRpcRequestEnvelope, ChipRpcResponseEnvelope,
};
use crate::transport::{
    CommissionedDevice, MatterCommissionRequest, MatterDeviceInfo, MatterTransport,
};

const SOCKET_NAME: &str = "chip-controller.sock";
const STORAGE_NAME: &str = "controller-storage.json";
const CHIPD_LOGFILE_ENV: &str = "RHYTHM_MATTER_LOGFILE";
const SIDECAR_START_TIMEOUT: Duration = Duration::from_secs(10);
const RPC_TIMEOUT: Duration = Duration::from_secs(120);

struct SidecarConfig {
    command: PathBuf,
    working_dir: PathBuf,
    log_path: Option<PathBuf>,
}

/// Desktop Matter transport backed by a native CHIP daemon over a Unix socket.
pub struct ChipTransport {
    socket_path: PathBuf,
    init_request: ChipInitControllerRequest,
    sidecar: Mutex<Option<Child>>,
    sidecar_config: Option<SidecarConfig>,
    initialized: AtomicBool,
    next_request_id: AtomicU64,
}

impl ChipTransport {
    /// Load or create the local CHIP controller sidecar state.
    pub fn load_or_create(data_path: &str, fabric_id: &str) -> Result<Self> {
        let chip_dir = Path::new(data_path).join("chip");
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
                storage_path: chip_dir.join(STORAGE_NAME).display().to_string(),
                ble_controller,
            },
            sidecar: Mutex::new(None),
            sidecar_config: Some(SidecarConfig {
                command,
                working_dir: chip_dir.clone(),
                log_path: sidecar_log_path_from_env(std::env::var_os(CHIPD_LOGFILE_ENV)),
            }),
            initialized: AtomicBool::new(false),
            next_request_id: AtomicU64::new(1),
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
                storage_path: "/tmp/test-controller-storage.json".to_string(),
                ble_controller: None,
            },
            sidecar: Mutex::new(None),
            sidecar_config: None,
            initialized: AtomicBool::new(true),
            next_request_id: AtomicU64::new(1),
        }
    }

    fn ensure_sidecar(&self) -> Result<()> {
        if self.initialized.load(Ordering::SeqCst) && self.socket_path.exists() {
            return Ok(());
        }

        if self.can_connect().is_err() {
            self.start_sidecar()?;
        }

        let response: ChipInitControllerResponse = self.decode_rpc_response(
            self.send_rpc_envelope(ChipRpcRequest::InitController(self.init_request.clone()))?,
        )?;
        if response.fabric_id != self.init_request.fabric_id {
            anyhow::bail!(
                "CHIP sidecar initialized unexpected fabric '{}'",
                response.fabric_id
            );
        }

        self.initialized.store(true, Ordering::SeqCst);
        Ok(())
    }

    fn call<T: serde::de::DeserializeOwned>(&self, request: ChipRpcRequest) -> Result<T> {
        self.ensure_sidecar()?;
        match self.send_rpc_envelope(request.clone()) {
            Ok(response) => self.decode_rpc_response(response),
            Err(first_error) => {
                self.initialized.store(false, Ordering::SeqCst);
                if self.sidecar_config.is_some() {
                    self.start_sidecar()?;
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

        let mut stream = UnixStream::connect(&self.socket_path)
            .with_context(|| format!("connecting to {}", self.socket_path.display()))?;
        stream
            .set_read_timeout(Some(RPC_TIMEOUT))
            .context("setting CHIP RPC read timeout")?;
        stream
            .set_write_timeout(Some(RPC_TIMEOUT))
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

    fn start_sidecar(&self) -> Result<()> {
        let Some(config) = &self.sidecar_config else {
            return Ok(());
        };

        if self.socket_path.exists() && self.can_connect().is_err() {
            let _ = fs::remove_file(&self.socket_path);
        }

        if let Ok(mut guard) = self.sidecar.lock() {
            if let Some(mut child) = guard.take() {
                let _ = child.kill();
                let _ = child.wait();
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
                })?;

            *guard = Some(child);
        }

        let deadline = Instant::now() + SIDECAR_START_TIMEOUT;
        while Instant::now() < deadline {
            if self.can_connect().is_ok() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(100));
        }

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
        let response: ChipRpcCommissionLightResponse =
            self.call(ChipRpcRequest::CommissionLight(request.clone()))?;
        Ok(response.device)
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;
    use std::thread;

    use crate::chip_rpc::{ChipRpcEmpty, ChipRpcListDevicesResponse, ChipRpcResponseEnvelope};

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
    fn sidecar_log_path_from_env_ignores_empty_values() {
        assert_eq!(sidecar_log_path_from_env(None), None);
        assert_eq!(sidecar_log_path_from_env(Some(OsString::from(""))), None);
        assert_eq!(
            sidecar_log_path_from_env(Some(OsString::from("/tmp/rhythm-matter.log"))),
            Some(PathBuf::from("/tmp/rhythm-matter.log"))
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
