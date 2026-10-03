//! HA light identity, capability and observation normalization.
use rhythm_devices::{ColorMode, LightCapabilities, LightType};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

/// Registry identity proves ownership; the mutable entity id is only a route.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaLightIdentity {
    pub scope: String,
    pub registry_id: String,
    pub unique_id: String,
    pub platform: String,
    pub device_id: Option<String>,
    pub config_entry_id: Option<String>,
}

impl HaLightIdentity {
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        format!(
            "ha-registry:{:x}",
            Sha256::digest(serde_json::to_vec(self).expect("identity serializes"))
        )
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct HaLightObservation {
    pub state: String,
    pub lights_on: Option<bool>,
    pub brightness: Option<u8>,
    pub kelvin: Option<u16>,
    pub xy: Option<[f64; 2]>,
    pub rgb: Option<[u8; 3]>,
    pub context_id: Option<String>,
    pub last_updated: Option<String>,
}
impl HaLightObservation {
    pub fn parse(value: &Value) -> Self {
        let state = value["state"].as_str().unwrap_or("unknown").to_string();
        let attrs = &value["attributes"];
        Self {
            lights_on: match state.as_str() {
                "on" => Some(true),
                "off" => Some(false),
                _ => None,
            },
            state,
            brightness: attrs["brightness"]
                .as_u64()
                .filter(|v| *v <= 255)
                .map(|v| ((v * 100 + 127) / 255) as u8),
            kelvin: kelvin(attrs, "color_temp_kelvin", "color_temp"),
            xy: serde_json::from_value::<[f64; 2]>(attrs["xy_color"].clone())
                .ok()
                .filter(|xy| xy.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v))),
            rgb: serde_json::from_value(attrs["rgb_color"].clone()).ok(),
            context_id: value["context"]["id"].as_str().map(str::to_owned),
            last_updated: value["last_updated"].as_str().map(str::to_owned),
        }
    }
    pub fn normalized(&self) -> rhythm_os::hub::LightObservation {
        rhythm_os::hub::LightObservation {
            availability: if self.available() {
                "available".to_owned()
            } else {
                self.state.clone()
            },
            lights_on: self.lights_on,
            brightness: self.brightness,
            kelvin: self.kelvin,
            xy: self.xy,
            rgb: self.rgb,
            context_id: self.context_id.clone(),
            source_at_epoch_ms: self
                .last_updated
                .as_deref()
                .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
                .map(|at| at.timestamp_millis()),
            received_at_epoch_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
        }
    }
    pub fn newer_than(&self, previous: &Self) -> bool {
        match (
            self.normalized().source_at_epoch_ms,
            previous.normalized().source_at_epoch_ms,
        ) {
            (Some(next), Some(old)) => next > old,
            (None, Some(_)) => false,
            _ => true,
        }
    }
    pub fn available(&self) -> bool {
        self.lights_on.is_some()
    }
}

fn kelvin(attrs: &Value, modern: &str, legacy: &str) -> Option<u16> {
    attrs[modern]
        .as_u64()
        .filter(|v| *v > 0 && *v <= u16::MAX as u64)
        .map(|v| v as u16)
        .or_else(|| {
            attrs[legacy]
                .as_u64()
                .filter(|v| *v > 0)
                .and_then(|v| u16::try_from(1_000_000 / v).ok())
                .filter(|v| *v > 0)
        })
}

pub fn capabilities(attrs: &Value) -> LightCapabilities {
    let modes: Vec<&str> = attrs["supported_color_modes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .collect();
    let color = modes
        .iter()
        .any(|m| matches!(*m, "hs" | "xy" | "rgb" | "rgbw" | "rgbww"));
    let cct = modes.contains(&"color_temp");
    let dimmable = color || cct || modes.iter().any(|m| matches!(*m, "brightness" | "white"));
    let mut caps = LightCapabilities::defaults_for(if color {
        LightType::ExtendedColor
    } else if cct {
        LightType::ColorTemperature
    } else if dimmable {
        LightType::Dimmable
    } else {
        LightType::OnOff
    });
    caps.color_modes.clear();
    // HA translates service colors into the entity's native supported mode.
    if color {
        caps.color_modes.push(ColorMode::Xy);
    }
    if cct {
        caps.color_modes.push(ColorMode::ColorTemperature);
    }
    if caps.color_modes.is_empty() {
        caps.color_modes.push(if dimmable {
            ColorMode::Dimmable
        } else {
            ColorMode::OnOff
        });
    }
    caps.min_kelvin = if cct {
        kelvin(attrs, "min_color_temp_kelvin", "max_mireds")
    } else {
        None
    };
    caps.max_kelvin = if cct {
        kelvin(attrs, "max_color_temp_kelvin", "min_mireds")
    } else {
        None
    };
    if matches!((caps.min_kelvin, caps.max_kelvin), (Some(a), Some(b)) if a > b) {
        caps.min_kelvin = None;
        caps.max_kelvin = None;
    }
    // HA LightEntityFeature.TRANSITION = 32. Absence is explicitly unsupported.
    caps.supports_transition = attrs["supported_features"].as_u64().unwrap_or(0) & 32 != 0;
    caps
}

#[derive(Clone, Debug)]
pub struct HaLightCatalogEntry {
    pub identity: Option<HaLightIdentity>,
    pub name: String,
    pub area_id: Option<String>,
    pub capabilities: LightCapabilities,
    pub observation: HaLightObservation,
}

pub const SELECTION_FILE: &str = "managed-ha-lights-v2.json";
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HaLightSelection {
    pub version: u32,
    pub lights: BTreeMap<String, HaLightIdentity>,
}
impl HaLightSelection {
    pub fn load(dir: &str) -> anyhow::Result<Self> {
        let path = std::path::Path::new(dir).join(SELECTION_FILE);
        if !path.exists() {
            return Ok(Self {
                version: 2,
                lights: BTreeMap::new(),
            });
        }
        let value: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        anyhow::ensure!(
            value.version == 2 && value.lights.len() <= 4096,
            "Unsupported HA selection schema"
        );
        anyhow::ensure!(
            value.lights.values().all(|v| !v.scope.is_empty()
                && !v.registry_id.is_empty()
                && !v.unique_id.is_empty()
                && !v.platform.is_empty()),
            "Invalid HA selection identity"
        );
        Ok(value)
    }
    pub fn resolve(
        &self,
        catalog: &BTreeMap<String, HaLightCatalogEntry>,
    ) -> std::collections::BTreeSet<String> {
        catalog
            .iter()
            .filter(|(_, entry)| {
                entry.identity.as_ref().is_some_and(|identity| {
                    self.lights.values().any(|reviewed| reviewed == identity)
                })
            })
            .map(|(id, _)| id.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn capabilities_preserve_actual_modes_ranges_and_transition_support() {
        let caps = capabilities(
            &json!({"supported_color_modes":["color_temp"],"min_mireds":200,"max_mireds":454,"supported_features":32}),
        );
        assert_eq!(caps.light_type, LightType::ColorTemperature);
        assert_eq!(caps.min_kelvin, Some(2202));
        assert_eq!(caps.max_kelvin, Some(5000));
        assert!(caps.supports_transition);
        let caps = capabilities(&json!({"supported_color_modes":["rgb"]}));
        assert_eq!(caps.color_modes, vec![ColorMode::Xy]);
        assert_eq!(caps.min_kelvin, None);
        assert!(!caps.supports_transition);
        let caps = capabilities(&json!({"supported_color_modes":["onoff"]}));
        assert_eq!(caps.light_type, LightType::OnOff);
        assert!(!caps.supports_transition);
    }
    #[test]
    fn observations_order_rfc3339_timestamps_with_offsets() {
        let first = HaLightObservation::parse(
            &json!({"state":"on","last_updated":"2026-10-03T10:00:00-04:00"}),
        );
        let later = HaLightObservation::parse(
            &json!({"state":"off","last_updated":"2026-10-03T14:00:01+00:00"}),
        );
        assert!(later.newer_than(&first));
        assert!(!first.newer_than(&later));
        assert!(!later.newer_than(&later));
    }
}
