//! Persistent Matter commissioned-device cache.
//!
//! The native CHIP sidecar owns the live controller, but Rhythm also keeps a
//! small JSON cache of commissioned nodes so startup can list known devices
//! before probing them. Forced unpair must be able to edit that file even when
//! the sidecar cannot initialize.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::transport::CommissionedDevice;

const DEVICE_STORE_SCHEMA_VERSION: u32 = 1;
const DEVICE_STORE_FILENAME: &str = "devices.json";

#[derive(Serialize, Deserialize)]
struct DeviceStoreFile {
    schema_version: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    compressed_fabric_id: Option<String>,
    devices: Vec<CommissionedDevice>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum DeviceStoreOnDisk {
    V1(DeviceStoreFile),
    Legacy(Vec<CommissionedDevice>),
}

/// Path to the CHIP sidecar's commissioned-device cache inside a Matter data dir.
pub fn chip_device_store_path(matter_data_path: impl AsRef<Path>) -> PathBuf {
    matter_data_path
        .as_ref()
        .join("chip")
        .join(DEVICE_STORE_FILENAME)
}

/// Remove a commissioned node from the on-disk Matter device cache.
///
/// Returns `Ok(true)` when the cache existed and contained the node, `Ok(false)`
/// when there was nothing to remove. Legacy array stores are rewritten into the
/// current schema only when a removal is actually made.
pub fn remove_commissioned_node(matter_data_path: impl AsRef<Path>, node_id: u64) -> Result<bool> {
    remove_commissioned_node_from_path(chip_device_store_path(matter_data_path), node_id)
}

pub(crate) fn remove_commissioned_node_from_path(
    path: impl AsRef<Path>,
    node_id: u64,
) -> Result<bool> {
    let path = path.as_ref();
    let Some(mut store) = read_store(path)? else {
        return Ok(false);
    };

    let original_len = store.devices.len();
    store.devices.retain(|device| device.node_id != node_id);
    if store.devices.len() == original_len {
        return Ok(false);
    }

    write_store(path, &store)?;
    Ok(true)
}

fn read_store(path: &Path) -> Result<Option<DeviceStoreFile>> {
    let json = match fs::read_to_string(path) {
        Ok(json) => json,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };

    let store = match serde_json::from_str::<DeviceStoreOnDisk>(&json)
        .with_context(|| format!("decoding {}", path.display()))?
    {
        DeviceStoreOnDisk::V1(file) => {
            if file.schema_version != DEVICE_STORE_SCHEMA_VERSION {
                anyhow::bail!(
                    "Unsupported Matter device store schema version {} in {}",
                    file.schema_version,
                    path.display()
                );
            }
            file
        }
        DeviceStoreOnDisk::Legacy(devices) => DeviceStoreFile {
            schema_version: DEVICE_STORE_SCHEMA_VERSION,
            compressed_fabric_id: None,
            devices,
        },
    };

    Ok(Some(store))
}

fn write_store(path: &Path, store: &DeviceStoreFile) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let json = serde_json::to_string_pretty(store)?;
    fs::write(path, json).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MatterColorMode;

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("rhythm-matter-device-store-{name}-{nanos}"))
    }

    fn commissioned_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Acme".to_string(),
            product_name: "Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    #[test]
    fn remove_commissioned_node_migrates_legacy_store() {
        let dir = unique_test_dir("legacy-remove");
        let devices_path = dir.join("devices.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &devices_path,
            serde_json::to_string_pretty(&vec![commissioned_device(102), commissioned_device(103)])
                .unwrap(),
        )
        .unwrap();

        assert!(remove_commissioned_node_from_path(&devices_path, 102).unwrap());

        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert_eq!(persisted["schema_version"], 1);
        let devices = persisted["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0]["node_id"], 103);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_commissioned_node_preserves_compressed_fabric_id() {
        let dir = unique_test_dir("fabric-preserve");
        let devices_path = dir.join("devices.json");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &devices_path,
            serde_json::to_string_pretty(&DeviceStoreFile {
                schema_version: DEVICE_STORE_SCHEMA_VERSION,
                compressed_fabric_id: Some("399026E03C18B2D2".to_string()),
                devices: vec![commissioned_device(102)],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(remove_commissioned_node_from_path(&devices_path, 102).unwrap());

        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert_eq!(persisted["compressed_fabric_id"], "399026E03C18B2D2");
        assert_eq!(persisted["devices"].as_array().unwrap().len(), 0);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn remove_commissioned_node_noops_when_missing_or_absent() {
        let dir = unique_test_dir("missing");
        let devices_path = dir.join("devices.json");

        assert!(!remove_commissioned_node_from_path(&devices_path, 102).unwrap());

        fs::create_dir_all(&dir).unwrap();
        fs::write(
            &devices_path,
            serde_json::to_string_pretty(&DeviceStoreFile {
                schema_version: DEVICE_STORE_SCHEMA_VERSION,
                compressed_fabric_id: None,
                devices: vec![commissioned_device(103)],
            })
            .unwrap(),
        )
        .unwrap();

        assert!(!remove_commissioned_node_from_path(&devices_path, 102).unwrap());
        let persisted: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&devices_path).unwrap()).unwrap();
        assert_eq!(persisted["devices"].as_array().unwrap().len(), 1);

        let _ = fs::remove_dir_all(dir);
    }
}
