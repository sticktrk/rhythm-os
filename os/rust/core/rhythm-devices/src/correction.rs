//! Per-device command corrections derived from physical measurements.
//!
//! The logical side of each curve is the Hue-relative target Rhythm wants.
//! The command side is the value that a specific device must receive to best
//! reproduce that target. Curves are optional and default to identity mapping.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct BrightnessCorrectionPoint {
    pub logical_percent: u8,
    pub command_percent: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ColorTemperatureCorrectionPoint {
    pub logical_kelvin: u16,
    pub command_kelvin: u16,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ControlCorrections {
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub brightness: Vec<BrightnessCorrectionPoint>,
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Vec::is_empty")
    )]
    pub color_temperature: Vec<ColorTemperatureCorrectionPoint>,
}

impl ControlCorrections {
    pub fn is_empty(&self) -> bool {
        self.brightness.is_empty() && self.color_temperature.is_empty()
    }

    /// Curves must have at least two strictly increasing logical points and
    /// values inside their command domains. Invalid curves fail safely to an
    /// identity mapping at runtime.
    pub fn is_valid(&self) -> bool {
        valid_brightness_curve(&self.brightness)
            && valid_color_temperature_curve(&self.color_temperature)
    }

    pub fn map_brightness(&self, logical_percent: u8) -> u8 {
        if logical_percent == 0 || self.brightness.is_empty() {
            return logical_percent;
        }
        if !valid_brightness_curve(&self.brightness) {
            return logical_percent;
        }
        interpolate_brightness(&self.brightness, logical_percent)
    }

    pub fn map_color_temperature(&self, logical_kelvin: u16) -> u16 {
        if self.color_temperature.is_empty()
            || !valid_color_temperature_curve(&self.color_temperature)
        {
            return logical_kelvin;
        }
        interpolate_color_temperature(&self.color_temperature, logical_kelvin)
    }
}

fn valid_brightness_curve(points: &[BrightnessCorrectionPoint]) -> bool {
    points.is_empty()
        || (points.len() >= 2
            && points.iter().all(|point| {
                (1..=100).contains(&point.logical_percent)
                    && (1..=100).contains(&point.command_percent)
            })
            && points
                .windows(2)
                .all(|pair| pair[0].logical_percent < pair[1].logical_percent))
}

fn valid_color_temperature_curve(points: &[ColorTemperatureCorrectionPoint]) -> bool {
    points.is_empty()
        || (points.len() >= 2
            && points.iter().all(|point| {
                (1000..=20_000).contains(&point.logical_kelvin)
                    && (1000..=20_000).contains(&point.command_kelvin)
            })
            && points
                .windows(2)
                .all(|pair| pair[0].logical_kelvin < pair[1].logical_kelvin))
}

fn interpolate_brightness(points: &[BrightnessCorrectionPoint], logical: u8) -> u8 {
    let first = points
        .first()
        .expect("validated correction curve is non-empty");
    let last = points
        .last()
        .expect("validated correction curve is non-empty");
    if logical <= first.logical_percent {
        return first.command_percent;
    }
    if logical >= last.logical_percent {
        return last.command_percent;
    }

    for pair in points.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if logical <= right.logical_percent {
            return interpolate_value(
                u16::from(left.logical_percent),
                u16::from(left.command_percent),
                u16::from(right.logical_percent),
                u16::from(right.command_percent),
                u16::from(logical),
            ) as u8;
        }
    }
    unreachable!("logical value inside correction bounds must have an interpolation pair")
}

fn interpolate_color_temperature(points: &[ColorTemperatureCorrectionPoint], logical: u16) -> u16 {
    let first = points
        .first()
        .expect("validated correction curve is non-empty");
    let last = points
        .last()
        .expect("validated correction curve is non-empty");
    if logical <= first.logical_kelvin {
        return first.command_kelvin;
    }
    if logical >= last.logical_kelvin {
        return last.command_kelvin;
    }

    for pair in points.windows(2) {
        let left = pair[0];
        let right = pair[1];
        if logical <= right.logical_kelvin {
            return interpolate_value(
                left.logical_kelvin,
                left.command_kelvin,
                right.logical_kelvin,
                right.command_kelvin,
                logical,
            );
        }
    }
    unreachable!("logical value inside correction bounds must have an interpolation pair")
}

fn interpolate_value(left_x: u16, left_y: u16, right_x: u16, right_y: u16, x: u16) -> u16 {
    let fraction = f64::from(x - left_x) / f64::from(right_x - left_x);
    let interpolated = f64::from(left_y) + fraction * (f64::from(right_y) - f64::from(left_y));
    interpolated.round().clamp(0.0, f64::from(u16::MAX)) as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corrections() -> ControlCorrections {
        ControlCorrections {
            brightness: vec![
                BrightnessCorrectionPoint {
                    logical_percent: 1,
                    command_percent: 7,
                },
                BrightnessCorrectionPoint {
                    logical_percent: 5,
                    command_percent: 13,
                },
                BrightnessCorrectionPoint {
                    logical_percent: 100,
                    command_percent: 100,
                },
            ],
            color_temperature: vec![
                ColorTemperatureCorrectionPoint {
                    logical_kelvin: 2700,
                    command_kelvin: 2400,
                },
                ColorTemperatureCorrectionPoint {
                    logical_kelvin: 4000,
                    command_kelvin: 3650,
                },
                ColorTemperatureCorrectionPoint {
                    logical_kelvin: 6500,
                    command_kelvin: 6200,
                },
            ],
        }
    }

    #[test]
    fn maps_endpoints_and_interpolates() {
        let corrections = corrections();
        assert!(corrections.is_valid());
        assert_eq!(corrections.map_brightness(0), 0);
        assert_eq!(corrections.map_brightness(1), 7);
        assert_eq!(corrections.map_brightness(3), 10);
        assert_eq!(corrections.map_color_temperature(2700), 2400);
        assert_eq!(corrections.map_color_temperature(3350), 3025);
    }

    #[test]
    fn clamps_outside_curve_and_invalid_curves_fail_to_identity() {
        let corrections = corrections();
        assert_eq!(corrections.map_brightness(100), 100);
        assert_eq!(corrections.map_color_temperature(2200), 2400);

        let invalid = ControlCorrections {
            brightness: vec![
                BrightnessCorrectionPoint {
                    logical_percent: 10,
                    command_percent: 20,
                },
                BrightnessCorrectionPoint {
                    logical_percent: 5,
                    command_percent: 10,
                },
            ],
            color_temperature: Vec::new(),
        };
        assert!(!invalid.is_valid());
        assert_eq!(invalid.map_brightness(7), 7);
    }
}
