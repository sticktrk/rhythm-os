//! Persist raw Matter probe metadata for later device-database work.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use log::{debug, warn};
use serde::Serialize;

use crate::hub_state::MatterHubData;
use crate::lifecycle::format_device_id;
use crate::transport::CommissionedDevice;

/// Maximum number of capture files retained in the captures directory.
/// Older captures are pruned after each successful write.
const MATTER_CAPTURE_RETAIN: usize = 20;

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

    prune_matter_captures(Path::new(capture_dir), MATTER_CAPTURE_RETAIN);

    Ok(Some(final_path))
}

/// Returns `true` if `file_name` matches the capture filename pattern used by
/// `persist_device_capture` (`matter-<node>[-<endpoint>].json`).
fn is_capture_file_name(file_name: &str) -> bool {
    file_name.starts_with("matter-") && file_name.ends_with(".json")
}

/// Keep only the newest `retain` capture files in `dir`, deleting older ones.
///
/// Ordering is by modification time (newest first), falling back to filename
/// ordering when mtimes are equal or unavailable. Files that don't match the
/// capture filename pattern are left alone. Individual delete errors are
/// logged and otherwise ignored.
pub fn prune_matter_captures(dir: &Path, retain: usize) {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) => {
            debug!(
                "Matter capture prune: cannot read {}: {}",
                dir.display(),
                error
            );
            return;
        }
    };

    let mut captures = Vec::<(SystemTime, PathBuf)>::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !path.is_file() || !is_capture_file_name(file_name) {
            continue;
        }
        // Unreadable mtimes collapse to UNIX_EPOCH (oldest), so the filename
        // tie-break below decides their order deterministically.
        let modified = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .unwrap_or(UNIX_EPOCH);
        captures.push((modified, path));
    }

    if captures.len() <= retain {
        return;
    }

    // Newest first by mtime, then by filename (descending) on ties.
    captures.sort_by(|left, right| right.0.cmp(&left.0).then_with(|| right.1.cmp(&left.1)));

    for (_, path) in captures.into_iter().skip(retain) {
        if let Err(error) = fs::remove_file(&path) {
            warn!(
                "Matter capture prune: failed to delete {}: {}",
                path.display(),
                error
            );
        }
    }
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
            fallback_caps: Mutex::new(HashSet::new()),
            device_quirks: Mutex::new(HashMap::new()),
            device_profiles: Mutex::new(HashMap::new()),
            pending_turn_on_plans: Arc::new(Mutex::new(HashMap::new())),
            needs_audition: Arc::new(Mutex::new(HashSet::new())),
            readback: Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            local_overrides: Mutex::new(crate::local_quirks::LocalMatterOverrides::default()),
            cloud_profiles: Mutex::new(CloudMatterProfileCatalog::default()),
            decommissioning: Mutex::new(HashSet::new()),
            recently_decommissioned: Mutex::new(HashMap::new()),
            node_proof_of_life: Arc::new(Mutex::new(HashMap::new())),
            on_off_observations: Arc::new(Mutex::new(HashMap::new())),
            attribute_report_history: Arc::new(Mutex::new(std::collections::VecDeque::new())),
            last_turn_on_dispatch: Mutex::new(HashMap::new()),
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

    /// Write a file and pin its mtime to a fixed epoch plus `mtime_offset_secs`,
    /// so prune ordering is deterministic regardless of write timing.
    fn write_file_with_mtime(dir: &Path, name: &str, mtime_offset_secs: u64) {
        let path = dir.join(name);
        fs::write(&path, b"{}").unwrap();
        let mtime = UNIX_EPOCH + std::time::Duration::from_secs(1_700_000_000 + mtime_offset_secs);
        fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(mtime)
            .unwrap();
    }

    fn remaining_file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn prune_matter_captures_keeps_all_files_under_retain() {
        let dir = unique_test_dir("prune-under");
        for i in 0..3 {
            write_file_with_mtime(&dir, &format!("matter-{}.json", i), i);
        }

        prune_matter_captures(&dir, 5);

        assert_eq!(
            remaining_file_names(&dir),
            vec!["matter-0.json", "matter-1.json", "matter-2.json"]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_matter_captures_keeps_only_newest_by_mtime() {
        let dir = unique_test_dir("prune-newest");
        // Mtime order deliberately disagrees with filename order: the two
        // newest files by mtime are matter-3 and matter-1.
        write_file_with_mtime(&dir, "matter-1.json", 40);
        write_file_with_mtime(&dir, "matter-2.json", 10);
        write_file_with_mtime(&dir, "matter-3.json", 50);
        write_file_with_mtime(&dir, "matter-4.json", 20);
        write_file_with_mtime(&dir, "matter-5.json", 30);

        prune_matter_captures(&dir, 2);

        assert_eq!(
            remaining_file_names(&dir),
            vec!["matter-1.json", "matter-3.json"]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_matter_captures_falls_back_to_filename_order_on_equal_mtimes() {
        let dir = unique_test_dir("prune-tiebreak");
        for i in 1..=4 {
            // Identical mtimes: filename ordering (descending) breaks the tie.
            write_file_with_mtime(&dir, &format!("matter-{}.json", i), 0);
        }

        prune_matter_captures(&dir, 2);

        assert_eq!(
            remaining_file_names(&dir),
            vec!["matter-3.json", "matter-4.json"]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_matter_captures_leaves_non_capture_files_alone() {
        let dir = unique_test_dir("prune-other");
        for i in 0..4 {
            write_file_with_mtime(&dir, &format!("matter-{}.json", i), i);
        }
        // Files that don't match the writer's `matter-*.json` pattern.
        write_file_with_mtime(&dir, "notes.txt", 0);
        write_file_with_mtime(&dir, "other.json", 0);
        write_file_with_mtime(&dir, "matter-9.json.tmp", 0);

        prune_matter_captures(&dir, 2);

        assert_eq!(
            remaining_file_names(&dir),
            vec![
                "matter-2.json",
                "matter-3.json",
                "matter-9.json.tmp",
                "notes.txt",
                "other.json"
            ]
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn persist_device_capture_prunes_old_captures_after_write() {
        let dir = unique_test_dir("prune-on-write");
        // Pre-seed more stale captures than the retain limit, all older than
        // the file the writer is about to create.
        for i in 0..(MATTER_CAPTURE_RETAIN + 5) {
            write_file_with_mtime(&dir, &format!("matter-{}.json", 1000 + i), i as u64);
        }

        let hub_data = hub_data();
        hub_data
            .capture_dir
            .set(dir.display().to_string())
            .expect("capture dir should be unset");
        persist_device_capture(&hub_data, &commissioned_device(), "pairing")
            .unwrap()
            .expect("capture path");

        let remaining = remaining_file_names(&dir);
        assert_eq!(remaining.len(), MATTER_CAPTURE_RETAIN);
        assert!(
            remaining.contains(&"matter-42.json".to_string()),
            "freshly written capture must survive the prune"
        );

        let _ = fs::remove_dir_all(dir);
    }
}
