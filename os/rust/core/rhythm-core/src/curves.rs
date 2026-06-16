//! Curve calculations for adaptive lighting.
//!
//! This module implements super-Gaussian (flat-topped bell curve) mapping
//! for brightness and color temperature curves throughout the day.
//!
//! The super-Gaussian formula: y(t) = exp(-((t - μ) / σ)^p)
//! - μ (mu) = peak hour, computed as (sunrise + sunset) / 2
//! - σ (sigma) = width parameter, different for left/right sides
//! - p = shape exponent (2=round top, 6=flat plateau)

use libm::{expf, fabsf, logf, powf};

/// Direction indicator for curve calculations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveDirection {
    /// Morning curve (values rise from min to max)
    Morning,
    /// Evening curve (values fall from max to min)
    Evening,
}

impl CurveDirection {
    /// Get the numeric direction value (+1 for morning, -1 for evening).
    pub fn value(&self) -> i8 {
        match self {
            CurveDirection::Morning => 1,
            CurveDirection::Evening => -1,
        }
    }

    /// Check if this is the morning direction.
    pub fn is_morning(&self) -> bool {
        matches!(self, CurveDirection::Morning)
    }
}

/// Simplified logistic mapping for morning/evening curves.
///
/// This function maps time to output values using a logistic (sigmoid) curve.
///
/// # Arguments
///
/// * `t` - Time in solar hours (0-12 for morning, 12-24 for evening)
/// * `midpoint` - Midpoint of the curve (hours)
/// * `steepness` - Steepness of the curve
/// * `out_min` - Minimum output value
/// * `out_max` - Maximum output value
/// * `direction` - Curve direction (Morning rises, Evening falls)
///
/// # Returns
///
/// Output value clamped to [out_min, out_max]
///
/// # Example
///
/// ```
/// use rhythm_core::curves::{map_half, CurveDirection};
///
/// // Morning curve: brightness rises from 1% to 100%
/// let brightness = map_half(6.0, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning);
/// assert!((brightness - 50.5).abs() < 1.0); // At midpoint, roughly 50%
/// ```
pub fn map_half(
    t: f32,
    midpoint: f32,
    steepness: f32,
    out_min: f32,
    out_max: f32,
    direction: CurveDirection,
) -> f32 {
    // Adjust time for evening calculation
    let te = if direction.is_morning() { t } else { t - 12.0 };

    // Calculate base logistic
    let base = if direction.is_morning() {
        // Morning: standard logistic (rises from 0 to 1)
        1.0 / (1.0 + expf(-steepness * (te - midpoint)))
    } else {
        // Evening: inverted logistic (falls from 1 to 0)
        1.0 - 1.0 / (1.0 + expf(-steepness * (te - midpoint)))
    };

    // Map to output range
    let span = out_max - out_min;
    let result = out_min + span * base;

    // Clamp to output range
    clamp(result, out_min, out_max)
}

/// Helper function to clamp a value between min and max.
#[inline]
pub fn clamp(value: f32, min: f32, max: f32) -> f32 {
    if value < min {
        min
    } else if value > max {
        max
    } else {
        value
    }
}

// ============================================================================
// Super-Gaussian curve implementation
// ============================================================================

/// Edge brightness constant - at sunrise/sunset, curve value is at this fraction.
/// 0.02 = 2% of the way from min to max
pub const EPSILON: f32 = 0.02;

/// Default shape exponent for the super-Gaussian curve.
/// 2 = round top (normal Gaussian), 6 = flat plateau
pub const DEFAULT_SHAPE_P: f32 = 6.0;

/// Default width multiplier (1.0 = width calibrated so y(edge) = EPSILON)
pub const DEFAULT_WIDTH: f32 = 1.0;

/// Base widths computed from sunrise/sunset times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BaseWidths {
    /// Peak hour (midpoint between sunrise and sunset)
    pub mu: f32,
    /// Base left width (before applying multiplier)
    pub sigma_l0: f32,
    /// Base right width (before applying multiplier)
    pub sigma_r0: f32,
}

/// Compute base widths so that y(sunrise) = ε and y(sunset) = ε.
///
/// The formula derives σ such that the curve reaches ε at the edges:
///   σ_L^0 = (μ - sunrise) / ln(1/ε)^(1/p)
///   σ_R^0 = (sunset - μ) / ln(1/ε)^(1/p)
///
/// # Arguments
///
/// * `sunrise` - Sunrise hour (0-24)
/// * `sunset` - Sunset hour (0-24)
/// * `shape_p` - Shape exponent (2-10)
///
/// # Returns
///
/// BaseWidths containing mu, sigma_l0, and sigma_r0
pub fn compute_base_widths(sunrise: f32, sunset: f32, shape_p: f32) -> BaseWidths {
    let mu = (sunrise + sunset) / 2.0;
    let ln_inv_eps = logf(1.0 / EPSILON); // ln(1/0.02) ≈ 3.91
    let factor = powf(ln_inv_eps, 1.0 / shape_p);

    let left_span = mu - sunrise;
    let right_span = sunset - mu;

    // Handle edge cases with fallback
    let sigma_l0 = if left_span <= 0.0 {
        6.0
    } else {
        left_span / factor
    };
    let sigma_r0 = if right_span <= 0.0 {
        6.0
    } else {
        right_span / factor
    };

    BaseWidths {
        mu,
        sigma_l0,
        sigma_r0,
    }
}

/// Compute super-Gaussian value at time t.
///
/// y(t) = exp(-((t - μ) / σ)^p)
///
/// Uses different widths for left (before μ) and right (after μ) sides
/// to allow asymmetric morning/evening ramp speeds.
///
/// # Arguments
///
/// * `hour` - Clock hour (0-24)
/// * `mu` - Center of the curve (peak hour)
/// * `sigma_l` - Left width (for hours before mu)
/// * `sigma_r` - Right width (for hours after mu)
/// * `shape_p` - Shape exponent (2=round, 6=flat top)
///
/// # Returns
///
/// Value from 0 to 1
pub fn super_gaussian(hour: f32, mu: f32, sigma_l: f32, sigma_r: f32, shape_p: f32) -> f32 {
    // Handle wraparound for hours near midnight
    let mut dist = hour - mu;
    // Normalize to -12..+12 range for proper wrapping
    if dist > 12.0 {
        dist -= 24.0;
    }
    if dist < -12.0 {
        dist += 24.0;
    }

    let sigma = if dist <= 0.0 { sigma_l } else { sigma_r };
    if sigma <= 0.01 {
        return if fabsf(dist) < 0.01 { 1.0 } else { 0.0 };
    }

    let normalized = fabsf(dist) / sigma;
    expf(-powf(normalized, shape_p))
}

/// Map super-Gaussian output to an output range.
///
/// This is the main function for computing brightness or color temperature
/// at a given hour using the super-Gaussian curve.
///
/// # Arguments
///
/// * `hour` - Clock hour (0-24)
/// * `sunrise` - Sunrise hour
/// * `sunset` - Sunset hour
/// * `width_left` - Left width multiplier (<1 = faster ramp, >1 = slower)
/// * `width_right` - Right width multiplier
/// * `shape_p` - Shape exponent (2-10)
/// * `out_min` - Minimum output value
/// * `out_max` - Maximum output value
///
/// # Returns
///
/// Output value clamped to [out_min, out_max]
///
/// # Example
///
/// ```
/// use rhythm_core::curves::map_super_gaussian;
///
/// // Brightness curve from 1% to 100% with sunrise at 6, sunset at 18
/// let brightness = map_super_gaussian(12.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
/// assert!(brightness > 95.0); // At noon, should be near max
/// ```
#[allow(clippy::too_many_arguments)]
pub fn map_super_gaussian(
    hour: f32,
    sunrise: f32,
    sunset: f32,
    width_left: f32,
    width_right: f32,
    shape_p: f32,
    out_min: f32,
    out_max: f32,
) -> f32 {
    let base = compute_base_widths(sunrise, sunset, shape_p);

    // Apply width multipliers:
    // - width < 1 = larger sigma = gentler slope = slower ramp
    // - width > 1 = smaller sigma = steeper slope = faster ramp
    let sigma_l = base.sigma_l0 / width_left.max(0.1);
    let sigma_r = base.sigma_r0 / width_right.max(0.1);

    // Get normalized value (0-1)
    let y = super_gaussian(hour, base.mu, sigma_l, sigma_r, shape_p);

    // Map to output range
    let span = out_max - out_min;
    let result = out_min + y * span;

    clamp(result, out_min, out_max)
}

/// Find the hour that produces a target output value on the super-Gaussian curve.
///
/// Used for stepping along the curve to find where a target brightness would occur.
///
/// # Arguments
///
/// * `target` - Target output value
/// * `sunrise` - Sunrise hour
/// * `sunset` - Sunset hour
/// * `width_left` - Left width multiplier
/// * `width_right` - Right width multiplier
/// * `shape_p` - Shape exponent
/// * `out_min` - Minimum output value
/// * `out_max` - Maximum output value
/// * `is_morning` - Whether to search the morning (ascending) or evening (descending) side
///
/// # Returns
///
/// Hour that produces the target value, or None if outside valid range.
#[allow(clippy::too_many_arguments)]
pub fn inverse_super_gaussian(
    target: f32,
    sunrise: f32,
    sunset: f32,
    width_left: f32,
    width_right: f32,
    shape_p: f32,
    out_min: f32,
    out_max: f32,
    is_morning: bool,
) -> Option<f32> {
    let span = out_max - out_min;
    if fabsf(span) < f32::EPSILON {
        return None;
    }

    // Normalize target to 0-1 range
    let normalized = (target - out_min) / span;

    // Clamp to valid range (avoid log of 0 or negative)
    let normalized = clamp(normalized, EPSILON, 1.0 - f32::EPSILON);

    // Inverse of exp(-x^p) = y is x = (-ln(y))^(1/p)
    // Then hour = mu ± x * sigma
    let base = compute_base_widths(sunrise, sunset, shape_p);
    // Match the width relationship from map_super_gaussian
    let sigma = if is_morning {
        base.sigma_l0 / width_left.max(0.1)
    } else {
        base.sigma_r0 / width_right.max(0.1)
    };

    if sigma <= 0.01 {
        return Some(base.mu);
    }

    // x = (-ln(normalized))^(1/p)
    let ln_val = -logf(normalized);
    if ln_val < 0.0 {
        return Some(base.mu);
    }
    let x = powf(ln_val, 1.0 / shape_p);

    // Distance from mu
    let dist = x * sigma;

    // Apply direction
    let hour = if is_morning {
        base.mu - dist // Morning: before mu
    } else {
        base.mu + dist // Evening: after mu
    };

    // Wrap to 0-24 range
    let hour = if hour < 0.0 {
        hour + 24.0
    } else if hour >= 24.0 {
        hour - 24.0
    } else {
        hour
    };

    Some(hour)
}

/// Calculate the inverse of the logistic function.
///
/// Given an output value, find the time that would produce it.
///
/// # Arguments
///
/// * `output` - The target output value
/// * `midpoint` - Midpoint of the curve (hours)
/// * `steepness` - Steepness of the curve
/// * `out_min` - Minimum output value
/// * `out_max` - Maximum output value
/// * `direction` - Curve direction
///
/// # Returns
///
/// Time value that produces the output, or None if outside valid range.
pub fn inverse_map_half(
    output: f32,
    midpoint: f32,
    steepness: f32,
    out_min: f32,
    out_max: f32,
    direction: CurveDirection,
) -> Option<f32> {
    let span = out_max - out_min;
    if fabsf(span) < f32::EPSILON || fabsf(steepness) < f32::EPSILON {
        return None;
    }

    // Normalize output to 0-1 range
    let normalized = (output - out_min) / span;

    // Clamp to valid range (avoid log of 0 or negative)
    let normalized = clamp(normalized, 0.001, 0.999);

    // Calculate the base value (before direction adjustment)
    let base = if direction.is_morning() {
        normalized
    } else {
        1.0 - normalized
    };

    // Inverse logistic: t = midpoint + ln(base / (1 - base)) / steepness
    let te = midpoint + libm::logf(base / (1.0 - base)) / steepness;

    // Adjust for evening
    let t = if direction.is_morning() {
        te
    } else {
        te + 12.0
    };

    Some(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_morning_curve_endpoints() {
        // At very low t, output should be near minimum
        let low = map_half(0.0, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning);
        assert!(low < 10.0, "At t=0, expected low brightness, got {}", low);

        // At high t, output should be near maximum
        let high = map_half(12.0, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning);
        assert!(
            high > 90.0,
            "At t=12, expected high brightness, got {}",
            high
        );
    }

    #[test]
    fn test_morning_curve_midpoint() {
        // At midpoint, output should be roughly in the middle
        let mid = map_half(6.0, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning);
        assert!(
            (mid - 50.5).abs() < 2.0,
            "At midpoint, expected ~50%, got {}",
            mid
        );
    }

    #[test]
    fn test_evening_curve_endpoints() {
        // At t=12 (start of evening), output should be near maximum
        let high = map_half(12.0, 8.0, 1.3, 1.0, 100.0, CurveDirection::Evening);
        assert!(
            high > 90.0,
            "At t=12, expected high brightness, got {}",
            high
        );

        // At t=24 (end of evening), output should be near minimum
        let low = map_half(24.0, 8.0, 1.3, 1.0, 100.0, CurveDirection::Evening);
        assert!(low < 10.0, "At t=24, expected low brightness, got {}", low);
    }

    #[test]
    fn test_evening_curve_midpoint() {
        // At midpoint (12 + 8 = 20), output should be roughly in the middle
        let mid = map_half(20.0, 8.0, 1.3, 1.0, 100.0, CurveDirection::Evening);
        assert!(
            (mid - 50.5).abs() < 2.0,
            "At evening midpoint, expected ~50%, got {}",
            mid
        );
    }

    #[test]
    fn test_steepness_affects_transition() {
        // Higher steepness = sharper transition
        let steep = map_half(6.5, 6.0, 3.0, 1.0, 100.0, CurveDirection::Morning);
        let gentle = map_half(6.5, 6.0, 0.5, 1.0, 100.0, CurveDirection::Morning);

        // With steep curve, should be further from midpoint value
        assert!(steep > gentle, "Steep curve should transition faster");
    }

    #[test]
    fn test_inverse_roundtrip() {
        let original_t = 8.0;
        let output = map_half(original_t, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning);

        if let Some(recovered_t) =
            inverse_map_half(output, 6.0, 1.5, 1.0, 100.0, CurveDirection::Morning)
        {
            assert!(
                (recovered_t - original_t).abs() < 0.1,
                "Expected t={}, got t={}",
                original_t,
                recovered_t
            );
        } else {
            panic!("inverse_map_half returned None");
        }
    }

    #[test]
    fn test_clamp() {
        assert_eq!(clamp(5.0, 0.0, 10.0), 5.0);
        assert_eq!(clamp(-1.0, 0.0, 10.0), 0.0);
        assert_eq!(clamp(15.0, 0.0, 10.0), 10.0);
    }

    // ========================================================================
    // Super-Gaussian curve tests
    // ========================================================================

    #[test]
    fn test_compute_base_widths_symmetric() {
        // Symmetric day: sunrise at 6, sunset at 18
        let widths = compute_base_widths(6.0, 18.0, 6.0);
        assert!(
            (widths.mu - 12.0).abs() < 0.01,
            "mu should be 12, got {}",
            widths.mu
        );
        assert!(
            (widths.sigma_l0 - widths.sigma_r0).abs() < 0.01,
            "Symmetric widths expected"
        );
    }

    #[test]
    fn test_compute_base_widths_asymmetric() {
        // Asymmetric day: sunrise at 5, sunset at 20 (mu = 12.5)
        let widths = compute_base_widths(5.0, 20.0, 6.0);
        assert!(
            (widths.mu - 12.5).abs() < 0.01,
            "mu should be 12.5, got {}",
            widths.mu
        );
        // Left span is 7.5, right span is 7.5 - should still be symmetric
        assert!((widths.sigma_l0 - widths.sigma_r0).abs() < 0.01);
    }

    #[test]
    fn test_super_gaussian_at_peak() {
        // At mu, value should be 1.0
        let widths = compute_base_widths(6.0, 18.0, 6.0);
        let y = super_gaussian(widths.mu, widths.mu, widths.sigma_l0, widths.sigma_r0, 6.0);
        assert!(
            (y - 1.0).abs() < 0.01,
            "At peak, y should be 1.0, got {}",
            y
        );
    }

    #[test]
    fn test_super_gaussian_at_edges() {
        // At sunrise and sunset, value should be approximately EPSILON
        let widths = compute_base_widths(6.0, 18.0, 6.0);

        let y_sunrise = super_gaussian(6.0, widths.mu, widths.sigma_l0, widths.sigma_r0, 6.0);
        assert!(
            (y_sunrise - EPSILON).abs() < 0.01,
            "At sunrise, y should be ~{}, got {}",
            EPSILON,
            y_sunrise
        );

        let y_sunset = super_gaussian(18.0, widths.mu, widths.sigma_l0, widths.sigma_r0, 6.0);
        assert!(
            (y_sunset - EPSILON).abs() < 0.01,
            "At sunset, y should be ~{}, got {}",
            EPSILON,
            y_sunset
        );
    }

    #[test]
    fn test_map_super_gaussian_at_noon() {
        // At noon (peak), brightness should be near max
        let bri = map_super_gaussian(12.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        assert!(
            bri > 95.0,
            "At noon, brightness should be near max, got {}",
            bri
        );
    }

    #[test]
    fn test_map_super_gaussian_at_sunrise() {
        // At sunrise, brightness should be near min + 2%
        let bri = map_super_gaussian(6.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        let expected = 1.0 + EPSILON * 99.0; // 1 + 0.02 * 99 ≈ 2.98
        assert!(
            (bri - expected).abs() < 2.0,
            "At sunrise, brightness should be ~{}, got {}",
            expected,
            bri
        );
    }

    #[test]
    fn test_map_super_gaussian_at_sunset() {
        // At sunset, brightness should be near min + 2%
        let bri = map_super_gaussian(18.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        let expected = 1.0 + EPSILON * 99.0;
        assert!(
            (bri - expected).abs() < 2.0,
            "At sunset, brightness should be ~{}, got {}",
            expected,
            bri
        );
    }

    #[test]
    fn test_map_super_gaussian_midnight() {
        // At midnight, brightness should be at minimum
        let bri = map_super_gaussian(0.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        assert!(
            bri < 2.0,
            "At midnight, brightness should be at min, got {}",
            bri
        );
    }

    #[test]
    fn test_width_multiplier_faster() {
        // width < 1 = faster ramp (steeper curve)
        let bri_normal = map_super_gaussian(8.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        let bri_fast = map_super_gaussian(8.0, 6.0, 18.0, 0.5, 1.0, 6.0, 1.0, 100.0);

        // At hour 8, faster morning ramp should result in higher brightness
        assert!(
            bri_fast > bri_normal,
            "Faster ramp should give higher brightness at hour 8: fast={}, normal={}",
            bri_fast,
            bri_normal
        );
    }

    #[test]
    fn test_width_multiplier_slower() {
        // width > 1 = slower ramp (gentler curve)
        let bri_normal = map_super_gaussian(8.0, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
        let bri_slow = map_super_gaussian(8.0, 6.0, 18.0, 1.5, 1.0, 6.0, 1.0, 100.0);

        // At hour 8, slower morning ramp should result in lower brightness
        assert!(
            bri_slow < bri_normal,
            "Slower ramp should give lower brightness at hour 8: slow={}, normal={}",
            bri_slow,
            bri_normal
        );
    }

    #[test]
    fn test_shape_p_affects_flatness() {
        // Lower p = rounder top, higher p = flatter top
        let bri_round = map_super_gaussian(11.0, 6.0, 18.0, 1.0, 1.0, 2.0, 1.0, 100.0);
        let bri_flat = map_super_gaussian(11.0, 6.0, 18.0, 1.0, 1.0, 10.0, 1.0, 100.0);

        // At hour 11 (near peak), flatter curve should give higher brightness
        assert!(
            bri_flat > bri_round,
            "Flatter curve (p=10) should give higher brightness near peak: flat={}, round={}",
            bri_flat,
            bri_round
        );
    }

    #[test]
    fn test_inverse_super_gaussian_roundtrip() {
        // Test that inverse_super_gaussian can recover the hour from brightness
        let target_bri = 50.0;

        // Find hour on morning side that gives target brightness
        if let Some(hour) =
            inverse_super_gaussian(target_bri, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0, true)
        {
            // Verify the hour produces the target brightness
            let actual_bri = map_super_gaussian(hour, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
            assert!(
                (actual_bri - target_bri).abs() < 1.0,
                "Roundtrip failed: target={}, actual={}, hour={}",
                target_bri,
                actual_bri,
                hour
            );
        } else {
            panic!("inverse_super_gaussian returned None");
        }
    }

    #[test]
    fn test_inverse_super_gaussian_evening() {
        // Test evening side inverse
        let target_bri = 50.0;

        if let Some(hour) =
            inverse_super_gaussian(target_bri, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0, false)
        {
            let actual_bri = map_super_gaussian(hour, 6.0, 18.0, 1.0, 1.0, 6.0, 1.0, 100.0);
            assert!(
                (actual_bri - target_bri).abs() < 1.0,
                "Evening roundtrip failed: target={}, actual={}, hour={}",
                target_bri,
                actual_bri,
                hour
            );
            // Hour should be after noon
            assert!(
                hour > 12.0,
                "Evening hour should be after noon, got {}",
                hour
            );
        } else {
            panic!("inverse_super_gaussian returned None for evening");
        }
    }
}
