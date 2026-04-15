//! BLE GATT provisioning frontend for Wi-Fi credential setup on ESP32-C6.
//!
//! The transport is ESP-IDF-specific, but the provisioning contract and session
//! loop are shared in `rhythm_os::provisioning`.

use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{anyhow, Result};
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
use rhythm_os::provisioning::{
    run_provisioning_session, ProvisioningBackend, ProvisioningConnectResult, ProvisioningDeviceInfo,
    ProvisioningEvent, ProvisioningFrontend, ProvisioningSessionConfig, ProvisioningStatus,
    WifiCredentials, PROVISIONING_DEVICE_INFO_UUID, PROVISIONING_SERVICE_UUID,
    PROVISIONING_STATUS_UUID, PROVISIONING_WIFI_CMD_UUID,
};

use crate::led::{LedStatus, Ws2812Led};

enum BleEvent {
    AppRegistered { gatt_if: u8 },
    ServiceCreated { service_handle: Handle },
    CharAdded {
        handle: Handle,
        service_handle: Handle,
        index: u8,
    },
    DescriptorAdded { service_handle: Handle },
    ServiceStarted,
    PeerConnected { conn_id: u16 },
    PeerDisconnected,
    Credentials(WifiCredentials),
    WriteError(String),
}

struct ChannelProvisioningBackend {
    cred_tx: Sender<WifiCredentials>,
    wifi_rx: Receiver<ProvisioningConnectResult>,
}

impl ChannelProvisioningBackend {
    fn new(
        cred_tx: Sender<WifiCredentials>,
        wifi_rx: Receiver<ProvisioningConnectResult>,
    ) -> Self {
        Self { cred_tx, wifi_rx }
    }
}

impl ProvisioningBackend for ChannelProvisioningBackend {
    fn begin_connect(&mut self, creds: WifiCredentials) -> Result<()> {
        self.cred_tx
            .send(creds)
            .map_err(|_| anyhow!("WiFi thread channel disconnected"))
    }

    fn poll_result(&mut self, timeout: Duration) -> Result<Option<ProvisioningConnectResult>> {
        match self.wifi_rx.recv_timeout(timeout) {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(anyhow!("WiFi result channel disconnected"))
            }
        }
    }
}

struct Esp32BleFrontend<'a, 'd> {
    led: &'a mut Ws2812Led,
    gap: EspBleGap<'d, Ble, &'a BtDriver<'d, Ble>>,
    gatts: EspGatts<'d, Ble, &'a BtDriver<'d, Ble>>,
    evt_rx: Receiver<BleEvent>,
    gatt_if: Option<u8>,
    status_handle: Option<Handle>,
    conn_id: Option<u16>,
}

impl<'a, 'd> Esp32BleFrontend<'a, 'd> {
    fn new(driver: &'a BtDriver<'d, Ble>, led: &'a mut Ws2812Led) -> Result<Self> {
        let gap = EspBleGap::new(driver)?;
        let gatts = EspGatts::new(driver)?;
        let (evt_tx, evt_rx) = mpsc::channel::<BleEvent>();
        let peer_addr: Arc<Mutex<Option<BdAddr>>> = Arc::new(Mutex::new(None));
        let peer_addr_gap = peer_addr.clone();

        gap.subscribe(move |event| match event {
            BleGapEvent::SecurityRequest => {
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

        let peer_addr_gatts = peer_addr.clone();
        let mut char_count: u8 = 0;
        gatts.subscribe(move |(gatt_if, event)| match event {
            GattsEvent::ServiceRegistered { status, app_id } => {
                info!("GATTS registered: status={:?} app={}", status, app_id);
                let _ = evt_tx.send(BleEvent::AppRegistered { gatt_if });
            }
            GattsEvent::ServiceCreated {
                status,
                service_handle,
                ..
            } => {
                info!(
                    "Service created: status={:?} handle={}",
                    status, service_handle
                );
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
                info!(
                    "Descriptor added: status={:?} svc={}",
                    status, service_handle
                );
                let _ = evt_tx.send(BleEvent::DescriptorAdded { service_handle });
            }
            GattsEvent::ServiceStarted { status, .. } => {
                info!("Service started: {:?}", status);
                let _ = evt_tx.send(BleEvent::ServiceStarted);
            }
            GattsEvent::PeerConnected { conn_id, addr, .. } => {
                info!("BLE client connected: conn_id={} addr={}", conn_id, addr);
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
                info!(
                    "BLE write handle={} len={} hex=[{}]",
                    handle,
                    value.len(),
                    value
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<Vec<_>>()
                        .join(" ")
                );
                if let Ok(s) = std::str::from_utf8(value) {
                    info!("BLE write UTF-8: {}", s);
                }

                if value.is_empty() || value[0] != b'{' {
                    return;
                }

                match std::str::from_utf8(value) {
                    Ok(json_str) => {
                        info!("BLE write on handle {}: {}", handle, json_str);
                        match serde_json::from_str::<serde_json::Value>(json_str) {
                            Ok(val) => {
                                let ssid = val.get("ssid").and_then(|v| v.as_str());
                                let password = val.get("password").and_then(|v| v.as_str());
                                match (ssid, password) {
                                    (Some(s), Some(p)) => {
                                        let _ = evt_tx.send(BleEvent::Credentials(WifiCredentials {
                                            ssid: s.to_string(),
                                            password: p.to_string(),
                                        }));
                                    }
                                    _ => {
                                        let _ = evt_tx.send(BleEvent::WriteError(
                                            "Missing ssid or password".to_string(),
                                        ));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ =
                                    evt_tx.send(BleEvent::WriteError(format!("Invalid JSON: {}", e)));
                            }
                        }
                    }
                    Err(e) => {
                        warn!("Invalid UTF-8 in BLE write: {}", e);
                    }
                }
            }
            _ => {}
        })?;

        Ok(Self {
            led,
            gap,
            gatts,
            evt_rx,
            gatt_if: None,
            status_handle: None,
            conn_id: None,
        })
    }

    fn notify_status_bytes(&mut self, bytes: &[u8]) {
        if let Some(handle) = self.status_handle {
            let _ = self.gatts.set_attr(handle, bytes);
            if let (Some(gatt_if), Some(conn_id)) = (self.gatt_if, self.conn_id) {
                let _ = self.gatts.notify(gatt_if, conn_id, handle, bytes);
            }
        }
    }
}

impl ProvisioningFrontend for Esp32BleFrontend<'_, '_> {
    fn start(&mut self, info: &ProvisioningDeviceInfo) -> Result<()> {
        info!("BLE device name: {}", info.name);

        self.gap.set_device_name(&info.name)?;
        self.gap.set_adv_conf(&AdvConfiguration {
            set_scan_rsp: false,
            include_name: false,
            include_txpower: false,
            flag: 0x06,
            service_uuid: Some(BtUuid::uuid128(PROVISIONING_SERVICE_UUID)),
            ..Default::default()
        })?;
        self.gap.set_adv_conf(&AdvConfiguration {
            set_scan_rsp: true,
            include_name: true,
            include_txpower: false,
            ..Default::default()
        })?;
        self.gap.set_security_conf(&SecurityConfiguration {
            auth_req_mode: AuthenticationRequest::SecureBonding,
            io_capabilities: IOCapabilities::NoInputNoOutput,
            initiator_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
            responder_key: Some(KeyMask::EncryptionKey | KeyMask::IdentityResolvingKey),
            max_key_size: Some(16),
            min_key_size: Some(7),
            ..Default::default()
        })?;

        self.gatt_if = None;
        self.status_handle = None;
        self.conn_id = None;

        self.gatts.register_app(0)?;

        let setup_timeout = Duration::from_secs(10);
        let setup_start = std::time::Instant::now();
        let mut service_running = false;
        let device_info_json = info.json_bytes()?;

        while !service_running && setup_start.elapsed() < setup_timeout {
            match self.evt_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(BleEvent::AppRegistered { gatt_if }) => {
                    self.gatt_if = Some(gatt_if);
                    let service_id = GattServiceId {
                        id: GattId {
                            uuid: BtUuid::uuid128(PROVISIONING_SERVICE_UUID),
                            inst_id: 0,
                        },
                        is_primary: true,
                    };
                    self.gatts.create_service(gatt_if, &service_id, 12)?;
                }
                Ok(BleEvent::ServiceCreated { service_handle }) => {
                    let perms: EnumSet<Permission> = Permission::Write.into();
                    let props: EnumSet<Property> = Property::Write | Property::WriteNoResponse;
                    let char_def = GattCharacteristic {
                        uuid: BtUuid::uuid128(PROVISIONING_WIFI_CMD_UUID),
                        permissions: perms,
                        properties: props,
                        max_len: 256,
                        auto_rsp: AutoResponse::ByGatt,
                    };
                    self.gatts
                        .add_characteristic(service_handle, &char_def, &[])?;
                }
                Ok(BleEvent::CharAdded {
                    handle,
                    service_handle,
                    index,
                }) => match index {
                    1 => {
                        let perms: EnumSet<Permission> = Permission::Read.into();
                        let props: EnumSet<Property> = Property::Read | Property::Notify;
                        let char_def = GattCharacteristic {
                            uuid: BtUuid::uuid128(PROVISIONING_STATUS_UUID),
                            permissions: perms,
                            properties: props,
                            max_len: 256,
                            auto_rsp: AutoResponse::ByGatt,
                        };
                        let initial = ProvisioningStatus::Waiting.json_bytes()?;
                        self.gatts
                            .add_characteristic(service_handle, &char_def, &initial)?;
                    }
                    2 => {
                        self.status_handle = Some(handle);
                        let cccd_uuid = BtUuid::uuid16(0x2902);
                        let mut cccd_val = [0u8; 2];
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
                        self.gatts.start_service(service_handle)?;
                    }
                    _ => {}
                },
                Ok(BleEvent::DescriptorAdded { service_handle }) => {
                    let perms: EnumSet<Permission> = Permission::Read.into();
                    let props: EnumSet<Property> = Property::Read.into();
                    let char_def = GattCharacteristic {
                        uuid: BtUuid::uuid128(PROVISIONING_DEVICE_INFO_UUID),
                        permissions: perms,
                        properties: props,
                        max_len: 256,
                        auto_rsp: AutoResponse::ByGatt,
                    };
                    self.gatts.add_characteristic(
                        service_handle,
                        &char_def,
                        device_info_json.as_slice(),
                    )?;
                }
                Ok(BleEvent::ServiceStarted) => {
                    service_running = true;
                }
                Ok(_) => {}
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(anyhow!("BLE event channel disconnected during setup"));
                }
            }
        }

        if !service_running {
            return Err(anyhow!("BLE GATT service setup timed out"));
        }

        self.gap.start_advertising()?;
        self.led.show(LedStatus::BleProvisioning);
        info!("BLE provisioning active — waiting for WiFi credentials...");

        Ok(())
    }

    fn poll_event(&mut self, timeout: Duration) -> Result<Option<ProvisioningEvent>> {
        match self.evt_rx.recv_timeout(timeout) {
            Ok(BleEvent::Credentials(creds)) => {
                info!("WiFi credentials received via BLE: ssid={}", creds.ssid);
                Ok(Some(ProvisioningEvent::Credentials(creds)))
            }
            Ok(BleEvent::WriteError(msg)) => {
                warn!("BLE write error: {}", msg);
                Ok(Some(ProvisioningEvent::Error(msg)))
            }
            Ok(BleEvent::PeerConnected { conn_id }) => {
                self.conn_id = Some(conn_id);
                self.led.show(LedStatus::BleConnected);
                Ok(None)
            }
            Ok(BleEvent::PeerDisconnected) => {
                self.conn_id = None;
                let _ = self.gap.start_advertising();
                self.led.show(LedStatus::BleProvisioning);
                Ok(None)
            }
            Ok(_) => Ok(None),
            Err(mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(anyhow!("BLE event channel disconnected"))
            }
        }
    }

    fn publish_status(&mut self, status: &ProvisioningStatus) -> Result<()> {
        let bytes = status.json_bytes()?;
        self.notify_status_bytes(bytes.as_slice());

        match status {
            ProvisioningStatus::Waiting => self.led.show(LedStatus::BleProvisioning),
            ProvisioningStatus::Connecting => self.led.show(LedStatus::WifiConnecting),
            ProvisioningStatus::Connected { .. } => {}
            ProvisioningStatus::WifiFailed { .. } | ProvisioningStatus::Failed { .. } => {
                self.led.show(LedStatus::BleProvisioning)
            }
        }

        Ok(())
    }

    fn stop(&mut self) -> Result<()> {
        info!("Stopping BLE provisioning frontend...");
        let _ = self.gap.stop_advertising();
        Ok(())
    }
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
pub fn run_provisioning<'d>(
    modem: BluetoothModem<'d>,
    nvs: EspDefaultNvsPartition,
    led: &mut Ws2812Led,
    cred_tx: Sender<WifiCredentials>,
    wifi_rx: Receiver<ProvisioningConnectResult>,
) -> Result<WifiCredentials> {
    info!("Starting BLE provisioning...");

    let mac = get_mac_address();
    let identity = ProvisioningDeviceInfo {
        name: format!("Rhythm-{:02X}{:02X}", mac[4], mac[5]),
        version: crate::FIRMWARE_VERSION.to_string(),
        mac: Some(format!(
            "{:02X}:{:02X}:{:02X}:{:02X}:{:02X}:{:02X}",
            mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
        )),
    };

    let bt_driver = BtDriver::<Ble>::new(modem, Some(nvs.clone()))?;
    info!("BLE driver initialized");

    esp_idf_svc::bt::ble::gatt::set_local_mtu(256)?;

    let mut frontend = Esp32BleFrontend::new(&bt_driver, led)?;
    let mut backend = ChannelProvisioningBackend::new(cred_tx, wifi_rx);
    let config = ProvisioningSessionConfig::default();

    run_provisioning_session(&mut frontend, &mut backend, &identity, &config)
}
