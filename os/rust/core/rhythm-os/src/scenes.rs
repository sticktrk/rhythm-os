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
    /// How the palette is dealt across the lights it covers. Defaults to
    /// [`PaletteMode::Spread`].
    #[serde(default)]
    pub palette_mode: PaletteMode,
    #[serde(default)]
    pub entries: Vec<LightSceneEntry>,
}

/// How a palette is dealt across the lights it covers.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PaletteMode {
    /// Treat the palette entries as anchors on a colour path and derive one
    /// distinct colour per light by walking the path from the first anchor to
    /// the last, so every bulb the apply covers differs whatever the palette
    /// length. With no more lights than anchors, the anchors are used as they
    /// are, in order.
    #[default]
    Spread,
    /// Deal the palette entries out in order and wrap, so colours repeat every
    /// `palette.len()` lights.
    Cycle,
}

/// A window onto a scene palette: the slot a target starts at and, when known,
/// how many slots the whole apply covers.
///
/// Spread mode places each slot on the colour path relative to the span, so a
/// whole-home apply passes the house-wide span to every target and persists
/// it with the binding; `None` means the span is the target's own light count.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PaletteWindow {
    pub offset: usize,
    pub span: Option<usize>,
}

impl PaletteWindow {
    /// The span to render `own_slots` lights with, unless the apply supplied a
    /// house-wide one.
    pub fn span_or(&self, own_slots: usize) -> usize {
        self.span.unwrap_or(own_slots).max(1)
    }
}

fn lerp(from: f32, to: f32, fraction: f32) -> f32 {
    from + (to - from) * fraction
}

fn rgb_to_hsv(rgb: Rgb) -> (f32, f32, f32) {
    let r = rgb.r as f32 / 255.0;
    let g = rgb.g as f32 / 255.0;
    let b = rgb.b as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if max == r {
        60.0 * (((g - b) / delta) % 6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let saturation = if max <= f32::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, saturation, max)
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> Rgb {
    let hue = hue.rem_euclid(360.0);
    let chroma = value * saturation;
    let x = chroma * (1.0 - ((hue / 60.0) % 2.0 - 1.0).abs());
    let m = value - chroma;
    let (r, g, b) = match hue {
        h if h < 60.0 => (chroma, x, 0.0),
        h if h < 120.0 => (x, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, x),
        h if h < 240.0 => (0.0, x, chroma),
        h if h < 300.0 => (x, 0.0, chroma),
        _ => (chroma, 0.0, x),
    };
    let channel = |v: f32| ((v + m) * 255.0).round().clamp(0.0, 255.0) as u8;
    Rgb::new(channel(r), channel(g), channel(b))
}

/// Blend two colours along the shortest arc of the hue wheel, so a walk from
/// orange to purple passes through red rather than fading through grey.
fn blend_rgb_on_hue_wheel(from: Rgb, to: Rgb, fraction: f32) -> Rgb {
    let (from_hue, from_sat, from_val) = rgb_to_hsv(from);
    let (to_hue, to_sat, to_val) = rgb_to_hsv(to);
    // A near-grey anchor has no meaningful hue; borrow the other side's.
    let from_hue = if from_sat < 0.05 { to_hue } else { from_hue };
    let to_hue = if to_sat < 0.05 { from_hue } else { to_hue };
    let delta = (to_hue - from_hue + 540.0).rem_euclid(360.0) - 180.0;
    hsv_to_rgb(
        from_hue + delta * fraction,
        lerp(from_sat, to_sat, fraction),
        lerp(from_val, to_val, fraction),
    )
}

/// The output `fraction` of the way from `from` to `to`.
///
/// Brightness blends linearly; RGB colours blend around the hue wheel and
/// colour temperatures blend linearly. Anything else (an off anchor, or a
/// mismatched colour kind) falls back to `from`.
fn blend_outputs(
    from: &LightSceneOutput,
    to: &LightSceneOutput,
    fraction: f32,
) -> LightSceneOutput {
    if fraction <= f32::EPSILON {
        return from.clone();
    }
    if fraction >= 1.0 - f32::EPSILON {
        return to.clone();
    }
    if from.power == LightScenePower::Off || to.power == LightScenePower::Off {
        return from.clone();
    }
    let brightness = lerp(from.brightness as f32, to.brightness as f32, fraction)
        .round()
        .clamp(1.0, 100.0) as u8;
    let color = match (&from.color, &to.color) {
        (
            Some(LightSceneColor::Rgb { rgb: from_rgb })
            | Some(LightSceneColor::RgbXy { rgb: from_rgb, .. }),
            Some(LightSceneColor::Rgb { rgb: to_rgb })
            | Some(LightSceneColor::RgbXy { rgb: to_rgb, .. }),
        ) => Some(LightSceneColor::Rgb {
            rgb: blend_rgb_on_hue_wheel(*from_rgb, *to_rgb, fraction),
        }),
        (
            Some(LightSceneColor::Kelvin {
                kelvin: from_kelvin,
            }),
            Some(LightSceneColor::Kelvin { kelvin: to_kelvin }),
        ) => Some(LightSceneColor::Kelvin {
            kelvin: lerp(*from_kelvin as f32, *to_kelvin as f32, fraction).round() as u16,
        }),
        _ => from.color,
    };
    LightSceneOutput {
        power: LightScenePower::On,
        brightness,
        color,
        transition_ms: from.transition_ms,
    }
}

impl LightSceneLayer {
    /// The palette output for slot `slot` of an apply covering `span` slots.
    ///
    /// Cycle mode returns `palette[slot % len]`. Spread mode does the same
    /// while the apply covers no more lights than there are anchors, and
    /// otherwise walks the path from the first anchor to the last: slot 0 is
    /// the first anchor, the last slot is the last anchor, and every slot in
    /// between blends the two anchors it falls between, so every slot of the
    /// span is distinct and the anchors themselves still appear along the
    /// way. The path is deliberately open rather than a loop: closing it would
    /// retrace the same hues on the way back and hand two lights one colour.
    /// `None` when the scene has no palette.
    pub fn palette_output(&self, slot: usize, span: usize) -> Option<LightSceneOutput> {
        let anchors = self.palette.len();
        if anchors == 0 {
            return None;
        }
        let cycled = || self.palette[slot % anchors].clone();
        match self.palette_mode {
            PaletteMode::Cycle => Some(cycled()),
            PaletteMode::Spread if span <= anchors => Some(cycled()),
            PaletteMode::Spread => {
                // span > anchors >= 1, so span >= 2 and the division is safe.
                let last_leg = anchors - 1;
                let position = (slot.min(span - 1) as f32 / (span - 1) as f32) * last_leg as f32;
                let lower = (position.floor() as usize).min(last_leg.saturating_sub(1));
                let fraction = position - lower as f32;
                Some(blend_outputs(
                    &self.palette[lower],
                    &self.palette[(lower + 1).min(last_leg)],
                    fraction,
                ))
            }
        }
    }

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
    /// How the house is carved into targets. Defaults to devices.
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
    Rooms,
    /// Every eligible light device is one target regardless of its room. Each
    /// device gets its own command, the palette rotates across the whole house
    /// in one continuous order, and dispatch runs as one paced lane per hub so
    /// hubs proceed concurrently. The default: a whole-home scene is one theme
    /// for the house, not a set of per-room recalls.
    #[default]
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

    fn rgb_output(rgb: Rgb, brightness: u8) -> LightSceneOutput {
        LightSceneOutput {
            power: LightScenePower::On,
            brightness,
            color: Some(LightSceneColor::Rgb { rgb }),
            transition_ms: Some(700),
        }
    }

    fn palette_layer(mode: PaletteMode, outputs: Vec<LightSceneOutput>) -> LightSceneLayer {
        LightSceneLayer {
            default_transition_ms: None,
            default_output: None,
            palette: outputs,
            palette_mode: mode,
            entries: Vec::new(),
        }
    }

    fn rgb_of(output: &LightSceneOutput) -> (u8, u8, u8) {
        match output.color {
            Some(LightSceneColor::Rgb { rgb }) => (rgb.r, rgb.g, rgb.b),
            other => panic!("expected rgb, got {other:?}"),
        }
    }

    #[test]
    fn palette_mode_defaults_to_spread_and_reads_from_the_wire() {
        let layer: LightSceneLayer = serde_json::from_str(r#"{"palette": []}"#).unwrap();
        assert_eq!(layer.palette_mode, PaletteMode::Spread);
        let layer: LightSceneLayer =
            serde_json::from_str(r#"{"palette": [], "palette_mode": "cycle"}"#).unwrap();
        assert_eq!(layer.palette_mode, PaletteMode::Cycle);
    }

    #[test]
    fn spread_uses_the_anchors_as_they_are_when_the_span_fits() {
        let orange = Rgb::new(255, 104, 0);
        let purple = Rgb::new(122, 0, 214);
        let layer = palette_layer(
            PaletteMode::Spread,
            vec![rgb_output(orange, 80), rgb_output(purple, 70)],
        );
        assert_eq!(rgb_of(&layer.palette_output(0, 2).unwrap()), (255, 104, 0));
        assert_eq!(rgb_of(&layer.palette_output(1, 2).unwrap()), (122, 0, 214));
        assert_eq!(layer.palette_output(1, 2).unwrap().brightness, 70);
    }

    #[test]
    fn spread_derives_one_distinct_colour_per_slot_across_a_larger_span() {
        let orange = Rgb::new(255, 104, 0);
        let purple = Rgb::new(122, 0, 214);
        let layer = palette_layer(
            PaletteMode::Spread,
            vec![rgb_output(orange, 80), rgb_output(purple, 40)],
        );
        let span = 6;
        let outputs: Vec<_> = (0..span)
            .map(|slot| layer.palette_output(slot, span).unwrap())
            .collect();
        let colours: std::collections::HashSet<_> = outputs.iter().map(rgb_of).collect();
        assert_eq!(colours.len(), span, "every slot gets its own colour");
        // The path starts on the first anchor and ends on the last.
        assert_eq!(rgb_of(&outputs[0]), (255, 104, 0));
        assert_eq!(rgb_of(&outputs[5]), (122, 0, 214));
        assert_eq!(outputs[0].brightness, 80);
        assert_eq!(outputs[5].brightness, 40);
        // Half way between orange and purple the loop passes through red, not
        // through grey, and brightness blends with it.
        let (r, g, b) = rgb_of(&outputs[1]);
        assert!(
            r > 200 && g < 104 && b < 120,
            "slot 1 walks toward red: {:?}",
            (r, g, b)
        );
        assert!(outputs[1].brightness < 80 && outputs[1].brightness > 40);
        assert_eq!(
            outputs[1].transition_ms,
            Some(700),
            "blends keep the anchor's timing"
        );
        // Brightness falls monotonically along the walk.
        let brightnesses: Vec<_> = outputs.iter().map(|output| output.brightness).collect();
        assert!(
            brightnesses.windows(2).all(|pair| pair[0] > pair[1]),
            "{brightnesses:?}"
        );
    }

    #[test]
    fn cycle_repeats_the_anchors_and_never_blends() {
        let layer = palette_layer(
            PaletteMode::Cycle,
            vec![
                rgb_output(Rgb::new(255, 0, 0), 50),
                rgb_output(Rgb::new(0, 0, 255), 50),
            ],
        );
        assert_eq!(rgb_of(&layer.palette_output(0, 6).unwrap()), (255, 0, 0));
        assert_eq!(rgb_of(&layer.palette_output(1, 6).unwrap()), (0, 0, 255));
        assert_eq!(rgb_of(&layer.palette_output(2, 6).unwrap()), (255, 0, 0));
    }

    #[test]
    fn spread_blends_colour_temperatures_and_keeps_off_anchors_intact() {
        let warm = LightSceneOutput {
            power: LightScenePower::On,
            brightness: 100,
            color: Some(LightSceneColor::Kelvin { kelvin: 2000 }),
            transition_ms: None,
        };
        let cool = LightSceneOutput {
            power: LightScenePower::On,
            brightness: 100,
            color: Some(LightSceneColor::Kelvin { kelvin: 6000 }),
            transition_ms: None,
        };
        let layer = palette_layer(PaletteMode::Spread, vec![warm.clone(), cool]);
        assert_eq!(
            layer.palette_output(1, 4).unwrap().color,
            Some(LightSceneColor::Kelvin { kelvin: 3333 })
        );
        assert_eq!(
            layer.palette_output(3, 4).unwrap().color,
            Some(LightSceneColor::Kelvin { kelvin: 6000 }),
            "the last slot is the last anchor"
        );

        let off = LightSceneOutput {
            power: LightScenePower::Off,
            brightness: 1,
            color: None,
            transition_ms: None,
        };
        let layer = palette_layer(PaletteMode::Spread, vec![warm, off.clone()]);
        assert_eq!(
            layer.palette_output(1, 4).unwrap().power,
            LightScenePower::On
        );
        assert_eq!(
            layer.palette_output(2, 4).unwrap().power,
            LightScenePower::On
        );
        assert_eq!(layer.palette_output(3, 4).unwrap(), off);
    }

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
