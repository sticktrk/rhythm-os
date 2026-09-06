//! Scene definitions and preview state.
//!
//! Scenes are saved output presets. They are intentionally separate from light
//! profiles: profiles calculate ongoing behavior, while scenes capture a
//! concrete multi-target look that may be previewed or committed.

use std::collections::BTreeMap;

use rhythm_core::{rgb_to_xy, LightingCommand, Rgb, XyColor};
use serde::{Deserialize, Serialize};

pub const LIGHT_SCENE_SCHEMA_VERSION: u32 = 1;
pub const DEFAULT_LIGHT_SCENE_PREVIEW_MS: u64 = 30_000;
const NATIVE_SCENE_ID_PREFIX: &str = "native-";

fn default_schema_version() -> u32 {
    LIGHT_SCENE_SCHEMA_VERSION
}

fn default_scene_source() -> SceneSource {
    SceneSource::User
}

fn default_light_scene_power() -> LightScenePower {
    LightScenePower::On
}

fn default_light_scene_brightness() -> u8 {
    100
}

/// Build the stable public ID for an integration-owned scene.
///
/// Native definitions remain ephemeral, but this reserved ID can be stored in
/// room state and backups so the owning integration can resolve it again after
/// reconnect or restore.
pub fn native_scene_id(provider: &str, external_id: &str) -> String {
    let provider = normalize_scene_id(provider, "integration");
    let external_id = normalize_scene_id(external_id, "scene");
    format!("{NATIVE_SCENE_ID_PREFIX}{provider}-{external_id}")
}

/// Whether an ID belongs to the reserved integration-owned scene namespace.
pub fn is_native_scene_id(id: &str) -> bool {
    let Some(reference) = id.strip_prefix(NATIVE_SCENE_ID_PREFIX) else {
        return false;
    };
    reference
        .split_once('-')
        .is_some_and(|(provider, external_id)| !provider.is_empty() && !external_id.is_empty())
}

/// Where a scene originated.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SceneSource {
    User,
    Imported {
        provider: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        external_id: Option<String>,
    },
}

/// A saved scene. The top-level shape is modality-neutral so future sound or
/// other device domains can be added without changing the scene identity model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneDefinition {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default = "default_scene_source")]
    pub source: SceneSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub light: Option<LightSceneLayer>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub extensions: BTreeMap<String, serde_json::Value>,
}

impl SceneDefinition {
    pub fn normalize(&mut self) {
        self.id = normalize_scene_id(&self.id, &self.name);
        self.name = self.name.trim().to_string();
        if self.name.is_empty() {
            self.name = self.id.clone();
        }
        if let Some(light) = &mut self.light {
            light.normalize();
        }
    }
}

fn normalize_scene_id(id: &str, fallback_name: &str) -> String {
    let source = if id.trim().is_empty() {
        fallback_name.trim()
    } else {
        id.trim()
    };
    let mut out = String::new();
    let mut last_sep = false;
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
            last_sep = false;
        } else if (ch == '-' || ch == '_' || ch.is_whitespace()) && !out.is_empty() && !last_sep {
            out.push('-');
            last_sep = true;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        crate::canonical::identity::generate_uuid_public()
    } else {
        out
    }
}

/// Lighting-specific scene layer.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSceneLayer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_transition_ms: Option<u32>,
    /// Optional fallback for scene-addressable lights in the target scope that
    /// do not have a more specific entry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_output: Option<LightSceneOutput>,
    /// Optional fallback palette for scene-addressable lights in the target
    /// scope. Room targets with per-device topology routes assign palette
    /// outputs in stable target order; grouped room dispatch uses the first
    /// palette output.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub palette: Vec<LightSceneOutput>,
    #[serde(default)]
    pub entries: Vec<LightSceneEntry>,
}

impl LightSceneLayer {
    pub fn normalize(&mut self) {
        if let Some(default_output) = &mut self.default_output {
            default_output.normalize();
        }
        for output in &mut self.palette {
            output.normalize();
        }
        for entry in &mut self.entries {
            entry.output.normalize();
        }
    }
}

/// One concrete light target in a scene.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSceneEntry {
    pub target: LightSceneTargetRef,
    pub output: LightSceneOutput,
}

/// Reference to a light scene target.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LightSceneTargetRef {
    /// Topology/canonical light-device node ID.
    Node { node_id: String },
}

impl LightSceneTargetRef {
    pub fn node_id(&self) -> &str {
        match self {
            Self::Node { node_id } => node_id,
        }
    }
}

/// Rendered light output for a scene entry.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightSceneOutput {
    #[serde(default = "default_light_scene_power")]
    pub power: LightScenePower,
    #[serde(default = "default_light_scene_brightness")]
    pub brightness: u8,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<LightSceneColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
}

impl LightSceneOutput {
    pub fn normalize(&mut self) {
        self.brightness = self.brightness.clamp(1, 100);
    }

    pub fn transition_ms(&self, layer_transition_ms: Option<u32>) -> Option<u32> {
        self.transition_ms.or(layer_transition_ms)
    }

    pub fn to_command(
        &self,
        layer_transition_ms: Option<u32>,
    ) -> Result<Option<LightingCommand>, String> {
        if self.power == LightScenePower::Off {
            return Ok(None);
        }
        let transition_ms = self.transition_ms.or(layer_transition_ms);
        let color = self
            .color
            .ok_or_else(|| "powered-on light scene output requires a color".to_string())?;
        let command = match color {
            LightSceneColor::Kelvin { kelvin } => {
                let command = LightingCommand::new(self.brightness, kelvin.clamp(500, 25_000));
                match transition_ms {
                    Some(ms) => command.transition(ms),
                    None => command,
                }
            }
            LightSceneColor::Rgb { rgb } => {
                LightingCommand::from_color(self.brightness, rgb, rgb_to_xy(rgb), transition_ms)
            }
            LightSceneColor::Xy { xy } => {
                LightingCommand::from_color(self.brightness, Rgb::new(0, 0, 0), xy, transition_ms)
            }
            LightSceneColor::RgbXy { rgb, xy } => {
                LightingCommand::from_color(self.brightness, rgb, xy, transition_ms)
            }
        };
        Ok(Some(command))
    }
}

/// Whether a scene output should turn a light on or off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightScenePower {
    On,
    Off,
}

/// Color encoding for a scene output.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LightSceneColor {
    Kelvin { kelvin: u16 },
    Rgb { rgb: Rgb },
    Xy { xy: XyColor },
    RgbXy { rgb: Rgb, xy: XyColor },
}

/// Durable scene collection.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct StoredScenes {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub scenes: Vec<SceneDefinition>,
    /// Factory-default scene IDs this install has already been offered.
    ///
    /// A new factory-default scene is seeded into an existing install exactly
    /// once. Recording the IDs that were seeded keeps a scene the user deleted
    /// from reappearing on the next restart. An empty list means the file was
    /// written before tracking existed.
    #[serde(default)]
    pub seeded_factory_scene_ids: Vec<String>,
}

/// Ephemeral preview session. Preview sessions are not persisted.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LightScenePreviewSession {
    pub id: String,
    pub scene_id: String,
    pub target_node_id: String,
    pub affected_node_ids: Vec<String>,
    pub previous_mood_scene_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub draft_scene: Option<SceneDefinition>,
    pub started_at_epoch_ms: u64,
    pub expires_at_epoch_ms: u64,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SceneApplyRequest {
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct ScenePreviewRequest {
    pub target_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct SceneDraftPreviewRequest {
    pub target_id: String,
    pub scene: SceneDefinition,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transition_ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

/// Whole-home scene apply request.
///
/// Unlike [`SceneApplyRequest`] there is no `target_id`: the server enumerates
/// every eligible room itself so the client never has to fan out. Dispatch
/// pacing and correlation are read from the same envelope fields node batches
/// use (`dispatch_spacing_ms`, `correlation_id`).
#[derive(Clone, Debug, Default, Deserialize)]
pub struct HomeSceneApplyRequest {
    #[serde(default)]
    pub transition_ms: Option<u32>,
    /// How the house is carved into targets. Defaults to rooms.
    #[serde(default)]
    pub target_mode: HomeSceneTargetMode,
}

/// How a whole-home scene apply chooses its targets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HomeSceneTargetMode {
    /// Every eligible room (plus every roomless light) is one target, planned
    /// by the single-target planner: a grouped room recalls one managed
    /// projection or one group command, and the palette rotates room by room.
    #[default]
    Rooms,
    /// Every eligible light device is one target regardless of its room. Each
    /// device gets its own command, the palette rotates across the whole house
    /// in one continuous order, and dispatch runs as one paced lane per hub so
    /// hubs proceed concurrently. This is the path for scenes authored per
    /// device so the house reads as one theme instead of a set of rooms.
    Devices,
}

/// One hub's share of a device-mode whole-home dispatch.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeSceneDispatchLane {
    pub hub: String,
    #[serde(default)]
    pub dispatch_count: usize,
}

/// Per-target outcome of a whole-home scene apply.
///
/// A target that failed to plan or dispatch reports `error` and does not stop
/// the remaining targets.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeSceneTargetResult {
    pub target_id: String,
    #[serde(default)]
    pub affected_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HomeSceneApplyResponse {
    pub scene_id: String,
    #[serde(default)]
    pub targets: Vec<HomeSceneTargetResult>,
    #[serde(default)]
    pub applied_target_count: usize,
    #[serde(default)]
    pub skipped_target_count: usize,
    #[serde(default)]
    pub queued: bool,
    #[serde(default)]
    pub dispatch_count: usize,
    #[serde(default)]
    pub dispatch_spacing_ms: u64,
    #[serde(default)]
    pub estimated_dispatch_ms: u64,
    #[serde(default)]
    pub target_mode: HomeSceneTargetMode,
    /// Present in device mode: one entry per hub lane dispatched concurrently.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispatch_lanes: Vec<HomeSceneDispatchLane>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SceneApplyResponse {
    pub scene_id: String,
    pub target_id: String,
    pub affected_node_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub unresolved_node_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_id: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_id_normalizes_from_name() {
        assert_eq!(normalize_scene_id("", "Icy Glow"), "icy-glow");
        assert_eq!(normalize_scene_id("  My Scene!! ", ""), "my-scene");
    }

    #[test]
    fn native_scene_ids_are_stable_and_reserved() {
        let id = native_scene_id("Hue", "5AA2EAC8-118E-4C25");
        assert_eq!(id, "native-hue-5aa2eac8-118e-4c25");
        assert!(is_native_scene_id(&id));
        assert!(!is_native_scene_id("hue-5aa2eac8-118e-4c25"));
        assert!(!is_native_scene_id("native-hue"));
    }

    #[test]
    fn output_clamps_brightness() {
        let mut output = LightSceneOutput {
            power: LightScenePower::On,
            brightness: 250,
            color: Some(LightSceneColor::Kelvin { kelvin: 4000 }),
            transition_ms: None,
        };
        output.normalize();
        assert_eq!(output.brightness, 100);
    }

    #[test]
    fn output_can_represent_power_off_without_color() {
        let output = LightSceneOutput {
            power: LightScenePower::Off,
            brightness: 100,
            color: None,
            transition_ms: Some(250),
        };

        assert_eq!(output.to_command(None).unwrap(), None);
    }

    #[test]
    fn powered_on_output_requires_color() {
        let output = LightSceneOutput {
            power: LightScenePower::On,
            brightness: 100,
            color: None,
            transition_ms: None,
        };

        assert!(output.to_command(None).unwrap_err().contains("color"));
    }
}
