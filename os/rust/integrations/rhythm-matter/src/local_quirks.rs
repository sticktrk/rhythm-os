//! Local Matter quirk overrides discovered by the bulb tester.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalQuirkStore {
    schema_version: u8,
    updated_at_unix_ms: u64,
    #[serde(default)]
    devices: BTreeMap<String, LocalQuirkOverride>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct LocalQuirkOverride {
    #[serde(default)]
    quirks: Vec<DeviceQuirk>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    capabilities: Option<LocalCapabilityOverride>,
    source: String,
    updated_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    report_id: Option<String>,
    /// Additive typed profile written by Bulb Audition. Legacy binaries ignore
    /// this field and continue to consume `quirks` and `capabilities`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    control_profile: Option<crate::control_profile::MatterControlProfile>,
    /// Runtime reconciliation signal retained across appliance restarts.
    #[serde(default, skip_serializing_if = "is_false")]
    needs_audition: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LocalCapabilityOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supports_transition: Option<bool>,
}

#[derive(Debug, Clone, Default)]
pub struct LocalMatterOverrides {
    pub quirks: HashMap<String, Vec<DeviceQuirk>>,
    pub capabilities: HashMap<String, LocalCapabilityOverride>,
    pub control_profiles: HashMap<String, crate::control_profile::MatterControlProfile>,
    pub needs_audition: HashSet<String>,
}

impl Default for LocalQuirkStore {
    fn default() -> Self {
        Self {
            schema_version: 1,
            updated_at_unix_ms: now_unix_ms(),
            devices: BTreeMap::new(),
        }
    }
}

fn is_false(value: &bool) -> bool {
    !*value
}

pub fn quirks_from_value(value: &serde_json::Value) -> Result<Vec<DeviceQuirk>> {
    serde_json::from_value(value.clone()).context("invalid quirk list")
}

pub fn quirks_to_value(quirks: &[DeviceQuirk]) -> serde_json::Value {
    serde_json::to_value(quirks).unwrap_or_else(|_| serde_json::Value::Array(Vec::new()))
}

pub fn capability_override_from_value(
    value: &serde_json::Value,
) -> Result<LocalCapabilityOverride> {
    let mut override_caps: LocalCapabilityOverride =
        serde_json::from_value(value.clone()).context("invalid capability hints")?;
    override_caps.normalize();
    Ok(override_caps)
}

pub fn capabilities_to_value(override_caps: &LocalCapabilityOverride) -> serde_json::Value {
    serde_json::to_value(override_caps)
        .unwrap_or_else(|_| serde_json::Value::Object(Default::default()))
}

pub fn apply_capability_override(
    caps: &mut LightCapabilities,
    override_caps: &LocalCapabilityOverride,
) {
    if let Some(min_brightness) = override_caps.min_brightness {
        caps.min_brightness = Some(min_brightness);
    }
    if let Some(supports_transition) = override_caps.supports_transition {
        caps.supports_transition = supports_transition;
    }
}

/// Applies a saved tester quirk set without allowing an older test result to
/// contradict an explicit built-in or cloud color-command preference.
///
/// Tester results remain authoritative for unprofiled devices and for all
/// non-color quirks. This lets newly curated device knowledge repair stale
/// local HS/XY choices after an upgrade without discarding measured command
/// spacing or other local behavior.
pub fn apply_quirk_override(
    curated_quirks: &[DeviceQuirk],
    local_quirks: &[DeviceQuirk],
) -> Vec<DeviceQuirk> {
    let Some(curated_color_preference) = color_preference_quirk(curated_quirks) else {
        return local_quirks.to_vec();
    };

    let mut effective_quirks = local_quirks
        .iter()
        .filter(|quirk| !is_color_preference_quirk(quirk))
        .cloned()
        .collect::<Vec<_>>();
    effective_quirks.push(curated_color_preference);
    effective_quirks
}

fn color_preference_quirk(quirks: &[DeviceQuirk]) -> Option<DeviceQuirk> {
    quirks
        .iter()
        .find(|quirk| {
            matches!(
                quirk,
                DeviceQuirk::Other(value)
                    if value == rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK
            )
        })
        .or_else(|| {
            quirks
                .iter()
                .find(|quirk| matches!(quirk, DeviceQuirk::NeedsHueSaturationNotCt))
        })
        .or_else(|| {
            quirks
                .iter()
                .find(|quirk| matches!(quirk, DeviceQuirk::NeedsXyNotCt))
        })
        .cloned()
}

fn is_color_preference_quirk(quirk: &DeviceQuirk) -> bool {
    match quirk {
        DeviceQuirk::NeedsHueSaturationNotCt | DeviceQuirk::NeedsXyNotCt => true,
        DeviceQuirk::Other(value) => {
            value == rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK
        }
        _ => false,
    }
}

pub fn load_for_state(state: &SharedState) -> HashMap<String, Vec<DeviceQuirk>> {
    load_overrides_for_state(state).quirks
}

pub fn load_overrides_for_state(state: &SharedState) -> LocalMatterOverrides {
    let Some(path) = store_path(state) else {
        return LocalMatterOverrides::default();
    };
    match load_store(&path) {
        Ok(store) => {
            let mut overrides = LocalMatterOverrides::default();
            for (device_id, entry) in store.devices {
                if !entry.quirks.is_empty() {
                    overrides.quirks.insert(device_id.clone(), entry.quirks);
                }
                if let Some(mut capabilities) = entry.capabilities {
                    capabilities.normalize();
                    if !capabilities.is_empty() {
                        overrides
                            .capabilities
                            .insert(device_id.clone(), capabilities);
                    }
                }
                if let Some(profile) = entry.control_profile {
                    overrides
                        .control_profiles
                        .insert(device_id.clone(), profile);
                }
                if entry.needs_audition {
                    overrides.needs_audition.insert(device_id);
                }
            }
            overrides
        }
        Err(error) => {
            log::warn!(
                target: "sys",
                "Matter: failed to load local quirk overrides from {}: {}",
                path.display(),
                error
            );
            LocalMatterOverrides::default()
        }
    }
}

pub fn save_device_override(
    state: &SharedState,
    device_id: &str,
    quirks: Vec<DeviceQuirk>,
    report_id: Option<String>,
) -> Result<()> {
    save_device_profile_override(state, device_id, Some(quirks), None, report_id)
}

pub fn save_device_profile_override(
    state: &SharedState,
    device_id: &str,
    quirks: Option<Vec<DeviceQuirk>>,
    capabilities: Option<LocalCapabilityOverride>,
    report_id: Option<String>,
) -> Result<()> {
    let path = store_path(state).context("data_dir not configured on AppState")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating local quirk dir {}", parent.display()))?;
    }

    let mut store = load_store(&path).unwrap_or_default();
    let now = now_unix_ms();
    let mut entry = store
        .devices
        .remove(device_id)
        .unwrap_or_else(|| LocalQuirkOverride {
            quirks: Vec::new(),
            capabilities: None,
            source: "matter_bulb_tester".to_string(),
            updated_at_unix_ms: now,
            report_id: None,
            control_profile: None,
            needs_audition: false,
        });
    if let Some(quirks) = quirks {
        entry.quirks = quirks;
    }
    if let Some(mut capabilities) = capabilities {
        capabilities.normalize();
        if !capabilities.is_empty() {
            entry.capabilities = Some(capabilities);
        }
    }
    entry.source = "matter_bulb_tester".to_string();
    entry.updated_at_unix_ms = now;
    entry.report_id = report_id;

    store.updated_at_unix_ms = now;
    store.devices.insert(device_id.to_string(), entry);

    let body = serde_json::to_vec_pretty(&store).context("serializing local quirk store")?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

/// Persist the typed profile without removing the legacy representation. The
/// compatibility fields remain writable by the previous field floor, while a
/// later current binary can recover this exact audition strategy.
pub fn save_device_control_profile(
    state: &SharedState,
    device_id: &str,
    profile: crate::control_profile::MatterControlProfile,
    report_id: Option<String>,
) -> Result<()> {
    let path = store_path(state).context("data_dir not configured on AppState")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating local quirk dir {}", parent.display()))?;
    }

    let mut store = load_store(&path).unwrap_or_default();
    let now = now_unix_ms();
    let mut entry = store
        .devices
        .remove(device_id)
        .unwrap_or_else(|| LocalQuirkOverride {
            quirks: Vec::new(),
            capabilities: None,
            source: "bulb_audition".to_string(),
            updated_at_unix_ms: now,
            report_id: None,
            control_profile: None,
            needs_audition: false,
        });
    entry.control_profile = Some(profile);
    entry.source = "bulb_audition".to_string();
    entry.updated_at_unix_ms = now;
    entry.report_id = report_id;
    store.updated_at_unix_ms = now;
    store.devices.insert(device_id.to_string(), entry);

    let body = serde_json::to_vec_pretty(&store).context("serializing local profile store")?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

pub(crate) fn save_needs_audition_at_path(
    path: &Path,
    device_id: &str,
    needs_audition: bool,
) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("creating local quirk dir {}", parent.display()))?;
    }

    let mut store = load_store(path).unwrap_or_default();
    let now = now_unix_ms();
    let existing = store.devices.remove(device_id);
    if existing.is_none() && !needs_audition {
        return Ok(());
    }
    let mut entry = existing.unwrap_or_else(|| LocalQuirkOverride {
        quirks: Vec::new(),
        capabilities: None,
        source: "runtime_readback".to_string(),
        updated_at_unix_ms: now,
        report_id: None,
        control_profile: None,
        needs_audition: false,
    });
    entry.needs_audition = needs_audition;
    entry.updated_at_unix_ms = now;
    store.updated_at_unix_ms = now;
    store.devices.insert(device_id.to_string(), entry);

    let body = serde_json::to_vec_pretty(&store).context("serializing local audition state")?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

impl LocalCapabilityOverride {
    pub fn is_empty(&self) -> bool {
        self.min_brightness.is_none() && self.supports_transition.is_none()
    }

    fn normalize(&mut self) {
        if let Some(min_brightness) = self.min_brightness {
            self.min_brightness = Some(min_brightness.clamp(1, 100));
        }
    }
}

pub(crate) fn store_path(state: &SharedState) -> Option<PathBuf> {
    let data_dir = state.lock().ok()?.data_dir.clone();
    if data_dir.is_empty() {
        return None;
    }
    Some(
        Path::new(&data_dir)
            .join("matter")
            .join("local_quirks.json"),
    )
}

fn load_store(path: &Path) -> Result<LocalQuirkStore> {
    match fs::read_to_string(path) {
        Ok(body) => serde_json::from_str(&body).context("parsing local quirk store"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(LocalQuirkStore::default())
        }
        Err(error) => Err(error).with_context(|| format!("reading {}", path.display())),
    }
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    use rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK;
    use rhythm_devices::LightType;
    use rhythm_os::state::AppState;
    use serde_json::json;

    #[test]
    fn capability_override_clamps_and_applies() {
        let override_caps = capability_override_from_value(&json!({
            "min_brightness": 250,
            "supports_transition": false,
        }))
        .unwrap();

        assert_eq!(override_caps.min_brightness, Some(100));
        assert_eq!(override_caps.supports_transition, Some(false));

        let mut caps = LightCapabilities::defaults_for(LightType::Dimmable);
        apply_capability_override(&mut caps, &override_caps);

        assert_eq!(caps.min_brightness, Some(100));
        assert!(!caps.supports_transition);
    }

    #[test]
    fn curated_color_preference_replaces_stale_local_preference() {
        let effective = apply_quirk_override(
            &[DeviceQuirk::Other(
                PREFER_COLOR_TEMPERATURE_QUIRK.to_string(),
            )],
            &[
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ],
        );

        assert_eq!(
            effective,
            vec![
                DeviceQuirk::CommandThrottleMs(250),
                DeviceQuirk::Other(PREFER_COLOR_TEMPERATURE_QUIRK.to_string()),
            ]
        );
    }

    #[test]
    fn unprofiled_device_keeps_local_color_preference() {
        assert_eq!(
            apply_quirk_override(&[], &[DeviceQuirk::NeedsHueSaturationNotCt]),
            vec![DeviceQuirk::NeedsHueSaturationNotCt]
        );
    }

    #[test]
    fn curated_ct_keeps_controller_priority_over_other_curated_color_quirks() {
        assert_eq!(
            apply_quirk_override(
                &[
                    DeviceQuirk::NeedsHueSaturationNotCt,
                    DeviceQuirk::Other(PREFER_COLOR_TEMPERATURE_QUIRK.to_string()),
                ],
                &[DeviceQuirk::NeedsXyNotCt],
            ),
            vec![DeviceQuirk::Other(
                PREFER_COLOR_TEMPERATURE_QUIRK.to_string()
            )]
        );
    }

    #[test]
    fn load_overrides_for_state_round_trips_needs_audition() {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-matter-needs-audition-{}",
            std::process::id()
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = dir.to_string_lossy().to_string();
        let path = store_path(&state).unwrap();

        save_needs_audition_at_path(&path, "matter-42", true).unwrap();
        assert!(load_overrides_for_state(&state)
            .needs_audition
            .contains("matter-42"));

        save_needs_audition_at_path(&path, "matter-42", false).unwrap();
        assert!(!load_overrides_for_state(&state)
            .needs_audition
            .contains("matter-42"));
    }
}
