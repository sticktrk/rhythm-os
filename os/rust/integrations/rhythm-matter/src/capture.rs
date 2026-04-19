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
