//! BLE GATT provisioning server for WiFi credential setup.
//!
//! Advertises a custom GATT service that accepts WiFi credentials from a BLE
//! client (e.g., a BLE client (e.g. nRF Connect)). Once credentials are received,
//! a separate WiFi thread attempts connection. The IP address is reported back
//! to the client via BLE notification before BLE is torn down.
//!
//! On ESP32-C6 the modem supports BLE+WiFi coexistence, so BLE stays alive
//! while WiFi connects. The modem is split in `main.rs`: BLE gets
//! `BluetoothModem`, WiFi gets a stolen `WifiModem` in its own thread.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use anyhow::Result;
use enumset::EnumSet;
use esp_idf_svc::bt::ble::gap::{
    AdvConfiguration, AuthenticationRequest, BleGapEvent, EspBleGap, IOCapabilities, KeyMask,
    SecurityConfiguration,
};
use esp_idf_svc::bt::ble::gatt::server::{EspGatts, GattsEvent};
use esp_idf_svc::bt::ble::gatt::{
    AutoResponse, GattCharacteristic, GattId, GattServiceId, Handle, Permission, Property,
};
use esp_idf_svc::bt::{BdAddr, Ble, BtDriver, BtUuid};
use esp_idf_svc::hal::modem::BluetoothModem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use log::{info, warn};

use crate::led::{LedStatus, Ws2812Led};

/// WiFi credentials received from BLE provisioning.
pub struct WifiCredentials {
    pub ssid: String,
    pub password: String,
}

/// Result of a WiFi connection attempt, sent from the WiFi thread back to BLE.
pub enum WifiResult {
    Connected { ip: String },
    Failed { error: String },
}

// ============================================================================
// UUIDs as u128 (big-endian, matching standard UUID representation)
// ============================================================================

// Service UUID:         72797468-6d00-1000-8000-00805f9b34fb
const SERVICE_UUID: u128 =      0x72797468_6d00_1000_8000_00805f9b34fb;
// WiFi Command char:   72797468-6d01-1000-8000-00805f9b34fb
const WIFI_CMD_UUID: u128 =     0x72797468_6d01_1000_8000_00805f9b34fb;
// Status char:          72797468-6d02-1000-8000-00805f9b34fb
const STATUS_UUID: u128 =       0x72797468_6d02_1000_8000_00805f9b34fb;
// Device Info char:     72797468-6d03-1000-8000-00805f9b34fb
const DEVICE_INFO_UUID: u128 =  0x72797468_6d03_1000_8000_00805f9b34fb;

// ============================================================================
// Internal event type — callback → main loop communication
// ============================================================================

enum BleEvent {
    /// GATT app registered, ready to create service.
    AppRegistered { gatt_if: u8 },
    /// Service created, ready to add characteristics.
    ServiceCreated { service_handle: Handle },
    /// A characteristic was added. `index`: 1-based count of chars added so far.
    CharAdded {
        handle: Handle,
        service_handle: Handle,
        index: u8,
    },
    /// A descriptor was added to a characteristic.
    DescriptorAdded { service_handle: Handle },
    /// Service started, ready to advertise.
    ServiceStarted,
    /// A BLE client connected.
    PeerConnected { conn_id: u16 },
    /// The BLE client disconnected.
    PeerDisconnected,
    /// WiFi credentials parsed from a write.
    Credentials(WifiCredentials),
    /// An error in write parsing.
    WriteError(String),
}

/// Get the base MAC address of the ESP32.
pub fn get_mac_address() -> [u8; 6] {
    let mut mac = [0u8; 6];
    unsafe {
        esp_idf_svc::sys::esp_read_mac(
            mac.as_mut_ptr(),
            esp_idf_svc::sys::esp_mac_type_t_ESP_MAC_BT,
        );
    }
    mac
}

/// Run BLE provisioning with BLE+WiFi coexistence.
///
/// Uses the split `BluetoothModem` so WiFi can connect concurrently.
/// Credentials are forwarded to a WiFi thread via `cred_tx`. The WiFi
/// thread sends its result back on `wifi_rx`. On success the IP is
/// notified to the BLE client before BLE tears down.
pub fn run_provisioning<'d>(
    modem: BluetoothModem<'d>,
    nvs: EspDefaultNvsPartition,
    led: &mut Ws2812Led,
    cred_tx: Sender<WifiCredentials>,
    wifi_rx: Receiver<WifiResult>,
) -> Result<WifiCredentials> {
    info!("Starting BLE provisioning...");

    let mac = get_mac_address();
    let device_name = format!("Rhythm-{:02X}{:02X}", mac[4], mac[5]);
    info!("BLE device name: {}", device_name);

    let device_info_json = format!(
        r#"{{"name":"{}","version":"{}","mac":"{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}"}}"#,
        device_name, crate::FIRMWARE_VERSION, mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
    );

    // Initialize BLE driver with BluetoothModem (not full Modem)
    let bt_driver = BtDriver::<Ble>::new(modem, Some(nvs.clone()))?;
    info!("BLE driver initialized");

    // Negotiate larger MTU for WiFi credential JSON (must be after BT init)
    esp_idf_svc::bt::ble::gatt::set_local_mtu(256)?;

    // Initialize GAP
    let gap = EspBleGap::new(&bt_driver)?;
    gap.set_device_name(&device_name)?;
    // Advertisement data: flags + 128-bit service UUID (~21 bytes).
    // The UUID must be in the advertisement for AccessorySetupKit (ASK)
    // discovery on iOS. Name goes in the scan response to stay under 31 bytes.
    gap.set_adv_conf(&AdvConfiguration {
        set_scan_rsp: false,
        include_name: false,
        include_txpower: false,
        flag: 0x06, // General discoverable + BR/EDR not supported
        service_uuid: Some(BtUuid::uuid128(SERVICE_UUID)),
        ..Default::default()
    })?;

    // Scan response: device name so scanners (nRF Connect)
    // can display a human-readable label. Android discovers via name prefix
    // from this scan response data.
    gap.set_adv_conf(&AdvConfiguration {
        set_scan_rsp: true,
        include_name: true,
        include_txpower: false,
        ..Default::default()
    })?;

    // "Just Works" pairing — no PIN, auto-accept. Required for ASK
    // (.bluetoothPairingLE) which initiates SMP on connect.
    gap.set_security_conf(&SecurityConfiguration {
        auth_req_mode: AuthenticationRequest::SecureBonding,
        io_capabilities: IOCapabilities::NoInputNoOutput,
        initiator_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
        responder_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
        max_key_size: Some(16),
        min_key_size: Some(7),
        ..Default::default()
    })?;

    // Shared peer address: GATTS PeerConnected stores it, GAP SecurityRequest reads it
    // to call esp_ble_gap_security_rsp (which the high-level API doesn't wrap).
    let peer_addr: Arc<Mutex<Option<BdAddr>>> = Arc::new(Mutex::new(None));
    let peer_addr_gap = peer_addr.clone();

    // GAP events — handle security + logging
    gap.subscribe(move |event| match event {
        BleGapEvent::SecurityRequest => {
            // Auto-accept the pairing request using the stored peer address
            if let Some(addr) = peer_addr_gap.lock().ok().and_then(|g| *g) {
                info!("BLE security request — accepting for {}", addr);
                unsafe {
                    esp_idf_svc::sys::esp_ble_gap_security_rsp(
                        addr.raw().as_ptr() as *mut u8,
                        true,
                    );
                }
            } else {
                warn!("BLE security request but no peer address stored");
            }
        }
        BleGapEvent::AuthenticationComplete { bd_addr, status } => {
            info!("BLE auth complete: addr={} status={:?}", bd_addr, status);
        }
        BleGapEvent::AdvertisingConfigured(status) => {
            info!("BLE adv configured: {:?}", status);
        }
        BleGapEvent::AdvertisingStarted(status) => {
            info!("BLE adv started: {:?}", status);
        }
        _ => {}
    })?;

    // Initialize GATT server
    let gatts = EspGatts::new(&bt_driver)?;

    // Event channel: callback → main loop
    let (evt_tx, evt_rx) = mpsc::channel::<BleEvent>();
    let mut char_count: u8 = 0;
    let peer_addr_gatts = peer_addr.clone();

    gatts.subscribe(move |(_gatt_if, event)| {
        match event {
            GattsEvent::ServiceRegistered { status, app_id } => {
                info!("GATTS registered: status={:?} app={}", status, app_id);
                let _ = evt_tx.send(BleEvent::AppRegistered { gatt_if: _gatt_if });
            }

            GattsEvent::ServiceCreated {
                status,
                service_handle,
                ..
            } => {
                info!("Service created: status={:?} handle={}", status, service_handle);
                char_count = 0;
                let _ = evt_tx.send(BleEvent::ServiceCreated { service_handle });
            }

            GattsEvent::CharacteristicAdded {
                status,
                attr_handle,
                service_handle,
                ..
            } => {
                char_count += 1;
                info!(
                    "Char added #{}: status={:?} attr={} svc={}",
                    char_count, status, attr_handle, service_handle
                );
                let _ = evt_tx.send(BleEvent::CharAdded {
                    handle: attr_handle,
                    service_handle,
                    index: char_count,
                });
            }

            GattsEvent::DescriptorAdded {
                status,
                service_handle,
                ..
            } => {
                info!("Descriptor added: status={:?} svc={}", status, service_handle);
                let _ = evt_tx.send(BleEvent::DescriptorAdded { service_handle });
            }

            GattsEvent::ServiceStarted { status, .. } => {
                info!("Service started: {:?}", status);
                let _ = evt_tx.send(BleEvent::ServiceStarted);
            }

            GattsEvent::PeerConnected { conn_id, addr, .. } => {
                info!("BLE client connected: conn_id={} addr={}", conn_id, addr);
                // Store peer address for GAP SecurityRequest handler
                if let Ok(mut guard) = peer_addr_gatts.lock() {
                    *guard = Some(addr);
                }
                let _ = evt_tx.send(BleEvent::PeerConnected { conn_id });
            }

            GattsEvent::PeerDisconnected { conn_id, .. } => {
                info!("BLE client disconnected: conn_id={}", conn_id);
                if let Ok(mut guard) = peer_addr_gatts.lock() {
                    *guard = None;
                }
                let _ = evt_tx.send(BleEvent::PeerDisconnected);
            }

            GattsEvent::Mtu { conn_id, mtu } => {
                info!("MTU negotiated: conn_id={} mtu={}", conn_id, mtu);
            }

            GattsEvent::Write { handle, value, .. } => {
                // Debug: hex-dump every write so we can discover Apple's wire format
                info!("BLE write handle={} len={} hex=[{}]",
                    handle,
                    value.len(),
                    value.iter().map(|b| format!("{:02x}", b)).collect::<Vec<_>>().join(" ")
                );
                if let Ok(s) = std::str::from_utf8(value) {
                    info!("BLE write UTF-8: {}", s);
                }

                // Skip non-JSON writes for credential parsing (but they're logged above)
                if value.is_empty() || value[0] != b'{' {
                    return;
                }

                // Parse WiFi credentials from the write value
                match std::str::from_utf8(value) {
                    Ok(json_str) => {
                        info!("BLE write on handle {}: {}", handle, json_str);
                        match serde_json::from_str::<serde_json::Value>(json_str) {
                            Ok(val) => {
                                let ssid = val.get("ssid").and_then(|v| v.as_str());
                                let password = val.get("password").and_then(|v| v.as_str());
                                match (ssid, password) {
                                    (Some(s), Some(p)) => {
                                        let _ = evt_tx.send(BleEvent::Credentials(
                                            WifiCredentials {
                                                ssid: s.to_string(),
                                                password: p.to_string(),
                                            },
                                        ));
                                    }
                                    _ => {
                                        let _ = evt_tx.send(BleEvent::WriteError(
                                            "Missing ssid or password".to_string(),
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = evt_tx.send(BleEvent::WriteError(format!(
                                    "Invalid JSON: {}",
                                    e
                                )));
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Invalid UTF-8 in BLE write: {}", e);
                    }
                }
            }

            _ => {}
        }
    })?;

    // Kick off the event-driven setup chain
    gatts.register_app(0)?;

    // State tracked in the main loop
    let mut gatt_if: Option<u8> = None;
    let mut status_handle: Option<Handle> = None;
    let mut conn_id: Option<u16> = None;

    // Process setup events until service is running
    let setup_timeout = Duration::from_secs(10);
    let mut service_running = false;

    let setup_start = std::time::Instant::now();
    while !service_running && setup_start.elapsed() < setup_timeout {
        match evt_rx.recv_timeout(Duration::from_millis(200)) {
            Ok(BleEvent::AppRegistered { gatt_if: gif }) => {
                gatt_if = Some(gif);

                let service_id = GattServiceId {
                    id: GattId {
                        uuid: BtUuid::uuid128(SERVICE_UUID),
                        inst_id: 0,
                    },
                    is_primary: true,
                };
                // 3 chars × 2 handles + 1 CCCD descriptor + 1 service = 10, +2 spare
                gatts.create_service(gif, &service_id, 12)?;
            }

            Ok(BleEvent::ServiceCreated { service_handle }) => {
                // Add WiFi Command char (Write, auto-response)
                let perms: EnumSet<Permission> = Permission::Write.into();
                let props: EnumSet<Property> = Property::Write | Property::WriteNoResponse;
                let char_def = GattCharacteristic {
                    uuid: BtUuid::uuid128(WIFI_CMD_UUID),
                    permissions: perms,
                    properties: props,
                    max_len: 256,
                    auto_rsp: AutoResponse::ByGatt,
                };
                gatts.add_characteristic(service_handle, &char_def, &[])?;
            }

            Ok(BleEvent::CharAdded {
                handle,
                service_handle,
                index,
            }) => {
                match index {
                    1 => {
                        // WiFi Command added → add Status char (Read + Notify)
                        let perms: EnumSet<Permission> = Permission::Read.into();
                        let props: EnumSet<Property> = Property::Read | Property::Notify;
                        let char_def = GattCharacteristic {
                            uuid: BtUuid::uuid128(STATUS_UUID),
                            permissions: perms,
                            properties: props,
                            max_len: 256,
                            auto_rsp: AutoResponse::ByGatt,
                        };
                        let initial = br#"{"status":"waiting"}"#;
                        gatts.add_characteristic(service_handle, &char_def, initial)?;
                    }
                    2 => {
                        // Status added → save handle, add CCCD descriptor with auto-response
                        // (the high-level add_descriptor API passes NULL for auto_rsp,
                        // so the stack won't respond to CCCD writes — use raw FFI instead)
                        status_handle = Some(handle);
                        let cccd_uuid = BtUuid::uuid16(0x2902);
                        let mut cccd_val = [0u8; 2]; // notifications disabled initially
                        let value = esp_idf_svc::sys::esp_attr_value_t {
                            attr_max_len: 2,
                            attr_len: 2,
                            attr_value: cccd_val.as_mut_ptr(),
                        };
                        let auto_rsp = esp_idf_svc::sys::esp_attr_control_t {
                            auto_rsp: esp_idf_svc::sys::ESP_GATT_AUTO_RSP as _,
                        };
                        esp_idf_svc::sys::esp!(unsafe {
                            esp_idf_svc::sys::esp_ble_gatts_add_char_descr(
                                service_handle,
                                cccd_uuid.raw() as *const _ as *mut _,
                                (esp_idf_svc::sys::ESP_GATT_PERM_READ
                                    | esp_idf_svc::sys::ESP_GATT_PERM_WRITE)
                                    as u16,
                                &value as *const _ as *mut _,
                                &auto_rsp as *const _ as *mut _,
                            )
                        })?;
                    }
                    3 => {
                        // Device Info added → start service
                        gatts.start_service(service_handle)?;
                    }
                    _ => {}
                }
            }

            Ok(BleEvent::DescriptorAdded { service_handle }) => {
                // CCCD added → add Device Info char (Read)
                let perms: EnumSet<Permission> = Permission::Read.into();
                let props: EnumSet<Property> = Property::Read.into();
                let char_def = GattCharacteristic {
                    uuid: BtUuid::uuid128(DEVICE_INFO_UUID),
                    permissions: perms,
                    properties: props,
                    max_len: 256,
                    auto_rsp: AutoResponse::ByGatt,
                };
                gatts.add_characteristic(
                    service_handle,
                    &char_def,
                    device_info_json.as_bytes(),
                )?;
            }

            Ok(BleEvent::ServiceStarted) => {
                service_running = true;
            }

            Ok(_) => {
                // Ignore other events during setup
            }

            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Keep waiting
            }

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("BLE event channel disconnected during setup"));
            }
        }
    }

    if !service_running {
        return Err(anyhow::anyhow!("BLE GATT service setup timed out"));
    }

    // Start advertising
    gap.start_advertising()?;
    info!("BLE provisioning active — waiting for WiFi credentials...");

    // Main provisioning loop: show LED + wait for credentials
    loop {
        led.show(LedStatus::BleProvisioning);

        match evt_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(BleEvent::Credentials(creds)) => {
                info!("WiFi credentials received via BLE: ssid={}", creds.ssid);

                // Update status characteristic
                if let Some(sh) = status_handle {
                    let _ = gatts.set_attr(sh, br#"{"status":"connecting"}"#);

                    // Notify connected client
                    if let (Some(gif), Some(cid)) = (gatt_if, conn_id) {
                        let _ = gatts.notify(
                            gif,
                            cid,
                            sh,
                            br#"{"status":"connecting"}"#,
                        );
                    }
                }

                // Send credentials to WiFi thread
                let return_ssid = creds.ssid.clone();
                let return_pass = creds.password.clone();
                if cred_tx.send(creds).is_err() {
                    return Err(anyhow::anyhow!("WiFi thread channel disconnected"));
                }

                // Wait for WiFi result while keeping BLE alive.
                // Inline rather than a separate function to avoid spelling
                // out the complex EspGatts/EspBleGap generics.
                info!("Waiting for WiFi connection result...");
                let wifi_result: Option<WifiResult> = {
                    let deadline = std::time::Instant::now() + Duration::from_secs(30);
                    loop {
                        // Check WiFi result (non-blocking)
                        match wifi_rx.recv_timeout(Duration::from_millis(100)) {
                            Ok(result) => break Some(result),
                            Err(mpsc::RecvTimeoutError::Timeout) => {}
                            Err(mpsc::RecvTimeoutError::Disconnected) => break None,
                        }

                        // Drain BLE events to keep connection alive
                        loop {
                            match evt_rx.try_recv() {
                                Ok(BleEvent::PeerConnected { conn_id: cid }) => {
                                    conn_id = Some(cid);
                                }
                                Ok(BleEvent::PeerDisconnected) => {
                                    conn_id = None;
                                    let _ = gap.start_advertising();
                                }
                                Ok(BleEvent::WriteError(msg)) => {
                                    warn!("BLE write error during WiFi wait: {}", msg);
                                }
                                Ok(_) => {}
                                Err(mpsc::TryRecvError::Empty) => break,
                                Err(mpsc::TryRecvError::Disconnected) => break,
                            }
                        }

                        // Timeout check
                        if std::time::Instant::now() > deadline {
                            warn!("WiFi connection wait timed out (30s)");
                            break Some(WifiResult::Failed {
                                error: "Connection timed out".to_string(),
                            });
                        }

                        // Keep LED indicating we're waiting
                        led.show(LedStatus::WifiConnecting);
                    }
                };

                match wifi_result {
                    Some(WifiResult::Connected { ip }) => {
                        info!("WiFi connected with IP: {}", ip);

                        // Notify IP to BLE client
                        if let Some(sh) = status_handle {
                            let msg = format!(r#"{{"status":"connected","ip":"{}"}}"#, ip);
                            let _ = gatts.set_attr(sh, msg.as_bytes());

                            if let (Some(gif), Some(cid)) = (gatt_if, conn_id) {
                                let _ = gatts.notify(gif, cid, sh, msg.as_bytes());
                            }
                        }

                        // Give client time to read the notification
                        thread::sleep(Duration::from_secs(2));

                        // Tear down BLE stack
                        info!("Stopping BLE after successful WiFi connection...");
                        let _ = gap.stop_advertising();
                        drop(gatts);
                        drop(gap);
                        drop(bt_driver);

                        return Ok(WifiCredentials {
                            ssid: return_ssid,
                            password: return_pass,
                        });
                    }
                    Some(WifiResult::Failed { error }) => {
                        warn!("WiFi connection failed: {}", error);

                        // Notify failure to BLE client
                        if let Some(sh) = status_handle {
                            let msg = format!(
                                r#"{{"status":"wifi_failed","error":"{}"}}"#,
                                error.replace('"', "'")
                            );
                            let _ = gatts.set_attr(sh, msg.as_bytes());

                            if let (Some(gif), Some(cid)) = (gatt_if, conn_id) {
                                let _ = gatts.notify(gif, cid, sh, msg.as_bytes());
                            }
                        }

                        // Loop back to wait for new credentials (retry)
                        info!("Waiting for new WiFi credentials...");
                        continue;
                    }
                    None => {
                        // WiFi thread channel disconnected or timed out
                        return Err(anyhow::anyhow!("WiFi result channel disconnected"));
                    }
                }
            }

            Ok(BleEvent::PeerConnected { conn_id: cid }) => {
                conn_id = Some(cid);
                led.show(LedStatus::BleConnected);
            }

            Ok(BleEvent::PeerDisconnected) => {
                conn_id = None;
                // Restart advertising
                let _ = gap.start_advertising();
            }

            Ok(BleEvent::WriteError(msg)) => {
                warn!("BLE write error: {}", msg);
                if let Some(sh) = status_handle {
                    let err_json = format!(r#"{{"status":"failed","error":"{}"}}"#, msg);
                    let _ = gatts.set_attr(sh, err_json.as_bytes());

                    if let (Some(gif), Some(cid)) = (gatt_if, conn_id) {
                        let _ = gatts.notify(gif, cid, sh, err_json.as_bytes());
                    }
                }
            }

            Ok(_) => {
                // Ignore setup events in provisioning loop
            }

            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Continue LED animation
            }

            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(anyhow::anyhow!("BLE event channel disconnected"));
            }
        }
    }
}
