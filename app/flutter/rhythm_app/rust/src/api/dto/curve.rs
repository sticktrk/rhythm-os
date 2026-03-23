//! Curve configuration DTOs for Flutter.

use rhythm_core::{
    CurveConfig,
    config::{
        DEFAULT_MIN_COLOR_TEMP, DEFAULT_MAX_COLOR_TEMP,
        DEFAULT_MIN_BRIGHTNESS, DEFAULT_MAX_BRIGHTNESS,
        DEFAULT_WIDTH_LEFT_BRI, DEFAULT_WIDTH_RIGHT_BRI,
        DEFAULT_WIDTH_LEFT_CCT, DEFAULT_WIDTH_RIGHT_CCT,
        DEFAULT_SHAPE_P, DEFAULT_MAX_DIM_STEPS,
    },
};

use super::color::RgbDto;
use super::solar::SolarInfoDto;

/// Curve configuration DTO for Flutter.
///
/// Maps to `CurveConfig` in Flutter's `config_model.dart`.
/// Uses super-Gaussian (flat-topped bell curve) parameters.
#[derive(Debug, Clone)]
pub struct CurveConfigDto {
    pub min_color_temp: i32,
    pub max_color_temp: i32,
    pub min_brightness: i32,
    pub max_brightness: i32,
    /// Morning ramp speed for brightness (<1 = faster, >1 = slower)
    pub width_left_bri: f64,
    /// Evening ramp speed for brightness (<1 = faster, >1 = slower)
    pub width_right_bri: f64,
    /// Morning ramp speed for CCT (<1 = faster, >1 = slower)
    pub width_left_cct: f64,
    /// Evening ramp speed for CCT (<1 = faster, >1 = slower)
    pub width_right_cct: f64,
    /// Shape exponent for super-Gaussian curve (2 = round, 6 = flat plateau)
    pub shape_p: f64,
    pub max_dim_steps: i32,
}

impl Default for CurveConfigDto {
    fn default() -> Self {
        Self {
            min_color_temp: DEFAULT_MIN_COLOR_TEMP as i32,
            max_color_temp: DEFAULT_MAX_COLOR_TEMP as i32,
            min_brightness: DEFAULT_MIN_BRIGHTNESS as i32,
            max_brightness: DEFAULT_MAX_BRIGHTNESS as i32,
            width_left_bri: DEFAULT_WIDTH_LEFT_BRI as f64,
            width_right_bri: DEFAULT_WIDTH_RIGHT_BRI as f64,
            width_left_cct: DEFAULT_WIDTH_LEFT_CCT as f64,
            width_right_cct: DEFAULT_WIDTH_RIGHT_CCT as f64,
            shape_p: DEFAULT_SHAPE_P as f64,
            max_dim_steps: DEFAULT_MAX_DIM_STEPS as i32,
        }
    }
}

impl From<CurveConfigDto> for CurveConfig {
    fn from(dto: CurveConfigDto) -> Self {
        CurveConfig {
            min_color_temp: dto.min_color_temp as u16,
            max_color_temp: dto.max_color_temp as u16,
            min_brightness: dto.min_brightness as u8,
            max_brightness: dto.max_brightness as u8,
            width_left_bri: dto.width_left_bri as f32,
            width_right_bri: dto.width_right_bri as f32,
            width_left_cct: dto.width_left_cct as f32,
            width_right_cct: dto.width_right_cct as f32,
            shape_p: dto.shape_p as f32,
            max_dim_steps: dto.max_dim_steps as u8,
        }
    }
}

/// Curve data for visualization.
///
/// Maps to `CurveData` in Flutter's `rhythm_api.dart`.
#[derive(Debug, Clone)]
pub struct CurveDataDto {
    pub hours: Vec<f64>,
    pub brightness: Vec<i32>,
    pub kelvin: Vec<i32>,
    pub solar: SolarInfoDto,
}

/// Lighting values at a specific time.
#[derive(Debug, Clone)]
pub struct LightingValuesDto {
    pub kelvin: i32,
    /// Color temperature in mireds (micro reciprocal degrees).
    /// Calculated as 1,000,000 / kelvin. Used by Hue bulbs (ct parameter).
    pub mireds: i32,
    pub brightness: i32,
    pub rgb: RgbDto,
    pub xy: super::color::XyDto,
    pub solar_time: f64,
    pub sun_position: f64,
}

/// A single step point for visualization.
#[derive(Debug, Clone)]
pub struct StepPointDto {
    pub hour: f64,
    pub brightness: i32,
    pub kelvin: i32,
    pub rgb: Vec<i32>,
}

/// Step sequences for dimming visualization.
#[derive(Debug, Clone)]
pub struct StepSequencesDto {
    pub step_up: Vec<StepPointDto>,
    pub step_down: Vec<StepPointDto>,
}
