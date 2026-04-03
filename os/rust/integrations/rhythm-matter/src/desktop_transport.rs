//! Desktop Matter transport using the `matc` library.
//!
//! Wraps `matc::devman::DeviceManager` to implement `MatterTransport`.
//! All matc I/O runs on a persistent background thread ("matter-io") with
//! its own `current_thread` tokio runtime. Commands are sent via channel;
//! results return via oneshot-style sync channels. The DM stays loaded for
//! the lifetime of the transport — no per-command thread spawn, runtime
//! creation, or disk reload.
//!
//! ## Abstraction boundary
//!
//! All `matc` types are contained within this file. The rest of rhythm-matter
//! only sees `MatterTransport`. When `rs-matter` gains controller support,
//! replace this file — nothing else changes.

use std::net::IpAddr;
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, info};
use matc::tlv::TlvItemValue;

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterDeviceInfo, MatterTransport, SubscribeSpec,
};

// ============================================================================
// IO thread protocol
// ============================================================================

/// How to initialize the DM on the IO thread.
enum InitMode {
    Load,
    Create,
    LoadOrCreate,
}

/// Requests sent from `MatcTransport` methods to the IO thread.
enum IoRequest {
    SendClusterCmd {
        node_id: u64,
        endpoint: u16,
        cluster: u32,
        cmd_id: u32,
        payload: Vec<u8>,
        reply: SyncSender<Result<()>>,
    },
    ReadAttribute {
        node_id: u64,
        endpoint: u16,
        cluster: u32,
        attr_id: u32,
        reply: SyncSender<Result<Vec<u8>>>,
    },
    Subscribe {
        node_id: u64,
        specs: Vec<SubscribeSpec>,
        reply: SyncSender<Result<()>>,
    },
    Ping {
        node_id: u64,
        reply: SyncSender<Result<bool>>,
    },
    ListDevices {
        reply: SyncSender<Result<Vec<MatterDeviceInfo>>>,
    },
    Commission {
        addr: String,
        passcode: u32,
        node_id: u64,
        name: String,
        reply: SyncSender<Result<CommissionedDevice>>,
    },
    Decommission {
        node_id: u64,
        force: bool,
        reply: SyncSender<Result<bool>>,
    },
    Shutdown,
}

// ============================================================================
// MatcTransport
// ============================================================================

/// Desktop Matter transport backed by the `matc` crate.
///
/// Manages a Matter fabric via a persistent background I/O thread that owns
/// the `matc::devman::DeviceManager` and a `current_thread` tokio runtime.
/// All matc operations are dispatched via channel, eliminating per-command
/// thread spawn, runtime creation, and DM reload overhead.
pub struct MatcTransport {
    io_tx: Sender<IoRequest>,
    io_handle: Option<std::thread::JoinHandle<()>>,
}

impl MatcTransport {
    /// Create a new Matter fabric at the given path.
    pub fn create(data_path: &str) -> Result<Self> {
        Self::start(data_path, InitMode::Create)
    }

    /// Load an existing Matter fabric from disk.
    pub fn load(data_path: &str) -> Result<Self> {
        Self::start(data_path, InitMode::Load)
    }

    /// Load existing fabric or create a new one if it doesn't exist.
    pub fn load_or_create(data_path: &str) -> Result<Self> {
        Self::start(data_path, InitMode::LoadOrCreate)
    }

    /// Spawn the IO thread with the given init mode.
    fn start(data_path: &str, init_mode: InitMode) -> Result<Self> {
        let (io_tx, io_rx) = mpsc::channel();
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<()>>(1);
        let data_path_owned = data_path.to_string();

        let handle = std::thread::Builder::new()
            .name("matter-io".to_string())
            .spawn(move || {
                io_thread_main(&data_path_owned, init_mode, io_rx, init_tx);
            })
            .context("Failed to spawn Matter I/O thread")?;

        // Wait for DM initialization result
        match init_rx.recv() {
            Ok(Ok(())) => {}
            Ok(Err(e)) => {
                let _ = handle.join();
                return Err(e);
            }
            Err(_) => {
                let _ = handle.join();
                return Err(anyhow::anyhow!("Matter I/O thread died during init"));
            }
        }

        Ok(Self {
            io_tx,
            io_handle: Some(handle),
        })
    }

    /// Send a request to the IO thread and wait for the response.
    fn request<T>(&self, build: impl FnOnce(SyncSender<Result<T>>) -> IoRequest) -> Result<T> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.io_tx
            .send(build(reply_tx))
            .map_err(|_| anyhow::anyhow!("Matter I/O thread not running"))?;
        reply_rx
            .recv()
            .map_err(|_| anyhow::anyhow!("Matter I/O thread crashed"))?
    }

    /// Commission a Matter device using the shared fabric.
    ///
    /// Phase 1 (caller thread): parse setup code + mDNS discovery.
    /// Phase 2 (IO thread): PASE → CASE commissioning + read device info.
    pub fn commission_device(
        &self,
        setup_code: &str,
        starting_node_id: u64,
    ) -> Result<CommissionedDevice> {
        // Phase 1: parse + discover on caller thread (no DM needed)
        let onboarding = matc::onboarding::decode_manual_pairing_code(setup_code)
            .with_context(|| format!("Invalid Matter setup code: {}", setup_code))?;

        info!(target: "sys", "Matter: commissioning with passcode={} discriminator={}",
            onboarding.passcode, onboarding.discriminator);

        let (addr, device_name) = discover_matter_device(onboarding.discriminator)?;

        info!(target: "sys", "Matter: discovered '{}' at {}", device_name, addr);

        let name = if device_name.is_empty() {
            format!("matter-{}", starting_node_id)
        } else {
            device_name
        };

        // Phase 2: commission on the IO thread
        self.request(|reply| IoRequest::Commission {
            addr,
            passcode: onboarding.passcode,
            node_id: starting_node_id,
            name,
            reply,
        })
    }

    /// Decommission a Matter device from the local fabric.
    ///
    /// When `force` is false, attempts to send `RemoveFabric` over the air so
    /// the device forgets our fabric and can be re-commissioned elsewhere.
    /// On failure (device unreachable, etc.) the OTA step is skipped and only
    /// the local registry is cleaned up.
    ///
    /// When `force` is true, skips the OTA step entirely.
    ///
    /// Returns `Ok(true)` if OTA removal succeeded, `Ok(false)` if only local.
    pub fn decommission_device(&self, node_id: u64, force: bool) -> Result<bool> {
        self.request(|reply| IoRequest::Decommission {
            node_id,
            force,
            reply,
        })
    }
}

impl Drop for MatcTransport {
    fn drop(&mut self) {
        let _ = self.io_tx.send(IoRequest::Shutdown);
        if let Some(handle) = self.io_handle.take() {
            let _ = handle.join();
        }
    }
}

impl MatterTransport for MatcTransport {
    fn commission(&self, _setup_code: &str) -> Result<CommissionedDevice> {
        // Commissioning must go through commission_device() which handles
        // mDNS discovery on the caller thread before dispatching to the IO thread.
        Err(anyhow::anyhow!(
            "Use MatcTransport::commission_device() instead"
        ))
    }

    fn send_cluster_cmd(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        cmd_id: u8,
        payload: &[u8],
    ) -> Result<()> {
        let payload = payload.to_vec();
        self.request(|reply| IoRequest::SendClusterCmd {
            node_id,
            endpoint,
            cluster: cluster as u32,
            cmd_id: cmd_id as u32,
            payload,
            reply,
        })
    }

    fn read_attribute(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        attr_id: u16,
    ) -> Result<Vec<u8>> {
        self.request(|reply| IoRequest::ReadAttribute {
            node_id,
            endpoint,
            cluster: cluster as u32,
            attr_id: attr_id as u32,
            reply,
        })
    }

    fn subscribe(&self, node_id: u64, specs: &[SubscribeSpec]) -> Result<()> {
        let specs = specs.to_vec();
        self.request(|reply| IoRequest::Subscribe {
            node_id,
            specs,
            reply,
        })
    }

    fn commissioned_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        self.request(|reply| IoRequest::ListDevices { reply })
    }

    fn ping(&self, node_id: u64) -> Result<bool> {
        self.request(|reply| IoRequest::Ping { node_id, reply })
    }
}

// ============================================================================
// IO thread
// ============================================================================

/// Main loop for the persistent Matter I/O thread.
///
/// Owns the `DeviceManager` and a `current_thread` tokio runtime for the
/// lifetime of the transport. All matc async operations are executed via
/// `rt.block_on()` which drives the reactor for each request.
fn io_thread_main(
    data_path: &str,
    init_mode: InitMode,
    cmd_rx: Receiver<IoRequest>,
    init_tx: SyncSender<Result<()>>,
) {
    let mut rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            let _ = init_tx.send(Err(
                anyhow::anyhow!(e).context("Failed to create Matter I/O runtime")
            ));
            return;
        }
    };

    let mut dm = match init_dm(&rt, data_path, init_mode) {
        Ok(dm) => dm,
        Err(e) => {
            let _ = init_tx.send(Err(e));
            return;
        }
    };

    let device_count = dm.list_devices().map(|d| d.len()).unwrap_or(0);
    info!(target: "sys", "Matter I/O thread started ({} devices)", device_count);
    let _ = init_tx.send(Ok(()));

    loop {
        // Use a timeout so the runtime periodically drives I/O even when
        // idle. Without this, incoming UDP packets (keepalives, retransmits)
        // accumulate in the socket's receive buffer and eventually cause
        // ENOBUFS when dm.connect() tries to send.
        let request = match cmd_rx.recv_timeout(Duration::from_secs(5)) {
            Ok(req) => req,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Drive the runtime briefly to drain pending I/O.
                // sleep() (not yield_now()) gives the I/O driver time to
                // process incoming UDP packets — yield_now returns immediately
                // when no other task is ready on a current_thread runtime.
                rt.block_on(async { tokio::time::sleep(Duration::from_millis(50)).await });
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        match request {
            IoRequest::SendClusterCmd {
                node_id,
                endpoint,
                cluster,
                cmd_id,
                payload,
                reply,
            } => {
                let result =
                    handle_send_cmd(&dm, &rt, node_id, endpoint, cluster, cmd_id, &payload);
                let _ = reply.send(result);
            }
            IoRequest::ReadAttribute {
                node_id,
                endpoint,
                cluster,
                attr_id,
                reply,
            } => {
                let result = handle_read_attribute(&dm, &rt, node_id, endpoint, cluster, attr_id);
                let _ = reply.send(result);
            }
            IoRequest::Subscribe {
                node_id,
                specs,
                reply,
            } => {
                let result = handle_subscribe(&dm, &rt, node_id, &specs);
                let _ = reply.send(result);
            }
            IoRequest::Ping { node_id, reply } => {
                let result = match handle_read_attribute(&dm, &rt, node_id, 1, 0x0006, 0x0000) {
                    Ok(_) => Ok(true),
                    Err(_) => Ok(false),
                };
                let _ = reply.send(result);
            }
            IoRequest::ListDevices { reply } => {
                let result = dm
                    .list_devices()
                    .map(|devices| {
                        devices
                            .into_iter()
                            .map(|d| MatterDeviceInfo {
                                node_id: d.node_id,
                                vendor_name: String::new(),
                                product_name: d.name,
                                reachable: true,
                            })
                            .collect()
                    })
                    .map_err(|e| anyhow::anyhow!("{}", e));
                let _ = reply.send(result);
            }
            IoRequest::Commission {
                addr,
                passcode,
                node_id,
                name,
                reply,
            } => {
                let result = handle_commission(&dm, &rt, &addr, passcode, node_id, &name);
                // After commission/decommission, recreate the runtime and reload
                // the DM. The old runtime's I/O driver may hold socket registrations
                // that prevent the new DM from binding port 5555. Dropping the
                // runtime first ensures full cleanup.
                if result.is_ok() {
                    if let Some(new) = reload_runtime_and_dm(dm, rt, data_path) {
                        dm = new.0;
                        rt = new.1;
                        // Warm-up: connect to the newly paired node to prime
                        // the CASE session cache and drain early UDP packets.
                        if let Err(e) = rt.block_on(dm.connect(node_id)) {
                            log::warn!(target: "sys", "Matter: post-commission warm-up connect failed (non-fatal): {}", e);
                        }
                    } else {
                        let _ = reply.send(result);
                        break;
                    }
                }
                let _ = reply.send(result);
            }
            IoRequest::Decommission {
                node_id,
                force,
                reply,
            } => {
                let result = handle_decommission(&dm, &rt, node_id, force);
                if result.is_ok() {
                    if let Some(new) = reload_runtime_and_dm(dm, rt, data_path) {
                        dm = new.0;
                        rt = new.1;
                    } else {
                        let _ = reply.send(result);
                        break;
                    }
                }
                let _ = reply.send(result);
            }
            IoRequest::Shutdown => {
                info!(target: "sys", "Matter I/O thread shutting down");
                break;
            }
        }
    }
}

/// Drop the DM and runtime, then recreate both from scratch.
///
/// Dropping the runtime ensures the I/O driver fully releases the UDP socket
/// (port 5555) before the new DM tries to bind it. This is needed after
/// commission/decommission because matc's async socket cleanup requires the
/// runtime to process it.
fn reload_runtime_and_dm(
    dm: matc::devman::DeviceManager,
    rt: tokio::runtime::Runtime,
    data_path: &str,
) -> Option<(matc::devman::DeviceManager, tokio::runtime::Runtime)> {
    drop(dm);
    drop(rt);

    let new_rt = match tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log::error!(target: "sys", "Matter: failed to recreate runtime after commission: {}", e);
            return None;
        }
    };

    match new_rt.block_on(matc::devman::DeviceManager::load(data_path)) {
        Ok(dm) => {
            let count = dm.list_devices().map(|d| d.len()).unwrap_or(0);
            info!(target: "sys", "Matter: reloaded fabric after commission ({} devices)", count);
            Some((dm, new_rt))
        }
        Err(e) => {
            log::error!(target: "sys", "Matter: failed to reload fabric after commission: {}", e);
            None
        }
    }
}

/// Initialize the DeviceManager based on the requested mode.
fn init_dm(
    rt: &tokio::runtime::Runtime,
    data_path: &str,
    mode: InitMode,
) -> Result<matc::devman::DeviceManager> {
    let config = matc::devman::ManagerConfig {
        fabric_id: 1,
        controller_id: 1,
        local_address: "0.0.0.0:5555".to_string(),
    };

    match mode {
        InitMode::Load => rt
            .block_on(matc::devman::DeviceManager::load(data_path))
            .context("Failed to load Matter fabric"),
        InitMode::Create => {
            let dm = rt
                .block_on(matc::devman::DeviceManager::create(data_path, config))
                .context("Failed to create Matter fabric")?;
            info!(target: "sys", "Matter: created new fabric at {}", data_path);
            Ok(dm)
        }
        InitMode::LoadOrCreate => rt
            .block_on(matc::devman::DeviceManager::load(data_path))
            .or_else(|_| {
                let dm = rt
                    .block_on(matc::devman::DeviceManager::create(data_path, config))
                    .context("Failed to create Matter fabric")?;
                info!(target: "sys", "Matter: created new fabric at {}", data_path);
                Ok(dm)
            }),
    }
}

// ============================================================================
// IO thread request handlers
// ============================================================================

/// Connect to a Matter node, retrying once on ENOBUFS after draining I/O.
///
/// ENOBUFS (os error 55) is a transient local error caused by accumulated
/// unread UDP packets in the socket's receive buffer. A brief sleep lets the
/// I/O driver drain the buffer before retrying.
fn connect_with_drain(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    node_id: u64,
) -> Result<matc::controller::Connection> {
    match rt.block_on(dm.connect(node_id)) {
        Ok(c) => Ok(c),
        Err(e) if format!("{:#}", e).contains("os error 55") => {
            debug!(target: "cmd", "Matter: ENOBUFS on connect to node {}, draining and retrying", node_id);
            rt.block_on(async { tokio::time::sleep(Duration::from_millis(100)).await });
            rt.block_on(dm.connect(node_id))
                .with_context(|| format!("Failed to connect to Matter node {}", node_id))
        }
        Err(e) => Err(anyhow::anyhow!(
            "Failed to connect to Matter node {}: {:#}",
            node_id,
            e
        )),
    }
}

/// Send a cluster command with one retry on invoke failure.
///
/// Only retries when the invoke fails (stale CASE session) — if the connect
/// itself fails, the device is unreachable and retrying just doubles the timeout.
fn handle_send_cmd(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    node_id: u64,
    endpoint: u16,
    cluster: u32,
    cmd_id: u32,
    payload: &[u8],
) -> Result<()> {
    let conn = connect_with_drain(dm, rt, node_id)?;

    match rt.block_on(conn.invoke_request(endpoint, cluster, cmd_id, payload)) {
        Ok(_) => return Ok(()),
        Err(e) => {
            debug!(target: "cmd", "Matter: invoke failed (retrying with fresh connection): {:#}", e);
        }
    }

    // Retry with fresh connection — the invoke failed, not the connect,
    // so a stale session is likely. Worth one reconnect attempt.
    let conn = connect_with_drain(dm, rt, node_id)
        .with_context(|| format!("Failed to reconnect to Matter node {}", node_id))?;
    rt.block_on(conn.invoke_request(endpoint, cluster, cmd_id, payload))
        .with_context(|| {
            format!(
                "Matter command failed: node={} cluster=0x{:04x} cmd=0x{:02x}",
                node_id, cluster, cmd_id
            )
        })?;
    Ok(())
}

/// Read an attribute and convert the TLV value to bytes.
fn handle_read_attribute(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    node_id: u64,
    endpoint: u16,
    cluster: u32,
    attr_id: u32,
) -> Result<Vec<u8>> {
    let conn = connect_with_drain(dm, rt, node_id)?;
    let val = rt
        .block_on(conn.read_request2(endpoint, cluster, attr_id))
        .with_context(|| {
            format!(
                "Matter read failed: node={} cluster=0x{:04x} attr=0x{:04x}",
                node_id, cluster, attr_id
            )
        })?;
    match val {
        TlvItemValue::Int(v) => Ok(v.to_le_bytes().to_vec()),
        TlvItemValue::Bool(b) => Ok(vec![if b { 1 } else { 0 }]),
        TlvItemValue::OctetString(data) => Ok(data),
        TlvItemValue::String(s) => Ok(s.into_bytes()),
        _ => Ok(Vec::new()),
    }
}

/// Subscribe to attribute reports.
fn handle_subscribe(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    node_id: u64,
    specs: &[SubscribeSpec],
) -> Result<()> {
    let conn = connect_with_drain(dm, rt, node_id)?;
    for spec in specs {
        rt.block_on(conn.im_subscribe_request(
            spec.endpoint,
            spec.cluster as u32,
            spec.attr_id as u32,
        ))
        .with_context(|| {
            format!(
                "Matter subscribe failed: node={} cluster=0x{:04x}",
                node_id, spec.cluster
            )
        })?;
    }
    Ok(())
}

/// Commission a device on the IO thread's runtime.
///
/// If the node ID already exists in the fabric, skips commissioning and
/// connects to the existing device to read its info (idempotent re-pair).
fn handle_commission(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    addr: &str,
    passcode: u32,
    node_id: u64,
    name: &str,
) -> Result<CommissionedDevice> {
    // Check if this node is already commissioned
    let already_exists = dm
        .list_devices()
        .unwrap_or_default()
        .iter()
        .any(|d| d.node_id == node_id);

    let conn = if already_exists {
        info!(target: "sys", "Matter: node {} already commissioned, reconnecting", node_id);
        rt.block_on(dm.connect(node_id))
            .with_context(|| format!("Failed to connect to existing Matter node {}", node_id))?
    } else {
        let conn = rt
            .block_on(dm.commission(addr, passcode, node_id, name))
            .context("Matter commissioning failed")?;

        info!(target: "sys", "Matter: commissioned node {} ({})", node_id, name);

        // Set fabric label so HomePod/other controllers show "Rhythm"
        {
            let mut label_tlv = matc::tlv::TlvBuffer::new();
            let _ = label_tlv.write_string(0, "Rhythm");
            let label_payload = label_tlv.data;
            match rt.block_on(conn.invoke_request(0, 0x003E, 0x09, &label_payload)) {
                Ok(_) => info!(target: "sys", "Matter: set fabric label to 'Rhythm'"),
                Err(e) => debug!(target: "sys", "Matter: failed to set fabric label: {}", e),
            }
        }

        conn
    };

    // Read device info (works for both fresh and existing commissions)
    let (vendor_name, product_name, vendor_id, product_id, serial) = read_basic_info(rt, &conn);
    let (color_modes, min_kelvin, max_kelvin) = probe_color_capabilities_rt(rt, &conn, 1);

    info!(target: "sys",
        "Matter: device info: vendor='{}' product='{}' vendor_id={} product_id={} serial={:?} \
         color_modes={:?} kelvin={}–{}",
        vendor_name, product_name, vendor_id, product_id, serial,
        color_modes, min_kelvin.unwrap_or(0), max_kelvin.unwrap_or(0));

    Ok(CommissionedDevice {
        node_id,
        vendor_name,
        product_name,
        vendor_id,
        product_id,
        serial_number: serial,
        light_endpoint: 1,
        color_modes,
        min_kelvin,
        max_kelvin,
    })
}

/// Decommission a device: OTA RemoveFabric (best-effort) + local registry removal.
fn handle_decommission(
    dm: &matc::devman::DeviceManager,
    rt: &tokio::runtime::Runtime,
    node_id: u64,
    force: bool,
) -> Result<bool> {
    let mut ota_ok = false;

    if !force {
        // Best-effort OTA: connect and send RemoveFabric so the device forgets us
        match (|| -> Result<()> {
            let conn = rt.block_on(dm.connect(node_id)).with_context(|| {
                format!("Failed to connect to node {} for decommission", node_id)
            })?;

            // Read CurrentFabricIndex (Operational Credentials cluster 0x003E, attr 0x0005, endpoint 0)
            let fabric_index = rt
                .block_on(conn.read_request2(0, 0x003E, 0x0005))
                .ok()
                .and_then(|v| match v {
                    TlvItemValue::Int(i) => Some(i as u8),
                    _ => None,
                })
                .unwrap_or(1);

            info!(target: "sys", "Matter: decommission node {} fabric_index={}", node_id, fabric_index);

            // Send RemoveFabric command (cluster 0x003E, cmd 0x0A)
            // Payload: TLV struct with field 0 = FabricIndex (uint8)
            let mut payload = matc::tlv::TlvBuffer::new();
            let _ = payload.write_uint8(0, fabric_index);

            rt.block_on(conn.invoke_request(0, 0x003E, 0x0A, &payload.data))
                .with_context(|| format!("RemoveFabric failed for node {}", node_id))?;

            info!(target: "sys", "Matter: sent RemoveFabric to node {}", node_id);
            Ok(())
        })() {
            Ok(()) => ota_ok = true,
            Err(e) => {
                log::warn!(target: "sys",
                    "Matter: OTA decommission failed for node {} (continuing with local removal): {}",
                    node_id, e
                );
            }
        }
    }

    // Always remove from local fabric registry
    dm.remove_device(node_id)
        .with_context(|| format!("Failed to remove node {} from local registry", node_id))?;
    info!(target: "sys", "Matter: removed node {} from local fabric", node_id);

    Ok(ota_ok)
}

// ============================================================================
// Helpers
// ============================================================================

/// Discover a commissionable Matter device via mdns-sd.
///
/// Browses `_matterc._udp.local.` for up to 15 seconds, matching by the
/// discriminator from the setup code. Returns `(ip:port, device_name)`.
/// Uses the `mdns-sd` crate (same one used for Hue bridge discovery and
/// Rhythm service advertisement) instead of matc's built-in mDNS which
/// fails in Docker containers and on some macOS configurations.
fn discover_matter_device(discriminator: u16) -> Result<(String, String)> {
    let daemon = mdns_sd::ServiceDaemon::new().context("Failed to start mDNS daemon")?;

    let service_type = "_matterc._udp.local.";
    let receiver = daemon
        .browse(service_type)
        .context("Failed to browse for Matter devices")?;

    // Manual pairing codes only encode a 4-bit short discriminator (upper 4 bits
    // of the 12-bit long discriminator). matc returns this as short_disc << 8.
    // The mDNS "D" TXT record advertises the full 12-bit long discriminator.
    // Match by comparing the upper 4 bits only (short discriminator matching).
    let short_disc = discriminator >> 8;
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    info!(target: "sys", "Matter: scanning for commissionable device (discriminator={}, short={})...",
        discriminator, short_disc);

    let mut found = None;
    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let txt_d = info
                    .get_properties()
                    .get("D")
                    .map(|v| v.val_str().to_string());

                info!(target: "sys", "Matter mDNS: resolved '{}' discriminator={:?} addrs={:?}",
                    info.get_fullname(), txt_d, info.get_addresses());

                let disc_matches = txt_d
                    .as_deref()
                    .and_then(|d| d.parse::<u16>().ok())
                    .map(|device_disc| (device_disc >> 8) == short_disc)
                    .unwrap_or(false);

                if disc_matches {
                    // Match — extract IPv4 address
                    let ip = info
                        .get_addresses()
                        .iter()
                        .find(|a| matches!(a, IpAddr::V4(_)))
                        .or_else(|| info.get_addresses().iter().next())
                        .ok_or_else(|| anyhow::anyhow!("Matter device has no IP address"))?
                        .to_string();

                    let port = info.get_port();
                    let name = info
                        .get_properties()
                        .get("DN")
                        .map(|v| v.val_str().to_string())
                        .unwrap_or_default();

                    found = Some((format!("{}:{}", ip, port), name));
                    break;
                }
            }
            Ok(event) => {
                info!(target: "sys", "Matter mDNS event: {:?}", event);
            }
            Err(_) => continue,
        }
    }

    let _ = daemon.stop_browse(service_type);
    let _ = daemon.shutdown();

    match found {
        Some(result) => {
            info!(target: "sys", "Matter: discovered device at {}", result.0);
            Ok(result)
        }
        None => Err(anyhow::anyhow!(
            "No commissionable Matter device found with discriminator {} (scanned 15s)",
            discriminator
        )),
    }
}

/// Read Basic Information cluster (endpoint 0, cluster 0x0028).
fn read_basic_info(
    rt: &tokio::runtime::Runtime,
    conn: &matc::controller::Connection,
) -> (String, String, u16, u16, Option<String>) {
    let vendor_name = rt
        .block_on(conn.read_request2(0, 0x0028, 1))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::String(s) => Some(s),
            _ => None,
        })
        .unwrap_or_default();
    let product_name = rt
        .block_on(conn.read_request2(0, 0x0028, 2))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::String(s) => Some(s),
            _ => None,
        })
        .unwrap_or_default();
    let vendor_id = rt
        .block_on(conn.read_request2(0, 0x0028, 4))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        })
        .unwrap_or(0);
    let product_id = rt
        .block_on(conn.read_request2(0, 0x0028, 5))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        })
        .unwrap_or(0);
    let serial = rt
        .block_on(conn.read_request2(0, 0x0028, 15))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::String(s) => Some(s),
            _ => None,
        });
    (vendor_name, product_name, vendor_id, product_id, serial)
}

/// Probe Color Control cluster using Runtime::block_on.
///
/// Must use `rt.block_on()` instead of `handle.block_on()` because a current_thread
/// runtime only drives I/O from `Runtime::block_on`.
fn probe_color_capabilities_rt(
    rt: &tokio::runtime::Runtime,
    conn: &matc::controller::Connection,
    endpoint: u16,
) -> (Vec<MatterColorMode>, Option<u16>, Option<u16>) {
    let cluster: u32 = 0x0300;

    // Read ColorCapabilities attribute (0x400A)
    let capabilities = rt
        .block_on(conn.read_request2(endpoint, cluster, 0x400A))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        })
        .unwrap_or(0);

    let mut modes = Vec::new();
    if capabilities & 0x01 != 0 {
        modes.push(MatterColorMode::HueSaturation);
    }
    if capabilities & 0x08 != 0 {
        modes.push(MatterColorMode::Xy);
    }
    if capabilities & 0x10 != 0 {
        modes.push(MatterColorMode::ColorTemperature);
    }

    // Fallback: read ColorMode attribute (0x0008)
    if modes.is_empty() {
        let color_mode = rt
            .block_on(conn.read_request2(endpoint, cluster, 0x0008))
            .ok()
            .and_then(|v| match v {
                TlvItemValue::Int(i) => Some(i as u8),
                _ => None,
            })
            .unwrap_or(2);
        match color_mode {
            0 => modes.push(MatterColorMode::HueSaturation),
            1 => modes.push(MatterColorMode::Xy),
            _ => modes.push(MatterColorMode::ColorTemperature),
        }
    }

    // Read CT range in mireds (min=0x400C, max=0x400D)
    let min_mireds = rt
        .block_on(conn.read_request2(endpoint, cluster, 0x400C))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        });
    let max_mireds = rt
        .block_on(conn.read_request2(endpoint, cluster, 0x400D))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        });

    // Convert mireds to kelvin (inverted)
    let min_kelvin = max_mireds
        .filter(|&m| m > 0)
        .map(|m| (1_000_000u32 / m as u32) as u16);
    let max_kelvin = min_mireds
        .filter(|&m| m > 0)
        .map(|m| (1_000_000u32 / m as u32) as u16);

    (modes, min_kelvin, max_kelvin)
}
