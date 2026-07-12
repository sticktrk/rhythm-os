//! Matter hub lifecycle — connect, disconnect, runtime creation.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use log::info;
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::{CommissionedDevice, MatterDeviceInfo, MatterTransport};

/// Connect to the local Matter fabric.
pub fn connect_matter(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let persisted_devices = transport.list_commissioned_devices().unwrap_or_default();
    let commissioned = if persisted_devices.is_empty() {
        transport.list_devices().unwrap_or_default()
    } else {
        persisted_devices
            .iter()
            .map(device_info_from_record)
            .collect()
    };
    let cloud_profiles = crate::cloud_profiles::load_or_sync_for_state(state);
    let initial_metadata = initial_device_metadata(
        state,
        &commissioned,
        &persisted_devices,
        &cloud_profiles,
        &hub_key,
    );
    let next_node_id = next_node_id_seed(&commissioned);
    let fabric_id = configured_fabric_id(state, &hub_key);

    info!(target: "sys", "Matter: {} commissioned devices", commissioned.len());

    let snapshot = {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        state
            .storage
            .as_ref()
            .and_then(|storage| storage.load_hub_registry_for(&hub_key).ok().flatten())
            .and_then(|value| {
                serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
            })
    };

    let commissioned_for_closure = commissioned.clone();
    let transport_for_closure = transport.clone();
    let fabric_id_for_closure = fabric_id.clone();
    let cloud_profiles_for_hub_data = cloud_profiles.clone();
    let node_proof_of_life = Arc::new(Mutex::new(HashMap::new()));
    let node_proof_of_life_for_closure = node_proof_of_life.clone();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let _ = event_tx.send(HubEvent::Connected {
        hub_key: Some(hub_key.clone()),
    });

    let (hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new("matter"),
        hub_key,
        true,
        snapshot,
        move |registry: Arc<Mutex<HubDeviceRegistry>>| -> Box<dyn std::any::Any + Send + Sync> {
            let transport_cell = {
                let cell = std::sync::OnceLock::new();
                let _ = cell.set(transport_for_closure.clone());
                cell
            };

            Box::new(Arc::new(MatterHubData {
                transport: transport_cell,
                capture_dir: std::sync::OnceLock::new(),
                registry,
                fabric_id: fabric_id_for_closure.clone(),
                commissioned: std::sync::Mutex::new(commissioned_for_closure.clone()),
                next_node_id: std::sync::atomic::AtomicU64::new(next_node_id),
                device_caps: std::sync::Mutex::new(initial_metadata.device_caps.clone()),
                device_quirks: std::sync::Mutex::new(initial_metadata.device_quirks.clone()),
                cloud_profiles: std::sync::Mutex::new(cloud_profiles_for_hub_data.clone()),
                decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
                recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
                node_proof_of_life: node_proof_of_life_for_closure.clone(),
                event_tx,
            }))
        },
        move |_registry, _shutdown| event_rx,
    )?;

    Ok((hub, event_rx))
}

fn device_info_from_record(device: &CommissionedDevice) -> MatterDeviceInfo {
    MatterDeviceInfo {
        node_id: device.node_id,
        vendor_name: device.vendor_name.clone(),
        product_name: device.product_name.clone(),
        // Persisted metadata proves identity and capabilities, not current
        // liveness. Keep the neutral startup behavior; command/read outcomes
        // update reachability after connect without starting background work.
        reachable: true,
    }
}

fn configured_fabric_id(state: &SharedState, hub_key: &HubKey) -> String {
    state
        .lock()
        .ok()
        .and_then(|state| state.hub_credentials.get(hub_key).cloned())
        .and_then(|creds| crate::provider::matter_fabric_id(&creds).map(ToOwned::to_owned))
        .unwrap_or_else(|| "default".to_string())
}

fn next_node_id_seed(commissioned: &[MatterDeviceInfo]) -> u64 {
    commissioned
        .iter()
        .map(|device| device.node_id)
        .max()
        .unwrap_or(99)
        .saturating_add(1)
}

#[derive(Clone)]
struct InitialDeviceMetadata {
    device_caps: HashMap<String, LightCapabilities>,
    device_quirks: HashMap<String, Vec<DeviceQuirk>>,
}

/// Build the best metadata available without talking to the device.
///
/// `list_devices` is backed by chipd's persisted device store, so its vendor
/// and product names survive a temporary connectivity failure. Preserve any
/// matching built-in profile here instead of replacing it with an anonymous
/// extended-color fallback. In particular, command quirks such as
/// `NeedsExplicitOn` remain necessary while the device is unreachable to
/// probes but reachable again by the time a light command is dispatched.
fn fallback_device_metadata(info: &MatterDeviceInfo) -> (LightCapabilities, Vec<DeviceQuirk>) {
    let Some(entry) =
        rhythm_devices::builtin_db().lookup(info.vendor_name.as_str(), info.product_name.as_str())
    else {
        return (
            crate::commissioning::fallback_device_capabilities(),
            Vec::new(),
        );
    };

    let quirks = entry
        .matter
        .as_ref()
        .map(|matter| matter.quirks.clone())
        .unwrap_or_default();
    (entry.capabilities(), quirks)
}

fn fallback_initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut device_caps = HashMap::new();
    let mut device_quirks = HashMap::new();
    let commissioned_nodes = commissioned
        .iter()
        .map(|info| info.node_id)
        .collect::<HashSet<_>>();

    let mut insert_fallback = |device_id: String, info: Option<&MatterDeviceInfo>| {
        let (caps, quirks) = info.map(fallback_device_metadata).unwrap_or_else(|| {
            (
                crate::commissioning::fallback_device_capabilities(),
                Vec::new(),
            )
        });
        device_caps.entry(device_id.clone()).or_insert(caps);
        device_quirks.entry(device_id).or_insert(quirks);
    };

    for info in commissioned {
        insert_fallback(format_device_id(info.node_id, 1), Some(info));
    }

    if let Ok(state) = state.lock() {
        for device in state.canonical_registry.devices() {
            for endpoint in device.active_endpoints() {
                if &endpoint.hub_key != hub_key {
                    continue;
                }
                let Some((node_id, _)) = parse_device_id(&endpoint.native_id) else {
                    continue;
                };
                if commissioned_nodes.contains(&node_id) {
                    let info = commissioned.iter().find(|info| info.node_id == node_id);
                    insert_fallback(endpoint.native_id.clone(), info);
                }
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        device_quirks,
    }
}

fn initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    persisted_devices: &[CommissionedDevice],
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut metadata = fallback_initial_device_metadata(state, commissioned, hub_key);
    let local_overrides = crate::local_quirks::load_overrides_for_state(state);
    let aliases_by_node = state
        .lock()
        .ok()
        .map(|state| {
            state
                .canonical_registry
                .devices()
                .flat_map(|device| device.active_endpoints())
                .filter(|endpoint| &endpoint.hub_key == hub_key)
                .filter_map(|endpoint| {
                    parse_device_id(&endpoint.native_id)
                        .map(|(node_id, _)| (node_id, endpoint.native_id.clone()))
                })
                .fold(
                    HashMap::<u64, Vec<String>>::new(),
                    |mut aliases, (node_id, id)| {
                        aliases.entry(node_id).or_default().push(id);
                        aliases
                    },
                )
        })
        .unwrap_or_default();
    for device in persisted_devices {
        let device_id = format_device_id(device.node_id, device.light_endpoint);
        let mut caps = crate::commissioning::build_device_capabilities(device);
        let mut quirks = crate::commissioning::build_device_quirks(device);
        cloud_profiles.apply_to_device(device, &mut caps, &mut quirks);
        if let Some(override_caps) = local_overrides.capabilities.get(&device_id) {
            crate::local_quirks::apply_capability_override(&mut caps, override_caps);
        }
        if let Some(local_quirks) = local_overrides.quirks.get(&device_id) {
            quirks = local_quirks.clone();
        }

        metadata.device_caps.insert(device_id.clone(), caps.clone());
        metadata
            .device_quirks
            .insert(device_id.clone(), quirks.clone());
        if let Some(aliases) = aliases_by_node.get(&device.node_id) {
            for alias in aliases {
                metadata.device_caps.insert(alias.clone(), caps.clone());
                metadata.device_quirks.insert(alias.clone(), quirks.clone());
            }
        }
    }
    metadata
}

/// Format a Matter node ID as a device ID string.
pub fn format_device_id(node_id: u64, endpoint: u16) -> String {
    if endpoint == 1 {
        format!("matter-{}", node_id)
    } else {
        format!("matter-{}-{}", node_id, endpoint)
    }
}

/// Parse a device ID string back into `(node_id, endpoint)`.
pub fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
    let rest = device_id.strip_prefix("matter-")?;
    match rest.split_once('-') {
        Some((node_str, endpoint_str)) => {
            let node_id = node_str.parse::<u64>().ok()?;
            let endpoint = endpoint_str.parse::<u16>().ok()?;
            Some((node_id, endpoint))
        }
        None => {
            let node_id = rest.parse::<u64>().ok()?;
            Some((node_id, 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
    use rhythm_os::state::AppState;

    use crate::provider::matter_credentials;
    use crate::transport::{
        CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterGroup,
        MatterGroupMember, MatterSubscriptionTarget,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeMatterTransport {
        devices: Vec<MatterDeviceInfo>,
        persisted_devices: Vec<CommissionedDevice>,
        probe_calls: AtomicUsize,
        subscribe_calls: AtomicUsize,
    }

    impl FakeMatterTransport {
        fn new(devices: Vec<MatterDeviceInfo>, persisted_devices: Vec<CommissionedDevice>) -> Self {
            Self {
                devices,
                persisted_devices,
                probe_calls: AtomicUsize::new(0),
                subscribe_calls: AtomicUsize::new(0),
            }
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            _request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            anyhow::bail!("not used")
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            Ok(self.devices.clone())
        }

        fn list_commissioned_devices(&self) -> Result<Vec<CommissionedDevice>> {
            Ok(self.persisted_devices.clone())
        }

        fn probe_light(&self, _node_id: u64) -> Result<CommissionedDevice> {
            self.probe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("startup must not probe persisted Matter devices")
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

    fn shared_state(prefix: &str) -> SharedState {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rhythm-matter-lifecycle-{}-{}-{}",
            prefix,
            std::process::id(),
            id
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = dir.to_string_lossy().to_string();
        state
    }

    fn device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            reachable: true,
        }
    }

    fn h6004_device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            reachable: true,
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            vendor_id: 100,
            product_id: 200,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn h6004_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            vendor_id: 4999,
            product_id: 24580,
            serial_number: Some(format!("h6004-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }

    fn sengled_w41_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            vendor_id: 4448,
            product_id: 36866,
            serial_number: Some(format!("sengled-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    #[test]
    fn parse_simple_device_id() {
        assert_eq!(parse_device_id("matter-100"), Some((100, 1)));
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(parse_device_id("matter-42-2"), Some((42, 2)));
    }

    #[test]
    fn parse_invalid_prefix() {
        assert_eq!(parse_device_id("hue-abc123"), None);
    }

    #[test]
    fn parse_invalid_node_id() {
        assert_eq!(parse_device_id("matter-abc"), None);
    }

    #[test]
    fn roundtrip() {
        assert_eq!(parse_device_id(&format_device_id(55, 1)), Some((55, 1)));
        assert_eq!(parse_device_id(&format_device_id(55, 3)), Some((55, 3)));
    }

    #[test]
    fn next_node_id_seed_uses_max_commissioned_node() {
        let commissioned = vec![
            MatterDeviceInfo {
                node_id: 100,
                vendor_name: "A".to_string(),
                product_name: "Light".to_string(),
                reachable: true,
            },
            MatterDeviceInfo {
                node_id: 105,
                vendor_name: "B".to_string(),
                product_name: "Lamp".to_string(),
                reachable: true,
            },
        ];

        assert_eq!(next_node_id_seed(&commissioned), 106);
    }

    #[test]
    fn fallback_initial_metadata_includes_persisted_endpoint_ids() {
        let state = shared_state("fallback-endpoints");
        let key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-12-2".to_string(),
            room_id: None,
            room_name: None,
            name: "Endpoint two bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("12")],
            manufacturer: None,
            model: None,
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 100);

        let metadata = fallback_initial_device_metadata(&state, &[device_info(12)], &key);

        assert!(metadata.device_caps.contains_key("matter-12"));
        assert!(metadata.device_caps.contains_key("matter-12-2"));
    }

    #[test]
    fn fallback_initial_metadata_preserves_known_device_quirks() {
        let state = shared_state("fallback-known-quirks");
        let key = HubKey::new(HubType::new("matter"), "local");

        let metadata = fallback_initial_device_metadata(&state, &[h6004_device_info(107)], &key);

        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert!(metadata
            .device_caps
            .get("matter-107")
            .is_some_and(LightCapabilities::supports_xy_color));
    }

    #[test]
    fn fallback_initial_metadata_preserves_sengled_hue_saturation_profile() {
        let state = shared_state("fallback-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let info = MatterDeviceInfo {
            node_id: 104,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            reachable: false,
        };

        let metadata = fallback_initial_device_metadata(&state, &[info], &key);
        let caps = metadata.device_caps.get("matter-104").unwrap();

        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn persisted_metadata_applies_local_overrides_after_builtin_and_cloud_profiles() {
        let state = shared_state("persisted-overrides");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = h6004_device(107);
        crate::local_quirks::save_device_profile_override(
            &state,
            "matter-107",
            Some(vec![DeviceQuirk::NeedsXyNotCt]),
            Some(crate::local_quirks::LocalCapabilityOverride {
                min_brightness: Some(17),
                supports_transition: Some(false),
            }),
            Some("test-report".to_string()),
        )
        .unwrap();

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &key,
        );

        let caps = metadata.device_caps.get("matter-107").unwrap();
        assert_eq!(caps.min_brightness, Some(17));
        assert!(!caps.supports_transition);
        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![DeviceQuirk::NeedsXyNotCt])
        );
    }

    #[test]
    fn persisted_sengled_metadata_keeps_profile_hue_saturation_and_explicit_on() {
        let state = shared_state("persisted-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = sengled_w41_device(104);

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &key,
        );

        let caps = metadata.device_caps.get("matter-104").unwrap();
        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn connect_matter_uses_persisted_records_without_probing_or_subscribing_and_tags_events() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect");
        let key = HubKey::new(HubType::new("matter"), "local");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), matter_credentials("local", "fabric-test"));

        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(10, 2), h6004_device(107)],
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();

        assert_eq!(hub.hub_key, key);
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(data.fabric_id, "fabric-test");
        assert_eq!(data.commissioned.lock().unwrap().len(), 2);
        assert!(data.device_caps.lock().unwrap().contains_key("matter-10-2"));
        assert!(data.device_caps.lock().unwrap().contains_key("matter-107"));
        assert_eq!(
            data.next_node_id.load(std::sync::atomic::Ordering::SeqCst),
            108
        );
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&key));
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 0);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }

    #[test]
    fn connect_matter_falls_back_to_basic_list_without_probing_old_transports() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect-old-transport");
        let transport = Arc::new(FakeMatterTransport::new(
            vec![h6004_device_info(107)],
            Vec::new(),
        ));

        let (hub, _event_rx) = connect_matter(&state, transport.clone()).unwrap();
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 0);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }
}
