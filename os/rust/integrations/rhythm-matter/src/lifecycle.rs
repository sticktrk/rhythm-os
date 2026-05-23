//! Matter hub lifecycle — connect, disconnect, runtime creation.

use std::collections::HashMap;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use log::{info, warn};
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::{
    MatterDeviceInfo, MatterSubscriptionTarget, MatterTransport,
    DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS, DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

/// Connect to the local Matter fabric.
pub fn connect_matter(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let commissioned = transport.list_devices().unwrap_or_default();
    let cloud_profiles = crate::cloud_profiles::load_or_sync_for_state(state);
    let initial_metadata =
        load_initial_device_metadata(state, &transport, &commissioned, &cloud_profiles);
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

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let _ = event_tx.send(HubEvent::Connected {
        hub_key: Some(hub_key.clone()),
    });
    start_attribute_report_loop(
        hub_key.clone(),
        transport.clone(),
        initial_metadata.subscription_targets.clone(),
        event_tx.clone(),
    );

    rhythm_os::lifecycle::connect_hub(
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
                cloud_profiles: std::sync::Mutex::new(cloud_profiles.clone()),
                decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
                recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
                event_tx,
            }))
        },
        move |_registry, _shutdown| event_rx,
    )
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
    subscription_targets: Vec<MatterSubscriptionTarget>,
}

fn load_initial_device_metadata(
    state: &SharedState,
    transport: &Arc<dyn MatterTransport>,
    commissioned: &[MatterDeviceInfo],
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
) -> InitialDeviceMetadata {
    let mut device_caps = HashMap::new();
    let mut device_quirks = HashMap::new();
    let mut subscription_targets = Vec::new();
    let local_overrides = crate::local_quirks::load_overrides_for_state(state);

    for info in commissioned {
        match transport.probe_light(info.node_id) {
            Ok(device) => {
                let device_id = format_device_id(device.node_id, device.light_endpoint);
                subscription_targets.push(MatterSubscriptionTarget {
                    node_id: device.node_id,
                    endpoint: device.light_endpoint,
                });
                let mut caps = crate::commissioning::build_device_capabilities(&device);
                let mut quirks = crate::commissioning::build_device_quirks(&device);
                cloud_profiles.apply_to_device(&device, &mut caps, &mut quirks);
                if let Some(override_caps) = local_overrides.capabilities.get(&device_id) {
                    crate::local_quirks::apply_capability_override(&mut caps, override_caps);
                }
                device_caps.insert(device_id.clone(), caps);
                if let Some(local_quirks) = local_overrides.quirks.get(&device_id) {
                    quirks = local_quirks.clone();
                }
                device_quirks.insert(device_id, quirks);
            }
            Err(error) => {
                warn!(
                    target: "sys",
                    "Matter: failed to probe node {} during connect: {}",
                    info.node_id,
                    error
                );
                subscription_targets.push(MatterSubscriptionTarget {
                    node_id: info.node_id,
                    endpoint: 1,
                });
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        device_quirks,
        subscription_targets,
    }
}

fn start_attribute_report_loop(
    hub_key: HubKey,
    transport: Arc<dyn MatterTransport>,
    targets: Vec<MatterSubscriptionTarget>,
    event_tx: Sender<HubEvent>,
) {
    if targets.is_empty() {
        return;
    }

    let _ = std::thread::Builder::new()
        .name("matter-attr-sub".to_string())
        .spawn(move || {
            if let Err(error) = transport.subscribe_on_off(
                &targets,
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
            ) {
                warn!(
                    target: "evt",
                    "Matter: failed to start On/Off attribute subscriptions: {}",
                    error
                );
                return;
            }

            info!(
                target: "evt",
                "Matter: subscribed to On/Off reports for {} endpoint(s)",
                targets.len()
            );

            loop {
                match transport.drain_attribute_reports() {
                    Ok(reports) => {
                        for report in reports {
                            if let Some(event) = crate::events::translate_report(&report) {
                                if event_tx.send(event.with_hub_key(hub_key.clone())).is_err() {
                                    return;
                                }
                            }
                        }
                    }
                    Err(error) => {
                        warn!(
                            target: "evt",
                            "Matter: failed to drain attribute reports: {}",
                            error
                        );
                        std::thread::sleep(Duration::from_secs(2));
                        continue;
                    }
                }

                std::thread::sleep(Duration::from_millis(250));
            }
        });
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
}
