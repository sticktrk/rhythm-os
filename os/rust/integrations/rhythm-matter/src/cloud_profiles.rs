//! Cloud-synced Matter device profile overlays.
//!
//! Supabase serves only manually approved profiles. Hubs cache the feed locally
//! and merge it between the built-in `rhythm-devices` database and per-device
//! tester overrides.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result};
use log::{info, warn};
use rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK;
use rhythm_devices::{ColorMode, DeviceQuirk, LightCapabilities, LightType};
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};

use crate::transport::CommissionedDevice;

const DEFAULT_PROFILE_FEED_URL: &str =
    "https://sskwvfcnrnkewqucepbr.supabase.co/functions/v1/matter-device-profiles";
const PROFILE_FEED_URL_ENV: &str = "RHYTHM_MATTER_DEVICE_PROFILES_URL";
const PROFILE_SYNC_DISABLE_ENV: &str = "RHYTHM_MATTER_PROFILE_SYNC";
const PROFILE_SYNC_TIMEOUT_SECS: u64 = 6;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudMatterProfileCatalog {
    #[serde(default)]
    pub schema_version: u32,
    #[serde(default)]
    pub profile_version: u64,
    #[serde(default)]
    pub generated_at: Option<String>,
    #[serde(default)]
    pub profiles: Vec<CloudMatterDeviceProfile>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudMatterDeviceProfile {
    pub profile_key: String,
    #[serde(default)]
    pub profile_version: u64,
    #[serde(rename = "match", default)]
    pub match_data: CloudMatterProfileMatch,
    #[serde(default)]
    pub capabilities: CloudMatterProfileCapabilities,
    #[serde(default)]
    pub quirks: CloudMatterProfileQuirks,
    #[serde(default)]
    pub recommended_control_strategy: serde_json::Value,
    #[serde(default)]
    pub evidence: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudMatterProfileMatch {
    #[serde(default)]
    pub manufacturer: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub matter_vendor_id: Option<u16>,
    #[serde(default)]
    pub matter_product_id: Option<u16>,
    #[serde(default)]
    pub firmware_version: Option<String>,
    #[serde(default)]
    pub cluster_fingerprint: serde_json::Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudMatterProfileCapabilities {
    #[serde(default)]
    pub preferred_color_command: Option<String>,
    #[serde(default)]
    pub preferred_level_command: Option<String>,
    #[serde(default)]
    pub min_brightness: Option<u8>,
    #[serde(default)]
    pub supports_transition: Option<bool>,
    #[serde(default)]
    pub usable_min_kelvin: Option<u16>,
    #[serde(default)]
    pub usable_max_kelvin: Option<u16>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudMatterProfileQuirks {
    #[serde(default)]
    pub runtime_quirks: Vec<serde_json::Value>,
    #[serde(default)]
    pub behavioral_quirks: Vec<serde_json::Value>,
    #[serde(default)]
    pub needs_explicit_on: Option<bool>,
    #[serde(default)]
    pub needs_xy_not_ct: Option<bool>,
    #[serde(default)]
    pub needs_hue_saturation_not_ct: Option<bool>,
    #[serde(default)]
    pub xy_color_commands_ack_but_no_visible_change: Option<bool>,
    #[serde(default)]
    pub recommended_command_spacing_ms: Option<u32>,
    #[serde(default)]
    pub on_restores_previous_level: Option<bool>,
    #[serde(default)]
    pub power_on_behavior: Option<String>,
}

impl CloudMatterProfileCatalog {
    pub fn apply_to_device(
        &self,
        device: &CommissionedDevice,
        caps: &mut LightCapabilities,
        quirks: &mut Vec<DeviceQuirk>,
    ) {
        let Some(profile) = self.profiles.iter().find(|profile| profile.matches(device)) else {
            return;
        };

        profile.apply(caps, quirks);
        info!(
            target: "sys",
            "Matter: applied cloud profile {} v{} to {} ({}/{})",
            profile.profile_key,
            profile.profile_version,
            device.node_id,
            device.vendor_id,
            device.product_id
        );
    }
}

impl CloudMatterDeviceProfile {
    fn matches(&self, device: &CommissionedDevice) -> bool {
        if let (Some(vendor_id), Some(product_id)) = (
            self.match_data.matter_vendor_id,
            self.match_data.matter_product_id,
        ) {
            return vendor_id == device.vendor_id && product_id == device.product_id;
        }

        let manufacturer_matches = self
            .match_data
            .manufacturer
            .as_deref()
            .is_some_and(|manufacturer| normalized_eq(manufacturer, &device.vendor_name));
        let model_matches = self
            .match_data
            .model
            .as_deref()
            .is_some_and(|model| normalized_eq(model, &device.product_name));
        manufacturer_matches && model_matches
    }

    fn apply(&self, caps: &mut LightCapabilities, quirks: &mut Vec<DeviceQuirk>) {
        if let Some(min_brightness) = self.capabilities.min_brightness {
            caps.min_brightness = Some(min_brightness.clamp(1, 100));
        }
        if let Some(supports_transition) = self.capabilities.supports_transition {
            caps.supports_transition = supports_transition;
        }
        if let Some(min_kelvin) = self.capabilities.usable_min_kelvin {
            caps.min_kelvin = Some(min_kelvin);
        }
        if let Some(max_kelvin) = self.capabilities.usable_max_kelvin {
            caps.max_kelvin = Some(max_kelvin);
        }

        if self.quirks.xy_color_commands_ack_but_no_visible_change == Some(true) {
            caps.color_modes.retain(|mode| *mode != ColorMode::Xy);
            recalculate_light_type(caps);
            push_unique_quirk(
                quirks,
                DeviceQuirk::Other("xy_color_ack_but_no_visible_change".to_string()),
            );
        }

        if self.quirks.needs_explicit_on == Some(true) {
            push_unique_quirk(quirks, DeviceQuirk::NeedsExplicitOn);
        }
        if self.quirks.needs_xy_not_ct == Some(true)
            || self.capabilities.preferred_color_command.as_deref() == Some("xy")
        {
            push_unique_quirk(quirks, DeviceQuirk::NeedsXyNotCt);
        }
        if self.quirks.needs_hue_saturation_not_ct == Some(true)
            || self.capabilities.preferred_color_command.as_deref() == Some("hue_saturation")
        {
            push_unique_quirk(quirks, DeviceQuirk::NeedsHueSaturationNotCt);
        }
        if self.capabilities.preferred_color_command.as_deref() == Some("color_temperature") {
            push_unique_quirk(
                quirks,
                DeviceQuirk::Other(PREFER_COLOR_TEMPERATURE_QUIRK.to_string()),
            );
        }
        if let Some(ms) = self
            .quirks
            .recommended_command_spacing_ms
            .filter(|ms| *ms > 0)
        {
            push_unique_quirk(quirks, DeviceQuirk::CommandThrottleMs(ms));
        }

        for value in &self.quirks.runtime_quirks {
            if let Some(quirk) = device_quirk_from_value(value) {
                push_unique_quirk(quirks, quirk);
            }
        }
    }
}

pub fn load_or_sync_for_state(state: &SharedState) -> CloudMatterProfileCatalog {
    if profile_sync_disabled() {
        return load_cached_for_state(state).unwrap_or_default();
    }

    match fetch_profile_catalog() {
        Ok(catalog) => {
            if let Err(error) = save_cached_for_state(state, &catalog) {
                warn!(
                    target: "sys",
                    "Matter: failed to cache cloud device profiles: {}",
                    error
                );
            }
            info!(
                target: "sys",
                "Matter: synced {} approved cloud device profile(s), feed version {}",
                catalog.profiles.len(),
                catalog.profile_version
            );
            catalog
        }
        Err(error) => {
            warn!(
                target: "sys",
                "Matter: cloud device profile sync failed: {}; using cached profiles",
                error
            );
            load_cached_for_state(state).unwrap_or_default()
        }
    }
}

pub fn load_cached_for_state(state: &SharedState) -> Result<CloudMatterProfileCatalog> {
    let path = cache_path(state).context("data_dir not configured on AppState")?;
    let body = fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    serde_json::from_str(&body).with_context(|| format!("parsing {}", path.display()))
}

fn save_cached_for_state(state: &SharedState, catalog: &CloudMatterProfileCatalog) -> Result<()> {
    let path = cache_path(state).context("data_dir not configured on AppState")?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    }
    let body = serde_json::to_vec_pretty(catalog).context("serializing cloud profiles")?;
    let tmp_path = path.with_extension("json.tmp");
    fs::write(&tmp_path, body).with_context(|| format!("writing {}", tmp_path.display()))?;
    fs::rename(&tmp_path, &path)
        .with_context(|| format!("renaming {} to {}", tmp_path.display(), path.display()))?;
    Ok(())
}

fn fetch_profile_catalog() -> Result<CloudMatterProfileCatalog> {
    let url = std::env::var(PROFILE_FEED_URL_ENV)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_PROFILE_FEED_URL.to_string());
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(PROFILE_SYNC_TIMEOUT_SECS))
        .build()
        .context("building Matter cloud profile HTTP client")?;
    client
        .get(&url)
        .send()
        .with_context(|| format!("fetching Matter cloud profiles from {}", url))?
        .error_for_status()
        .with_context(|| format!("Matter cloud profile feed returned error from {}", url))?
        .json()
        .context("decoding Matter cloud profile feed")
}

fn cache_path(state: &SharedState) -> Option<PathBuf> {
    let data_dir = state.lock().ok()?.data_dir.clone();
    if data_dir.is_empty() {
        return None;
    }
    Some(
        Path::new(&data_dir)
            .join("matter")
            .join("cloud_profiles.json"),
    )
}

fn profile_sync_disabled() -> bool {
    matches!(
        std::env::var(PROFILE_SYNC_DISABLE_ENV)
            .ok()
            .as_deref()
            .map(str::trim),
        Some("0" | "false" | "off" | "disabled")
    )
}

fn normalized_eq(lhs: &str, rhs: &str) -> bool {
    normalize(lhs) == normalize(rhs)
}

fn normalize(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn push_unique_quirk(quirks: &mut Vec<DeviceQuirk>, quirk: DeviceQuirk) {
    if !quirks.contains(&quirk) {
        quirks.push(quirk);
    }
}

fn device_quirk_from_value(value: &serde_json::Value) -> Option<DeviceQuirk> {
    match value {
        serde_json::Value::String(value) if value == "needs_explicit_on" => {
            Some(DeviceQuirk::NeedsExplicitOn)
        }
        serde_json::Value::String(value) if value == "needs_xy_not_ct" => {
            Some(DeviceQuirk::NeedsXyNotCt)
        }
        serde_json::Value::String(value) if value == "needs_hue_saturation_not_ct" => {
            Some(DeviceQuirk::NeedsHueSaturationNotCt)
        }
        serde_json::Value::String(value) if value == PREFER_COLOR_TEMPERATURE_QUIRK => Some(
            DeviceQuirk::Other(PREFER_COLOR_TEMPERATURE_QUIRK.to_string()),
        ),
        serde_json::Value::Object(map) => map
            .get("command_throttle_ms")
            .and_then(serde_json::Value::as_u64)
            .and_then(|ms| u32::try_from(ms).ok())
            .map(DeviceQuirk::CommandThrottleMs),
        _ => None,
    }
}

fn recalculate_light_type(caps: &mut LightCapabilities) {
    caps.light_type = if caps.color_modes.contains(&ColorMode::HueSaturation)
        || caps.color_modes.contains(&ColorMode::Xy)
    {
        LightType::ExtendedColor
    } else if caps.color_modes.contains(&ColorMode::ColorTemperature) {
        LightType::ColorTemperature
    } else if caps.color_modes.contains(&ColorMode::Dimmable) {
        LightType::Dimmable
    } else {
        LightType::OnOff
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::{SystemTime, UNIX_EPOCH};

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn device() -> CommissionedDevice {
        CommissionedDevice {
            node_id: 100,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![
                crate::transport::MatterColorMode::HueSaturation,
                crate::transport::MatterColorMode::Xy,
                crate::transport::MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }

    fn unique_test_dir(name: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("rhythm-cloud-profiles-{}-{}", name, nanos));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn applies_matching_cloud_profile() {
        let catalog = CloudMatterProfileCatalog {
            profiles: vec![CloudMatterDeviceProfile {
                profile_key: "matter:1:2".to_string(),
                match_data: CloudMatterProfileMatch {
                    matter_vendor_id: Some(1),
                    matter_product_id: Some(2),
                    ..Default::default()
                },
                capabilities: CloudMatterProfileCapabilities {
                    supports_transition: Some(false),
                    min_brightness: Some(8),
                    preferred_color_command: Some("color_temperature".to_string()),
                    ..Default::default()
                },
                quirks: CloudMatterProfileQuirks {
                    xy_color_commands_ack_but_no_visible_change: Some(true),
                    recommended_command_spacing_ms: Some(250),
                    ..Default::default()
                },
                ..Default::default()
            }],
            ..Default::default()
        };
        let mut caps = crate::capabilities::capabilities_from_commissioned(&device());
        let mut quirks = Vec::new();

        catalog.apply_to_device(&device(), &mut caps, &mut quirks);

        assert!(!caps.supports_transition);
        assert_eq!(caps.min_brightness, Some(8));
        assert!(!caps.color_modes.contains(&ColorMode::Xy));
        assert!(quirks.contains(&DeviceQuirk::CommandThrottleMs(250)));
        assert!(quirks.contains(&DeviceQuirk::Other(
            PREFER_COLOR_TEMPERATURE_QUIRK.to_string()
        )));
    }

    #[test]
    fn profile_matches_normalized_manufacturer_model_and_dedupes_quirks() {
        let profile = CloudMatterDeviceProfile {
            profile_key: "normalized".to_string(),
            match_data: CloudMatterProfileMatch {
                manufacturer: Some(" shenzhen-qianyan technology ".to_string()),
                model: Some("h 6004".to_string()),
                ..Default::default()
            },
            capabilities: CloudMatterProfileCapabilities {
                min_brightness: Some(0),
                supports_transition: Some(true),
                usable_min_kelvin: Some(2300),
                usable_max_kelvin: Some(6400),
                preferred_color_command: Some("xy".to_string()),
                ..Default::default()
            },
            quirks: CloudMatterProfileQuirks {
                needs_explicit_on: Some(true),
                needs_xy_not_ct: Some(true),
                needs_hue_saturation_not_ct: Some(true),
                recommended_command_spacing_ms: Some(0),
                runtime_quirks: vec![
                    serde_json::json!("needs_explicit_on"),
                    serde_json::json!("needs_xy_not_ct"),
                    serde_json::json!("needs_hue_saturation_not_ct"),
                    serde_json::json!({"command_throttle_ms": 125}),
                    serde_json::json!({"command_throttle_ms": 9_999_999_999_u64}),
                    serde_json::json!("unknown"),
                ],
                ..Default::default()
            },
            ..Default::default()
        };
        assert!(profile.matches(&device()));

        let catalog = CloudMatterProfileCatalog {
            profiles: vec![profile],
            ..Default::default()
        };
        let mut caps = crate::capabilities::capabilities_from_commissioned(&device());
        let mut quirks = vec![DeviceQuirk::NeedsExplicitOn];

        catalog.apply_to_device(&device(), &mut caps, &mut quirks);

        assert_eq!(caps.min_brightness, Some(1));
        assert!(caps.supports_transition);
        assert_eq!(caps.min_kelvin, Some(2300));
        assert_eq!(caps.max_kelvin, Some(6400));
        assert_eq!(
            quirks
                .iter()
                .filter(|quirk| **quirk == DeviceQuirk::NeedsExplicitOn)
                .count(),
            1
        );
        assert!(quirks.contains(&DeviceQuirk::NeedsXyNotCt));
        assert!(quirks.contains(&DeviceQuirk::NeedsHueSaturationNotCt));
        assert!(quirks.contains(&DeviceQuirk::CommandThrottleMs(125)));
    }

    #[test]
    fn cache_path_save_load_and_sync_disabled_env_are_deterministic() {
        let _guard = ENV_LOCK.lock().unwrap();
        let state = Arc::new(Mutex::new(rhythm_os::state::AppState::default()));
        assert!(cache_path(&state).is_none());
        assert!(load_cached_for_state(&state)
            .unwrap_err()
            .to_string()
            .contains("data_dir not configured"));

        let dir = unique_test_dir("cache");
        state.lock().unwrap().data_dir = dir.display().to_string();
        let catalog = CloudMatterProfileCatalog {
            schema_version: 1,
            profile_version: 99,
            generated_at: Some("2026-01-01T00:00:00Z".to_string()),
            profiles: vec![CloudMatterDeviceProfile {
                profile_key: "cached".to_string(),
                profile_version: 3,
                ..Default::default()
            }],
        };

        save_cached_for_state(&state, &catalog).unwrap();
        let path = cache_path(&state).unwrap();
        assert_eq!(path, dir.join("matter").join("cloud_profiles.json"));
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());

        let loaded = load_cached_for_state(&state).unwrap();
        assert_eq!(loaded.schema_version, 1);
        assert_eq!(loaded.profile_version, 99);
        assert_eq!(loaded.profiles[0].profile_key, "cached");

        for value in ["0", "false", "off", "disabled"] {
            std::env::set_var(PROFILE_SYNC_DISABLE_ENV, value);
            assert!(profile_sync_disabled(), "{} should disable sync", value);
        }
        for value in ["1", "true", "enabled", ""] {
            std::env::set_var(PROFILE_SYNC_DISABLE_ENV, value);
            assert!(
                !profile_sync_disabled(),
                "{} should not disable sync",
                value
            );
        }
        std::env::remove_var(PROFILE_SYNC_DISABLE_ENV);

        let _ = fs::remove_dir_all(dir);
    }
}
