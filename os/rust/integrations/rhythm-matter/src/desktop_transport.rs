//! Desktop Matter transport using the `matc` library.
//!
//! Wraps `matc::devman::DeviceManager` to implement `MatterTransport`.
//! Uses `tokio::task::block_in_place` to bridge matc's async API to our
//! blocking trait interface (same pattern as reqwest's blocking mode).
//!
//! ## Abstraction boundary
//!
//! All `matc` types are contained within this file. The rest of rhythm-matter
//! only sees `MatterTransport`. When `rs-matter` gains controller support,
//! replace this file — nothing else changes.

use std::net::IpAddr;
use std::time::Duration;

use anyhow::{Context, Result};
use log::{debug, info};
use matc::tlv::TlvItemValue;

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterDeviceInfo, MatterTransport, SubscribeSpec,
};

/// Desktop Matter transport backed by the `matc` crate.
///
/// Manages a Matter fabric via `matc::devman::DeviceManager`.
/// Connections are created per-command (matc's `Connection` is not Clone).
pub struct MatcTransport {
    dm: matc::devman::DeviceManager,
}

impl MatcTransport {
    /// Create a new Matter fabric at the given path.
    pub fn create(data_path: &str) -> Result<Self> {
        let config = matc::devman::ManagerConfig {
            fabric_id: 1,
            controller_id: 1,
            local_address: "0.0.0.0:5555".to_string(),
        };

        let dm = block_on(matc::devman::DeviceManager::create(data_path, config))
            .context("Failed to create Matter fabric")?;

        info!(target: "sys", "Matter: created new fabric at {}", data_path);
        Ok(Self { dm })
    }

    /// Load an existing Matter fabric from disk.
    pub fn load(data_path: &str) -> Result<Self> {
        let dm = block_on(matc::devman::DeviceManager::load(data_path))
            .context("Failed to load Matter fabric")?;

        let device_count = dm.list_devices().map(|d| d.len()).unwrap_or(0);
        info!(target: "sys", "Matter: loaded fabric from {} ({} devices)", data_path, device_count);
        Ok(Self { dm })
    }

    /// Load existing fabric or create a new one if it doesn't exist.
    pub fn load_or_create(data_path: &str) -> Result<Self> {
        match Self::load(data_path) {
            Ok(t) => Ok(t),
            Err(_) => Self::create(data_path),
        }
    }

    /// Create a CASE connection to a device by node_id.
    fn connect(&self, node_id: u64) -> Result<matc::controller::Connection> {
        block_on(self.dm.connect(node_id))
            .with_context(|| format!("Failed to connect to Matter node {}", node_id))
    }

    /// Send a command with one retry on connection failure.
    fn send_with_retry(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u32,
        cmd_id: u32,
        payload: &[u8],
    ) -> Result<()> {
        let conn = self.connect(node_id)?;
        match block_on(conn.invoke_request(endpoint, cluster, cmd_id, payload)) {
            Ok(_) => return Ok(()),
            Err(e) => {
                debug!(target: "cmd", "Matter: command failed (retrying): {}", e);
            }
        }

        // Retry with fresh connection
        let conn = self.connect(node_id)?;
        block_on(conn.invoke_request(endpoint, cluster, cmd_id, payload))
            .with_context(|| {
                format!(
                    "Matter command failed: node={} cluster=0x{:04x} cmd=0x{:02x}",
                    node_id, cluster, cmd_id
                )
            })?;
        Ok(())
    }

    /// Generate the next available node ID.
    fn next_node_id(&self) -> u64 {
        let max = self
            .dm
            .list_devices()
            .unwrap_or_default()
            .iter()
            .map(|d| d.node_id)
            .max()
            .unwrap_or(99);
        max + 1
    }
}

impl MatterTransport for MatcTransport {
    fn commission(&self, setup_code: &str) -> Result<CommissionedDevice> {
        // Parse setup code to get passcode and discriminator
        let onboarding = matc::onboarding::decode_manual_pairing_code(setup_code)
            .with_context(|| format!("Invalid Matter setup code: {}", setup_code))?;

        info!(target: "sys", "Matter: commissioning with passcode={} discriminator={}",
            onboarding.passcode, onboarding.discriminator);

        // Discover commissionable devices via mdns-sd (replaces matc's broken mDNS)
        let (addr, device_name) = discover_matter_device(onboarding.discriminator)?;

        info!(target: "sys", "Matter: discovered '{}' at {}", device_name, addr);

        let node_id = self.next_node_id();
        let name = if device_name.is_empty() {
            format!("matter-{}", node_id)
        } else {
            device_name
        };

        // Commission: PASE → operational certificates → CASE session
        let conn = block_on(self.dm.commission(&addr, onboarding.passcode, node_id, &name))
            .context("Matter commissioning failed")?;

        info!(target: "sys", "Matter: commissioned node {} ({})", node_id, name);

        // Read Basic Information cluster (endpoint 0, cluster 0x0028) for device details
        let vendor_name = read_string_attr(&conn, 0, 0x0028, 1).unwrap_or_default();
        let product_name = read_string_attr(&conn, 0, 0x0028, 2).unwrap_or_default();
        let vendor_id = read_u16_attr(&conn, 0, 0x0028, 4).unwrap_or(0);
        let product_id = read_u16_attr(&conn, 0, 0x0028, 5).unwrap_or(0);
        let serial = read_string_attr(&conn, 0, 0x0028, 15).ok();

        // Probe Color Control cluster for capabilities
        let (color_modes, min_kelvin, max_kelvin) = probe_color_capabilities(&conn, 1);

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

    fn send_cluster_cmd(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        cmd_id: u8,
        payload: &[u8],
    ) -> Result<()> {
        self.send_with_retry(node_id, endpoint, cluster as u32, cmd_id as u32, payload)
    }

    fn read_attribute(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        attr_id: u16,
    ) -> Result<Vec<u8>> {
        let conn = self.connect(node_id)?;
        let val = block_on(conn.read_request2(endpoint, cluster as u32, attr_id as u32))
            .with_context(|| {
                format!(
                    "Matter read failed: node={} cluster=0x{:04x} attr=0x{:04x}",
                    node_id, cluster, attr_id
                )
            })?;

        // Convert TlvItemValue to bytes
        match val {
            TlvItemValue::Int(v) => Ok(v.to_le_bytes().to_vec()),
            TlvItemValue::Bool(b) => Ok(vec![if b { 1 } else { 0 }]),
            TlvItemValue::OctetString(data) => Ok(data),
            TlvItemValue::String(s) => Ok(s.into_bytes()),
            _ => Ok(Vec::new()),
        }
    }

    fn subscribe(&self, node_id: u64, specs: &[SubscribeSpec]) -> Result<()> {
        let conn = self.connect(node_id)?;
        for spec in specs {
            block_on(conn.im_subscribe_request(
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

    fn commissioned_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        let devices = self.dm.list_devices()?;
        Ok(devices
            .into_iter()
            .map(|d| MatterDeviceInfo {
                node_id: d.node_id,
                vendor_name: d.name.clone(),
                product_name: d.name,
                reachable: true,
            })
            .collect())
    }

    fn ping(&self, node_id: u64) -> Result<bool> {
        let conn = match self.connect(node_id) {
            Ok(c) => c,
            Err(_) => return Ok(false),
        };
        match block_on(conn.read_request2(1, 0x0006, 0x0000)) {
            Ok(_) => Ok(true),
            Err(_) => Ok(false),
        }
    }
}

// ============================================================================
// Helpers
// ============================================================================

/// Bridge async matc calls to blocking context.
fn block_on<F: std::future::Future>(f: F) -> F::Output {
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        tokio::task::block_in_place(|| handle.block_on(f))
    } else {
        let rt = tokio::runtime::Runtime::new().expect("Failed to create tokio runtime");
        rt.block_on(f)
    }
}

/// Read a string attribute from a device.
fn read_string_attr(
    conn: &matc::controller::Connection,
    endpoint: u16,
    cluster: u32,
    attr: u32,
) -> Result<String> {
    let val = block_on(conn.read_request2(endpoint, cluster, attr))?;
    match val {
        TlvItemValue::String(s) => Ok(s),
        _ => Err(anyhow::anyhow!("Attribute is not a string")),
    }
}

/// Read a u16 attribute from a device.
fn read_u16_attr(
    conn: &matc::controller::Connection,
    endpoint: u16,
    cluster: u32,
    attr: u32,
) -> Result<u16> {
    let val = block_on(conn.read_request2(endpoint, cluster, attr))?;
    match val {
        TlvItemValue::Int(v) => Ok(v as u16),
        _ => Err(anyhow::anyhow!("Attribute is not an integer")),
    }
}


/// Discover a commissionable Matter device via mdns-sd.
///
/// Browses `_matterc._udp.local.` for up to 15 seconds, matching by the
/// discriminator from the setup code. Returns `(ip:port, device_name)`.
/// Uses the `mdns-sd` crate (same one used for Hue bridge discovery and
/// Rhythm service advertisement) instead of matc's built-in mDNS which
/// fails in Docker containers and on some macOS configurations.
fn discover_matter_device(discriminator: u16) -> Result<(String, String)> {
    let daemon = mdns_sd::ServiceDaemon::new()
        .context("Failed to start mDNS daemon")?;

    let service_type = "_matterc._udp.local.";
    let receiver = daemon.browse(service_type)
        .context("Failed to browse for Matter devices")?;

    let disc_str = discriminator.to_string();
    let deadline = std::time::Instant::now() + Duration::from_secs(15);

    info!(target: "sys", "Matter: scanning for commissionable device (discriminator={})...", discriminator);

    let mut found = None;
    while std::time::Instant::now() < deadline {
        match receiver.recv_timeout(Duration::from_millis(200)) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(info)) => {
                let txt_d = info.get_properties()
                    .get("D")
                    .map(|v| v.val_str().to_string());

                info!(target: "sys", "Matter mDNS: resolved '{}' discriminator={:?} addrs={:?}",
                    info.get_fullname(), txt_d, info.get_addresses());

                if txt_d.as_deref() == Some(&disc_str) {
                    // Match — extract IPv4 address
                    let ip = info.get_addresses()
                        .iter()
                        .find(|a| matches!(a, IpAddr::V4(_)))
                        .or_else(|| info.get_addresses().iter().next())
                        .ok_or_else(|| anyhow::anyhow!("Matter device has no IP address"))?
                        .to_string();

                    let port = info.get_port();
                    let name = info.get_properties()
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

/// Probe Color Control cluster for supported capabilities.
fn probe_color_capabilities(
    conn: &matc::controller::Connection,
    endpoint: u16,
) -> (Vec<MatterColorMode>, Option<u16>, Option<u16>) {
    let cluster: u32 = 0x0300;

    // Read ColorCapabilities attribute (0x400A)
    let capabilities = block_on(conn.read_request2(endpoint, cluster, 0x400A))
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
        let color_mode = block_on(conn.read_request2(endpoint, cluster, 0x0008))
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
    let min_mireds = block_on(conn.read_request2(endpoint, cluster, 0x400C))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        });
    let max_mireds = block_on(conn.read_request2(endpoint, cluster, 0x400D))
        .ok()
        .and_then(|v| match v {
            TlvItemValue::Int(i) => Some(i as u16),
            _ => None,
        });

    // Convert mireds to kelvin (inverted)
    let min_kelvin = max_mireds.filter(|&m| m > 0).map(|m| (1_000_000u32 / m as u32) as u16);
    let max_kelvin = min_mireds.filter(|&m| m > 0).map(|m| (1_000_000u32 / m as u32) as u16);

    (modes, min_kelvin, max_kelvin)
}
