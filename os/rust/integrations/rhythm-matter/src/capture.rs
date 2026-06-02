//! Persist raw Matter probe metadata for later device-database work.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use serde::Serialize;

use crate::hub_state::MatterHubData;
use crate::lifecycle::format_device_id;
use crate::transport::CommissionedDevice;

#[derive(Debug, Serialize)]
struct MatterDeviceCapture {
    source: String,
    captured_at_unix_ms: u64,
    device_id: String,
    commissioned: CommissionedDevice,
    derived_capabilities: rhythm_devices::LightCapabilities,
    derived_quirks: Vec<rhythm_devices::DeviceQuirk>,
    db_match: Option<MatterDbMatchCapture>,
}

#[derive(Debug, Serialize)]
struct MatterDbMatchCapture {
    manufacturer: String,
    model: String,
    name: String,
    matter_vendor_id: Option<u16>,
    matter_product_id: Option<u16>,
    matter_quirks: Vec<rhythm_devices::DeviceQuirk>,
}

pub fn persist_device_capture(
    hub_data: &MatterHubData,
    device: &CommissionedDevice,
    source: &str,
) -> Result<Option<PathBuf>> {
    let Some(capture_dir) = hub_data.capture_dir.get() else {
        return Ok(None);
    };

    fs::create_dir_all(capture_dir)
        .with_context(|| format!("creating Matter capture dir {}", capture_dir))?;

    let device_id = format_device_id(device.node_id, device.light_endpoint);
    let db_match =
        crate::capabilities::lookup_db_entry(device, rhythm_devices::builtin_db()).map(|entry| {
            MatterDbMatchCapture {
                manufacturer: entry.manufacturer.clone(),
                model: entry.model.clone(),
                name: entry.name.clone(),
                matter_vendor_id: entry.matter.as_ref().and_then(|matter| matter.vendor_id),
                matter_product_id: entry.matter.as_ref().and_then(|matter| matter.product_id),
                matter_quirks: entry
                    .matter
                    .as_ref()
                    .map(|matter| matter.quirks.clone())
                    .unwrap_or_default(),
            }
        });

    let capture = MatterDeviceCapture {
        source: source.to_string(),
        captured_at_unix_ms: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64,
        device_id: device_id.clone(),
        commissioned: device.clone(),
        derived_capabilities: crate::commissioning::build_device_capabilities(device),
        derived_quirks: crate::commissioning::build_device_quirks(device),
        db_match,
    };

    let final_path = Path::new(capture_dir).join(format!("{}.json", device_id));
    let temp_path = final_path.with_extension("json.tmp");
    let data = serde_json::to_vec_pretty(&capture)?;

    fs::write(&temp_path, data).with_context(|| format!("writing {}", temp_path.display()))?;
    fs::rename(&temp_path, &final_path).with_context(|| {
        format!(
            "renaming {} to {}",
            temp_path.display(),
            final_path.display()
        )
    })?;

    Ok(Some(final_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{HashMap, HashSet};
    use std::sync::atomic::AtomicU64;
    use std::sync::{Arc, Mutex};

    use rhythm_os::hub::HubEvent;

    use crate::cloud_profiles::CloudMatterProfileCatalog;
    use crate::controller::MatterDeviceRegistry;
    use crate::transport::{MatterColorMode, MatterDeviceInfo};

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-matter-capture-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn hub_data() -> MatterHubData {
        let (event_tx, _event_rx) = std::sync::mpsc::channel::<HubEvent>();
        MatterHubData {
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry: Arc::new(Mutex::new(MatterDeviceRegistry::new())),
            fabric_id: "test".to_string(),
            commissioned: Mutex::new(Vec::<MatterDeviceInfo>::new()),
            next_node_id: AtomicU64::new(100),
            device_caps: Mutex::new(HashMap::new()),
            device_quirks: Mutex::new(HashMap::new()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            event_tx,
        }
    }

    fn commissioned_device() -> CommissionedDevice {
        CommissionedDevice {
            node_id: 42,
            vendor_name: "Example Vendor".to_string(),
            product_name: "Example Lamp".to_string(),
            vendor_id: 0xfff1,
            product_id: 0x0001,
            serial_number: Some("serial-42".to_string()),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
                MatterColorMode::HueSaturation,
            ],
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
        }
    }

    #[test]
    fn persist_device_capture_returns_none_when_capture_dir_is_disabled() {
        let hub_data = hub_data();

        let result = persist_device_capture(&hub_data, &commissioned_device(), "probe").unwrap();

        assert_eq!(result, None);
    }

    #[test]
    fn persist_device_capture_writes_json_payload_and_promotes_temp_file() {
        let dir = unique_test_dir("writes");
        let hub_data = hub_data();
        hub_data
            .capture_dir
            .set(dir.display().to_string())
            .expect("capture dir should be unset");

        let path = persist_device_capture(&hub_data, &commissioned_device(), "pairing")
            .unwrap()
            .expect("capture path");

        assert_eq!(path, dir.join("matter-42.json"));
        assert!(path.exists());
        assert!(!dir.join("matter-42.json.tmp").exists());

        let json: serde_json::Value =
            serde_json::from_slice(&fs::read(&path).expect("capture file")).unwrap();
        assert_eq!(json["source"], "pairing");
        assert_eq!(json["device_id"], "matter-42");
        assert_eq!(json["commissioned"]["node_id"], 42);
        assert_eq!(json["commissioned"]["vendor_name"], "Example Vendor");
        assert_eq!(json["derived_capabilities"]["light_type"], "extended_color");
        assert!(json["derived_quirks"].is_array());
        assert!(json["captured_at_unix_ms"].as_u64().is_some());

        let _ = fs::remove_dir_all(dir);
    }
}
