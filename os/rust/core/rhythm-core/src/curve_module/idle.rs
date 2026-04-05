//! Idle curve module for soft_off mode.
//!
//! Cycles through the full color spectrum across the solar day so
//! that every 2-3 hour segment is visually distinct, even at 1%
//! brightness. Uses direct RGB colors outside the blackbody spectrum.
//! The curve's brightness (1%) is authoritative for soft-off dimming.

extern crate alloc;

use rhythm_curve::color::{rgb_to_xy, Rgb};
use rhythm_curve::context::CurveContext;
use rhythm_curve::module::LightCurveModule;
use rhythm_curve::steps::{StepAction, StepResult};
use rhythm_curve::values::LightingValues;

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// Configuration for the idle curve.
///
/// Both `fade_ms` and `motion_timeout_secs` are optional. When `None`,
/// `RhythmCurveModule::calculate_idle()` fills them from the main curve.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct IdleCurveConfig {
    /// Fallback static color (used if the palette is empty).
    pub rgb: Rgb,
    /// Fade duration in milliseconds for idle/soft-off transitions.
    /// None = inherit from the main curve's `fade_ms`.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub fade_ms: Option<u16>,
    /// Motion timeout in seconds for idle mode.
    /// None = inherit from the main curve's time-of-day-aware timeout.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub motion_timeout_secs: Option<u16>,
}

impl Default for IdleCurveConfig {
    fn default() -> Self {
        Self {
            rgb: Rgb::new(255, 147, 41), // warm amber
            fade_ms: None,
            motion_timeout_secs: None,
        }
    }
}

/// A single color keyframe in the idle palette.
struct Keyframe {
    hour: f32,
    r: u8,
    g: u8,
    b: u8,
}

/// The 24-hour idle palette keyed to solar time.
///
/// Colors rotate around the spectrum so every 2-3 hour segment is
/// visually distinct, even at 1% brightness on real bulbs.
///
/// Midnight: deep blue → dawn: teal/cyan → morning: warm amber →
/// noon: coral/orange → afternoon: magenta/rose → evening: purple →
/// night: indigo → midnight: deep blue
const PALETTE: &[Keyframe] = &[
    Keyframe {
        hour: 0.0,
        r: 10,
        g: 10,
        b: 80,
    },
    Keyframe {
        hour: 3.0,
        r: 5,
        g: 50,
        b: 80,
    },
    Keyframe {
        hour: 5.0,
        r: 5,
        g: 80,
        b: 60,
    },
    Keyframe {
        hour: 7.0,
        r: 40,
        g: 80,
        b: 10,
    },
    Keyframe {
        hour: 9.0,
        r: 180,
        g: 100,
        b: 5,
    },
    Keyframe {
        hour: 11.0,
        r: 220,
        g: 60,
        b: 5,
    },
    Keyframe {
        hour: 13.0,
        r: 220,
        g: 30,
        b: 30,
    },
    Keyframe {
        hour: 15.0,
        r: 200,
        g: 20,
        b: 100,
    },
    Keyframe {
        hour: 17.0,
        r: 160,
        g: 15,
        b: 180,
    },
    Keyframe {
        hour: 19.0,
        r: 80,
        g: 10,
        b: 200,
    },
    Keyframe {
        hour: 21.0,
        r: 40,
        g: 10,
        b: 160,
    },
    Keyframe {
        hour: 23.0,
        r: 15,
        g: 10,
        b: 100,
    },
    Keyframe {
        hour: 24.0,
        r: 10,
        g: 10,
        b: 80,
    },
];

/// Smoothstep for S-curved transitions between keyframes.
#[inline]
fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Interpolate between two keyframes with smoothstep easing.
fn lerp(a: &Keyframe, b: &Keyframe, t: f32) -> Rgb {
    let t = smoothstep(t);
    let r = (a.r as f32 + (b.r as f32 - a.r as f32) * t).round() as u8;
    let g = (a.g as f32 + (b.g as f32 - a.g as f32) * t).round() as u8;
    let bv = (a.b as f32 + (b.b as f32 - a.b as f32) * t).round() as u8;
    Rgb::new(r, g, bv)
}

/// Sample the idle palette at a given solar hour (0–24).
fn sample(hour: f32) -> Rgb {
    let h = hour.rem_euclid(24.0);
    for i in 0..PALETTE.len() - 1 {
        if h >= PALETTE[i].hour && h <= PALETTE[i + 1].hour {
            let span = PALETTE[i + 1].hour - PALETTE[i].hour;
            let frac = if span > 0.0 {
                (h - PALETTE[i].hour) / span
            } else {
                0.0
            };
            return lerp(&PALETTE[i], &PALETTE[i + 1], frac);
        }
    }
    Rgb::new(PALETTE[0].r, PALETTE[0].g, PALETTE[0].b)
}

/// A curve module that cycles through the full color spectrum over 24 hours.
///
/// Used for idle (soft_off) mode. The brightness value (1%) is
/// authoritative — the engine uses it directly for soft-off dimming.
///
/// Night: deep blue. Dawn: teal → green. Morning: warm amber → orange.
/// Noon: coral/red peak. Afternoon: rose → magenta. Evening: purple →
/// indigo. Night: back to deep blue.
#[derive(Debug, Clone)]
pub struct IdleCurveModule {
    config: IdleCurveConfig,
}

impl IdleCurveModule {
    /// Module identifier.
    pub const ID: &str = "idle";

    /// Module display name.
    pub const NAME: &str = "Idle Curve";

    /// Create a new idle curve module with the given config.
    pub fn new(config: IdleCurveConfig) -> Self {
        Self { config }
    }

    /// Create with default config.
    pub fn with_defaults() -> Self {
        Self::new(IdleCurveConfig::default())
    }

    /// Get the current config.
    pub fn config(&self) -> &IdleCurveConfig {
        &self.config
    }
}

impl LightCurveModule for IdleCurveModule {
    fn id(&self) -> &str {
        Self::ID
    }

    fn name(&self) -> &str {
        Self::NAME
    }

    fn calculate(&self, ctx: &CurveContext) -> LightingValues {
        let solar_time = ctx.solar_time();
        let rgb = sample(solar_time);
        let xy = rgb_to_xy(rgb);
        LightingValues::from_color(
            rgb,
            xy,
            1,
            solar_time,
            0.0,
            self.config.fade_ms.unwrap_or(0) as u32,
            self.config.motion_timeout_secs.unwrap_or(0),
        )
    }

    fn calculate_brightness(&self, _ctx: &CurveContext) -> u8 {
        1 // soft-off brightness: 1%
    }

    fn calculate_color_temperature(&self, _ctx: &CurveContext) -> u16 {
        0 // not meaningful for direct color
    }

    fn calculate_step(&self, ctx: &CurveContext, _action: StepAction) -> StepResult {
        StepResult {
            values: self.calculate(ctx),
            time_offset_minutes: 0.0,
            at_boundary: true,
        }
    }

    fn is_at_maximum(&self, _ctx: &CurveContext) -> bool {
        true
    }

    fn is_at_minimum(&self, _ctx: &CurveContext) -> bool {
        true
    }

    fn min_brightness(&self) -> u8 {
        1
    }

    fn max_brightness(&self) -> u8 {
        1
    }

    fn min_color_temp(&self) -> u16 {
        0
    }

    fn max_color_temp(&self) -> u16 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_curve::solar::SolarTime;

    fn ctx_at(hour: f32) -> CurveContext {
        CurveContext::new(hour, SolarTime::default(), None)
    }

    #[test]
    fn test_idle_curve_identity() {
        let curve = IdleCurveModule::with_defaults();
        assert_eq!(curve.id(), "idle");
        assert_eq!(curve.name(), "Idle Curve");
    }

    #[test]
    fn test_idle_curve_outputs_direct_color() {
        let curve = IdleCurveModule::with_defaults();
        let values = curve.calculate(&ctx_at(12.0));

        assert!(values.is_direct_color);
        assert_eq!(values.kelvin, 0);
        assert!(values.xy.x > 0.0);
        assert!(values.xy.y > 0.0);
    }

    #[test]
    fn test_color_varies_across_day() {
        let curve = IdleCurveModule::with_defaults();

        let midnight = curve.calculate(&ctx_at(0.0));
        let noon = curve.calculate(&ctx_at(12.0));
        let evening = curve.calculate(&ctx_at(20.0));

        assert_ne!(midnight.rgb, noon.rgb);
        assert_ne!(noon.rgb, evening.rgb);
    }

    #[test]
    fn test_midnight_wrap_is_smooth() {
        let curve = IdleCurveModule::with_defaults();

        let before = curve.calculate(&ctx_at(23.9));
        let after = curve.calculate(&ctx_at(0.1));

        let dr = (before.rgb.r as i16 - after.rgb.r as i16).unsigned_abs();
        let dg = (before.rgb.g as i16 - after.rgb.g as i16).unsigned_abs();
        let db = (before.rgb.b as i16 - after.rgb.b as i16).unsigned_abs();

        assert!(dr < 10, "red jump at midnight: {}", dr);
        assert!(dg < 5, "green jump at midnight: {}", dg);
        assert!(db < 10, "blue jump at midnight: {}", db);
    }

    #[test]
    fn test_dominant_colors_follow_palette() {
        let curve = IdleCurveModule::with_defaults();

        // Morning: warm amber, green/red dominate over blue
        let morning = curve.calculate(&ctx_at(9.0));
        assert!(
            morning.rgb.r > morning.rgb.b,
            "morning should be warm (red > blue)"
        );

        // Evening: purple/blue dominates
        let evening = curve.calculate(&ctx_at(19.0));
        assert!(
            evening.rgb.b > evening.rgb.g,
            "evening should be blue-dominant"
        );

        // Night: blue dominant
        let night = curve.calculate(&ctx_at(2.0));
        assert!(night.rgb.b > night.rgb.r, "night should be blue-dominant");
    }

    #[test]
    fn test_idle_curve_always_at_boundary() {
        let curve = IdleCurveModule::with_defaults();
        let ctx = ctx_at(10.0);

        assert!(curve.is_at_maximum(&ctx));
        assert!(curve.is_at_minimum(&ctx));

        let step = curve.calculate_step(&ctx, StepAction::Brighten);
        assert!(step.at_boundary);
        assert_eq!(step.time_offset_minutes, 0.0);
    }

    #[test]
    fn test_xy_coordinates_valid() {
        let curve = IdleCurveModule::with_defaults();
        for hour in 0..24 {
            let values = curve.calculate(&ctx_at(hour as f32));
            assert!(
                values.xy.x >= 0.0 && values.xy.x <= 1.0,
                "x out of range at h={}",
                hour
            );
            assert!(
                values.xy.y >= 0.0 && values.xy.y <= 1.0,
                "y out of range at h={}",
                hour
            );
        }
    }

    #[test]
    fn test_brightness_always_nominal() {
        let curve = IdleCurveModule::with_defaults();
        for hour in [0.0, 6.0, 12.0, 18.0] {
            assert_eq!(curve.calculate_brightness(&ctx_at(hour)), 1);
        }
    }

}
