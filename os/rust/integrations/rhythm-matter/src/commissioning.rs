//! Shared Matter commissioning orchestration.

use std::sync::Arc;

use anyhow::Context;
use anyhow::Result;
use log::{error, info, warn};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::canonical::identity::{HardwareId, HubKey};
use rhythm_os::hub::HubType;
use rhythm_os::pairing::{PairedDeviceInfo, PairingSession, PairingStage, PairingStatus};
use rhythm_os::state::SharedState;
use serde_json::Value;

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterCommissionRequest, MatterCommissioningNetwork,
    MatterCommissioningRendezvous, MatterCommissioningWifiCredentials, MatterTransport,
};

/// Parsed Matter pairing request owned by `rhythm-matter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatterPairingParams {
    /// Raw Matter setup payload. May be an `MT:` QR payload or a manual code.
    pub setup_payload: String,
    /// Optional client-generated pairing session ID for SSE correlation.
    pub session_id: Option<String>,
    /// Matter network being commissioned.
    pub network: MatterCommissioningNetwork,
    /// How the commissioner reaches the device.
    pub rendezvous: MatterCommissioningRendezvous,
}

impl MatterPairingParams {
    /// Parse the integration-specific request payload.
    ///
    /// When clients omit `rendezvous`, manual setup codes default to
    /// on-network commissioning while QR payloads keep the existing auto
    /// behavior.
    pub fn from_value(params: &Value) -> Result<Self> {
        let setup_payload = params
            .get("setup_payload")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| anyhow::anyhow!("Missing 'setup_payload' in pairing params"))?;
        let session_id = params
            .get("session_id")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);

        let network = match params.get("network").and_then(|value| value.as_str()) {
            None | Some("wifi") => MatterCommissioningNetwork::Wifi,
            Some(other) => anyhow::bail!(
                "Unsupported Matter network '{}'; only 'wifi' is currently supported",
                other
            ),
        };

        let rendezvous = match params.get("rendezvous").and_then(|value| value.as_str()) {
            None => default_rendezvous_for_payload(setup_payload),
            Some("auto") => MatterCommissioningRendezvous::Auto,
            Some("ble") => MatterCommissioningRendezvous::Ble,
            Some("on_network") => MatterCommissioningRendezvous::OnNetwork,
            Some(other) => anyhow::bail!(
                "Unsupported Matter rendezvous '{}'; expected 'auto', 'ble', or 'on_network'",
                other
            ),
        };

        Ok(Self {
            setup_payload: setup_payload.to_string(),
            session_id,
            network,
            rendezvous,
        })
    }

    /// Build the transport-facing request for a specific local node ID.
    pub fn to_commission_request(
        &self,
        node_id: u64,
        wifi_credentials: MatterCommissioningWifiCredentials,
    ) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: self.setup_payload.clone(),
            node_id,
            network: self.network,
            rendezvous: self.rendezvous,
            wifi_credentials,
        }
    }
}

fn default_rendezvous_for_payload(setup_payload: &str) -> MatterCommissioningRendezvous {
    if is_qr_setup_payload(setup_payload) {
        MatterCommissioningRendezvous::Auto
    } else {
        MatterCommissioningRendezvous::OnNetwork
    }
}

fn is_qr_setup_payload(setup_payload: &str) -> bool {
    setup_payload
        .get(..3)
        .map(|prefix| prefix.eq_ignore_ascii_case("MT:"))
        .unwrap_or(false)
}

/// Ensure the local Matter hub exists before commissioning.
pub fn ensure_matter_hub_connected(state: &SharedState) -> Result<()> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let already_connected = state
        .lock()
        .map(|state| state.hubs.contains_key(&hub_key))
        .unwrap_or(false);

    if !already_connected {
        info!(target: "pair", "Matter hub not connected, auto-bootstrapping...");
        let credentials = serde_json::json!({ "fabric_id": "default" });
        rhythm_os::commands::do_hub_credentials(state, "matter", "local", &credentials)?;
    }

    Ok(())
}

/// Commission a Matter-over-WiFi light using the shared transport.
pub fn pair_device(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
    request: &MatterPairingParams,
) -> Result<PairingSession> {
    let wifi_credentials = load_commissioning_wifi_credentials(state)?;
    let node_id = hub_data.reserve_node_id();
    let commission_request = request.to_commission_request(node_id, wifi_credentials);
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        request.session_id.as_deref(),
        PairingStatus::Commissioning,
        PairingStage::Commissioning,
        "Commissioning Matter device",
        None,
        None,
    );

    match transport.commission_light(&commission_request) {
        Ok(device) => {
            build_success_session(state, &hub_data, device, request.session_id.as_deref())
        }
        Err(error) => {
            error!(target: "pair", "Matter commissioning error: {:#}", error);
            Ok(PairingSession {
                hub_type: "matter".to_string(),
                status: PairingStatus::Failed,
                device: None,
                error: Some(summarize_commissioning_error(&error)),
            })
        }
    }
}

fn summarize_commissioning_error(error: &anyhow::Error) -> String {
    let detail = format!("{:#}", error);
    let lower = detail.to_ascii_lowercase();

    if lower.contains("addressresolve") || lower.contains("operational discovery failed") {
        return "The light joined the Wi-Fi network, but Rhythm could not discover it over mDNS afterwards. Rhythm reset its Matter controller to recover; wait a few seconds and retry pairing without factory-resetting the light.".to_string();
    }

    if lower.contains("gatt write characteristic operation failed") {
        return "Matter BLE commissioning reached the bulb, but macOS CoreBluetooth failed the GATT write. This matches the current official Matter controller behavior on this host. Try Linux/BlueZ or the appliance target for real commissioning.".to_string();
    }

    if lower.contains("connectiondelegate timeout")
        || lower.contains("discovery timed out")
        || lower.contains("pasesession.cpp")
        || lower.contains("blemanagerimpl.cpp")
        || lower.contains("chip error 0x00000032: timeout")
    {
        return "Matter BLE commissioning timed out while discovering the bulb from this host. Factory-reset the bulb, keep it close to the machine, and if it still fails, try Linux/BlueZ or the appliance target.".to_string();
    }

    if is_linux_ble_stack_error(&lower)
        || lower.contains("chip error 0x000000ac")
        || lower.contains("ble device doesn't seem to support chip")
    {
        return "Matter BLE pairing reached the appliance Bluetooth stack, but BlueZ/CHIP lost the BLE connection during commissioning. Rhythm reset the Matter controller; wait a few seconds, keep the light close, and retry pairing.".to_string();
    }

    if lower.contains("matter wi-fi commissioning requires stored appliance wi-fi credentials") {
        return "Matter pairing needs stored appliance Wi-Fi credentials on the server before a light can be commissioned.".to_string();
    }

    detail
}

fn is_linux_ble_stack_error(lower_detail: &str) -> bool {
    [
        "matter ble commissioning failed",
        "blemanagerimpl.cpp",
        "bluezendpoint.cpp",
        "bluezobjectmanager.cpp",
        "pasesession.cpp",
        "chipoble",
        "ble adapter unavailable",
        "d-bus system bus",
        "operation was cancelled",
    ]
    .iter()
    .any(|needle| lower_detail.contains(needle))
}

fn load_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<MatterCommissioningWifiCredentials> {
    let wifi = match load_stored_commissioning_wifi_credentials(state)? {
        Some(wifi) => wifi,
        None => load_platform_commissioning_wifi_credentials(state)?.ok_or_else(|| {
            anyhow::anyhow!(
                "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
            )
        })?,
    };

    Ok(MatterCommissioningWifiCredentials {
        ssid: wifi.ssid,
        password: wifi.password,
    })
}

fn load_stored_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let Some(storage) = state.storage.as_ref() else {
        return Ok(None);
    };

    storage
        .load_commissioning_wifi_credentials()
        .context("loading stored appliance Wi-Fi credentials")
}

fn load_platform_commissioning_wifi_credentials(
    state: &SharedState,
) -> Result<Option<rhythm_os::provisioning::WifiCredentials>> {
    let provider = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?
        .commissioning_wifi_credentials_provider
        .clone();
    let Some(provider) = provider else {
        return Ok(None);
    };

    let creds = provider().context("loading platform appliance Wi-Fi credentials")?;
    if let Some(creds) = creds.as_ref() {
        persist_commissioning_wifi_credentials(state, creds);
        info!(
            target: "pair",
            "Recovered Matter commissioning Wi-Fi credentials from platform network config for SSID '{}'",
            creds.ssid
        );
    }
    Ok(creds)
}

fn persist_commissioning_wifi_credentials(
    state: &SharedState,
    creds: &rhythm_os::provisioning::WifiCredentials,
) {
    let result = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))
        .and_then(|state| {
            let storage = state
                .storage
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
            storage.save_commissioning_wifi_credentials(creds)
        });

    if let Err(error) = result {
        warn!(
            target: "pair",
            "Failed to persist recovered Matter commissioning Wi-Fi credentials: {:#}",
            error
        );
    }
}

fn build_success_session(
    state: &SharedState,
    hub_data: &Arc<MatterHubData>,
    device: CommissionedDevice,
    session_id: Option<&str>,
) -> Result<PairingSession> {
    rhythm_os::pairing::emit_pairing_progress(
        state,
        "matter",
        session_id,
        PairingStatus::Commissioning,
        PairingStage::Finalizing,
        "Finalizing paired Matter device",
        None,
        None,
    );

    let device_id = crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);
    let device_name = format!("{} {}", device.vendor_name, device.product_name);
    let hub_key = HubKey::new(HubType::new("matter"), "local");

    info!(
        target: "sys",
        "Matter: paired {} (node {}, id={})",
        device_name,
        device.node_id,
        device_id
    );

    hub_data.record_commissioned_device(&device);
    store_device_metadata(hub_data, &device, &device_id);
    if let Err(error) = crate::capture::persist_device_capture(hub_data, &device, "pair") {
        warn!(
            target: "sys",
            "Matter: failed to persist pair capture for {}: {}",
            device_id,
            error
        );
    }
    register_canonical_identity(state, &hub_key, &device, &device_id, &device_name)?;
    materialize_unassigned_canonical_device(state, &hub_key, &device_id)?;

    let _ = hub_data.event_tx.send(
        crate::events::device_paired_event(
            device.node_id,
            &device.vendor_name,
            &device.product_name,
        )
        .with_hub_key(hub_key),
    );

    Ok(PairingSession {
        hub_type: "matter".to_string(),
        status: PairingStatus::Complete,
        device: Some(PairedDeviceInfo {
            device_id,
            name: device_name,
            device_type: DeviceType::Light,
            manufacturer: Some(device.vendor_name),
            model: Some(device.product_name),
        }),
        error: None,
    })
}

pub(crate) fn store_device_metadata(
    hub_data: &Arc<MatterHubData>,
    device: &CommissionedDevice,
    device_id: &str,
) {
    let mut caps = build_device_capabilities(device);
    let mut quirks = build_device_quirks(device);
    if let Ok(cloud_profiles) = hub_data.cloud_profiles.lock() {
        cloud_profiles.apply_to_device(device, &mut caps, &mut quirks);
    }

    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        device_caps.insert(device_id.to_string(), caps);
        info!(
            target: "sys",
            "Matter: stored caps for {} ({} devices tracked)",
            device_id,
            device_caps.len()
        );
    }

    if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
        device_quirks.insert(device_id.to_string(), quirks);
    }
}

pub(crate) fn fallback_device_capabilities() -> rhythm_devices::LightCapabilities {
    rhythm_devices::LightCapabilities {
        color_modes: vec![
            rhythm_devices::ColorMode::HueSaturation,
            rhythm_devices::ColorMode::ColorTemperature,
        ],
        ..rhythm_devices::LightCapabilities::defaults_for(rhythm_devices::LightType::ExtendedColor)
    }
}

pub(crate) fn store_fallback_device_metadata(hub_data: &Arc<MatterHubData>, device_id: &str) {
    if let Ok(mut device_caps) = hub_data.device_caps.lock() {
        if !device_caps.contains_key(device_id) {
            device_caps.insert(device_id.to_string(), fallback_device_capabilities());
            info!(
                target: "sys",
                "Matter: stored fallback caps for {} ({} devices tracked)",
                device_id,
                device_caps.len()
            );
        }
    }

    if let Ok(mut device_quirks) = hub_data.device_quirks.lock() {
        device_quirks.entry(device_id.to_string()).or_default();
    }
}

pub(crate) fn build_device_capabilities(
    device: &CommissionedDevice,
) -> rhythm_devices::LightCapabilities {
    let mut caps = crate::capabilities::capabilities_from_commissioned(device);
    crate::capabilities::enrich_from_db(&mut caps, device, rhythm_devices::builtin_db());
    caps
}

pub(crate) fn build_device_quirks(device: &CommissionedDevice) -> Vec<rhythm_devices::DeviceQuirk> {
    crate::capabilities::quirks_from_db(device, rhythm_devices::builtin_db())
}

fn register_canonical_identity(
    state: &SharedState,
    hub_key: &HubKey,
    device: &CommissionedDevice,
    device_id: &str,
    device_name: &str,
) -> Result<()> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let identity = rhythm_os::canonical::identity::DiscoveredIdentity {
        native_id: device_id.to_string(),
        room_id: None,
        room_name: None,
        name: device_name.to_string(),
        device_type: DeviceType::Light,
        hardware_ids: vec![HardwareId::matter(&device.node_id.to_string())],
        manufacturer: Some(device.vendor_name.clone()),
        model: Some(device.product_name.clone()),
    };

    let mut state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    state.canonical_registry.resolve(&identity, hub_key, now);
    Ok(())
}

fn materialize_unassigned_canonical_device(
    state: &SharedState,
    hub_key: &HubKey,
    device_id: &str,
) -> Result<()> {
    let state_guard = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
    let (canonical_id, already_assigned) = state_guard
        .canonical_registry
        .find_by_native_id(hub_key, device_id)
        .map(|device| (device.id.clone(), device.room_id.is_some()))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Canonical device missing after Matter pairing: {}",
                device_id
            )
        })?;

    drop(state_guard);

    if already_assigned {
        rhythm_os::commands::reconcile_runtime_from_state(state)?;
    } else {
        rhythm_os::commands::do_canonical_assign_room(state, &canonical_id, None)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Mutex, OnceLock};

    use rhythm_os::hub::HubEvent;
    use rhythm_os::provisioning::WifiCredentials;
    use rhythm_os::storage::{FileStorage, Storage};

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::controller::MatterDeviceRegistry;
    use crate::transport::{
        MatterColorMode, MatterDeviceInfo, MatterGroup, MatterGroupMember,
        MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
    };

    #[derive(Default)]
    struct FakeMatterTransport {
        commission_requests: Mutex<Vec<MatterCommissionRequest>>,
        commission_error: Mutex<Option<String>>,
        subscribe_calls: AtomicUsize,
    }

    impl FakeMatterTransport {
        fn with_commission_error(error: impl Into<String>) -> Self {
            Self {
                commission_error: Mutex::new(Some(error.into())),
                ..Self::default()
            }
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            self.commission_requests
                .lock()
                .unwrap()
                .push(request.clone());
            if let Some(error) = self.commission_error.lock().unwrap().clone() {
                anyhow::bail!(error);
            }
            Ok(commissioned_device(request.node_id))
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(Vec::new())
        }

        fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
            Ok(commissioned_device(node_id))
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn configure_group(&self, _group: &MatterGroup) -> Result<()> {
            Ok(())
        }

        fn remove_group(&self, _group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn run_level_command(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _command: MatterLevelCommandVariant,
            _level_or_step: u8,
            _step_mode: Option<MatterLevelStepMode>,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }

        fn subscribe_on_off(
            &self,
            _targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> Result<()> {
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("automatic subscriptions are disabled")
        }
    }

    fn state() -> SharedState {
        Arc::new(std::sync::Mutex::new(rhythm_os::state::AppState::default()))
    }

    fn unique_data_dir(name: &str) -> std::path::PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rhythm-matter-commissioning-{name}-{nanos}"))
    }

    fn state_with_storage(name: &str) -> (SharedState, std::path::PathBuf) {
        let path = unique_data_dir(name);
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        let state = state();
        state.lock().unwrap().storage = Some(std::sync::Arc::new(storage));
        (state, path)
    }

    fn wifi(ssid: &str, password: &str) -> WifiCredentials {
        WifiCredentials {
            ssid: ssid.to_string(),
            password: password.to_string(),
        }
    }

    fn save_wifi(path: &std::path::Path, creds: &WifiCredentials) {
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        storage.save_commissioning_wifi_credentials(creds).unwrap();
    }

    fn commissioned_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Acme".to_string(),
            product_name: "Color Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: 2,
            color_modes: vec![
                MatterColorMode::ColorTemperature,
                MatterColorMode::Xy,
                MatterColorMode::HueSaturation,
            ],
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
        }
    }

    fn hub_data() -> (Arc<MatterHubData>, std::sync::mpsc::Receiver<HubEvent>) {
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        (
            Arc::new(MatterHubData {
                transport: OnceLock::new(),
                capture_dir: OnceLock::new(),
                registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
                fabric_id: "default".to_string(),
                commissioned: Mutex::new(Vec::new()),
                next_node_id: AtomicU64::new(10),
                device_caps: Mutex::new(HashMap::new()),
                device_quirks: Mutex::new(HashMap::new()),
                cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
                decommissioning: Mutex::new(HashSet::new()),
                recently_decommissioned: Mutex::new(HashMap::new()),
                node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
                event_tx,
            }),
            event_rx,
        )
    }

    fn install_transport(
        hub_data: &Arc<MatterHubData>,
        transport: Arc<FakeMatterTransport>,
    ) -> Arc<FakeMatterTransport> {
        let transport_dyn: Arc<dyn MatterTransport> = transport.clone();
        assert!(hub_data.transport.set(transport_dyn).is_ok());
        transport
    }

    fn pairing_request() -> MatterPairingParams {
        MatterPairingParams::from_value(&serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "session_id": "pair-1",
            "rendezvous": "ble"
        }))
        .unwrap()
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn pairing_params_accept_raw_mt_payload() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "network": "wifi",
            "rendezvous": "ble"
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        let request = parsed.to_commission_request(
            123,
            MatterCommissioningWifiCredentials {
                ssid: "wifi".to_string(),
                password: "secret".to_string(),
            },
        );

        assert_eq!(request.setup_payload, "MT:Y.K908OC16750648G00");
        assert_eq!(request.node_id, 123);
        assert_eq!(request.rendezvous, MatterCommissioningRendezvous::Ble);
    }

    #[test]
    fn pairing_params_trim_validate_and_default_manual_codes_to_on_network() {
        let parsed = MatterPairingParams::from_value(&serde_json::json!({
            "setup_payload": " 12345678901 ",
            "session_id": "   ",
        }))
        .unwrap();

        assert_eq!(parsed.setup_payload, "12345678901");
        assert_eq!(parsed.session_id, None);
        assert_eq!(parsed.network, MatterCommissioningNetwork::Wifi);
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::OnNetwork);

        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({}))),
            "Missing 'setup_payload' in pairing params"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": " ",
            }))),
            "Missing 'setup_payload' in pairing params"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": "MT:payload",
                "network": "thread",
            }))),
            "Unsupported Matter network 'thread'; only 'wifi' is currently supported"
        );
        assert_eq!(
            string_error(MatterPairingParams::from_value(&serde_json::json!({
                "setup_payload": "MT:payload",
                "rendezvous": "nfc",
            }))),
            "Unsupported Matter rendezvous 'nfc'; expected 'auto', 'ble', or 'on_network'"
        );
    }

    #[test]
    fn pairing_params_default_to_auto_rendezvous() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.rendezvous, MatterCommissioningRendezvous::Auto);
    }

    #[test]
    fn pairing_params_accept_session_id_for_progress_correlation() {
        let params = serde_json::json!({
            "setup_payload": "MT:Y.K908OC16750648G00",
            "session_id": "pair-1",
        });

        let parsed = MatterPairingParams::from_value(&params).unwrap();
        assert_eq!(parsed.session_id.as_deref(), Some("pair-1"));
    }

    #[test]
    fn commissioning_error_summarizes_darwin_gatt_failures() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: Ble Error 0x00000407: GATT write characteristic operation failed"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("CoreBluetooth failed the GATT write"));
        assert!(message.contains("Linux/BlueZ"));
    }

    #[test]
    fn commissioning_error_summarizes_ble_timeouts() {
        let error = anyhow::anyhow!("commissioning Matter light: ConnectionDelegate timeout");

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("timed out while discovering the bulb"));
    }

    /// Regression for issue #117: operational-discovery timeouts were
    /// summarized with the BLE-discovery message telling the user to
    /// factory-reset the bulb and keep it close — advice that cannot help
    /// when the host's mDNS resolution is what actually failed.
    #[test]
    fn commissioning_error_summarizes_operational_discovery_timeouts() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/lib/address_resolve/AddressResolve_DefaultImpl.cpp:124: CHIP Error 0x00000032: Timeout"
        )
        .context("Matter operational discovery failed; reset CHIP sidecar before next attempt");

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("could not discover it over mDNS"));
        assert!(message.contains("retry pairing"));
        assert!(!message.contains("Factory-reset the bulb"));
        assert!(!message.contains("AddressResolve_DefaultImpl.cpp"));
    }

    #[test]
    fn commissioning_error_summarizes_linux_chip_timeouts() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/protocols/secure_channel/PASESession.cpp:310: CHIP Error 0x00000032: Timeout"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("timed out while discovering the bulb"));
        assert!(!message.contains("PASESession.cpp"));
    }

    #[test]
    fn commissioning_error_summarizes_linux_ble_stack_failures() {
        for detail in [
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: FAIL: Get D-Bus system bus: Could not connect: Connection refused",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezObjectManager.cpp:118: CHIP Error 0x000000AC: Internal error",
            "Matter BLE commissioning failed; reset CHIP sidecar before next attempt: commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:493: Operation was cancelled",
        ] {
            let error = anyhow::anyhow!(detail);

            let message = summarize_commissioning_error(&error);

            assert!(
                message.contains("BlueZ/CHIP lost the BLE connection"),
                "expected BlueZ summary for {detail}"
            );
            assert!(message.contains("retry pairing"));
        }
    }

    #[test]
    fn commissioning_error_summarizes_bluez_internal_errors() {
        let error = anyhow::anyhow!(
            "commissioning Matter light: src/platform/Linux/bluez/BluezEndpoint.cpp:623: CHIP Error 0x000000AC: Internal error"
        );

        let message = summarize_commissioning_error(&error);

        assert!(message.contains("BlueZ"));
        assert!(!message.contains("BluezEndpoint.cpp"));
    }

    #[test]
    fn wifi_credentials_load_from_storage_or_platform_and_persist_recovered_values() {
        let state = state();
        assert_eq!(
            string_error(load_commissioning_wifi_credentials(&state)),
            "Matter Wi-Fi commissioning requires stored appliance Wi-Fi credentials; provision the appliance over Wi-Fi before pairing Matter lights"
        );

        let stored_wifi = wifi("StoredNet", "stored-secret");
        let (stored_state, stored_path) = state_with_storage("stored");
        save_wifi(&stored_path, &stored_wifi);

        let loaded = load_commissioning_wifi_credentials(&stored_state).unwrap();
        assert_eq!(loaded.ssid, stored_wifi.ssid);
        assert_eq!(loaded.password, stored_wifi.password);
        std::fs::remove_dir_all(stored_path).ok();

        let recovered_wifi = wifi("PlatformNet", "platform-secret");
        let (platform_state, platform_path) = state_with_storage("platform");
        platform_state
            .lock()
            .unwrap()
            .commissioning_wifi_credentials_provider = Some(Arc::new({
            let recovered_wifi = recovered_wifi.clone();
            move || Ok(Some(recovered_wifi.clone()))
        }));

        let loaded = load_commissioning_wifi_credentials(&platform_state).unwrap();
        assert_eq!(loaded.ssid, recovered_wifi.ssid);
        assert_eq!(loaded.password, recovered_wifi.password);

        let storage = FileStorage::new(platform_path.to_str().unwrap()).unwrap();
        assert_eq!(
            storage.load_commissioning_wifi_credentials().unwrap(),
            Some(recovered_wifi)
        );
        std::fs::remove_dir_all(platform_path).ok();
    }

    #[test]
    fn ensure_matter_hub_connected_reports_missing_provider_when_not_bootstrapped() {
        let state = state();
        assert_eq!(
            string_error(ensure_matter_hub_connected(&state)),
            "No hub provider registered"
        );
    }

    #[test]
    fn pair_device_success_records_fabric_state_metadata_and_event_without_subscription() {
        let (state, path) = state_with_storage("pair-success");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, event_rx) = hub_data();
        let transport = install_transport(&hub_data, Arc::new(FakeMatterTransport::default()));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Complete);
        let paired = session.device.unwrap();
        assert_eq!(paired.device_id, "matter-10-2");
        assert_eq!(paired.name, "Acme Color Lamp");
        assert_eq!(paired.device_type, DeviceType::Light);

        let requests = transport.commission_requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].node_id, 10);
        assert_eq!(requests[0].wifi_credentials.ssid, "PairNet");
        assert_eq!(requests[0].wifi_credentials.password, "pair-secret");
        assert_eq!(requests[0].rendezvous, MatterCommissioningRendezvous::Ble);
        drop(requests);

        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 0);
        assert_eq!(hub_data.commissioned.lock().unwrap()[0].node_id, 10);
        assert!(hub_data
            .device_caps
            .lock()
            .unwrap()
            .contains_key("matter-10-2"));
        assert!(hub_data
            .device_quirks
            .lock()
            .unwrap()
            .contains_key("matter-10-2"));

        let matter_key = HubKey::new(HubType::new("matter"), "local");
        let state_guard = state.lock().unwrap();
        let canonical = state_guard
            .canonical_registry
            .find_by_native_id(&matter_key, "matter-10-2")
            .expect("paired device should be in canonical registry");
        assert!(state_guard
            .topology
            .get_device_node(&canonical.id)
            .is_some());
        drop(state_guard);

        match event_rx.try_recv().unwrap() {
            HubEvent::DevicePaired {
                hub_key,
                device_id,
                name,
                device_type,
            } => {
                assert_eq!(hub_key, Some(matter_key));
                assert_eq!(device_id, "matter-10");
                assert_eq!(name, "Acme Color Lamp");
                assert_eq!(device_type, DeviceType::Light);
            }
            other => panic!("expected device paired event, got {other:?}"),
        }

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn pair_device_failure_reports_summarized_error_without_recording_device() {
        let (state, path) = state_with_storage("pair-failure");
        save_wifi(&path, &wifi("PairNet", "pair-secret"));
        let (hub_data, _event_rx) = hub_data();
        let transport = Arc::new(FakeMatterTransport::with_commission_error(
            "ConnectionDelegate timeout",
        ));

        let session = pair_device(
            &state,
            transport.clone() as Arc<dyn MatterTransport>,
            hub_data.clone(),
            &pairing_request(),
        )
        .unwrap();

        assert_eq!(session.status, PairingStatus::Failed);
        assert!(session.device.is_none());
        assert!(session
            .error
            .as_deref()
            .unwrap()
            .contains("timed out while discovering the bulb"));
        assert_eq!(transport.commission_requests.lock().unwrap().len(), 1);
        assert!(hub_data.commissioned.lock().unwrap().is_empty());

        std::fs::remove_dir_all(path).ok();
    }
}
