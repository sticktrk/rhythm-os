//! CIE xy color gamut definitions and math.

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

/// A point in the CIE 1931 xy chromaticity diagram.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct XyPoint {
    pub x: f32,
    pub y: f32,
}

impl XyPoint {
    pub const fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }
}

/// A color gamut defined by three CIE xy vertices (RGB primaries).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct GamutTriangle {
    pub red: XyPoint,
    pub green: XyPoint,
    pub blue: XyPoint,
}

impl GamutTriangle {
    pub const fn new(red: XyPoint, green: XyPoint, blue: XyPoint) -> Self {
        Self { red, green, blue }
    }

    /// Check if an xy point lies inside this gamut triangle.
    pub fn contains(&self, point: &XyPoint) -> bool {
        let d1 = cross(point, &self.red, &self.green);
        let d2 = cross(point, &self.green, &self.blue);
        let d3 = cross(point, &self.blue, &self.red);

        let has_neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
        let has_pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);

        !(has_neg && has_pos)
    }

    /// Clamp an xy point to the nearest point inside this gamut triangle.
    pub fn clamp_xy(&self, point: XyPoint) -> XyPoint {
        if self.contains(&point) {
            return point;
        }

        // Find the closest point on each triangle edge, pick the nearest.
        let candidates = [
            closest_point_on_segment(&point, &self.red, &self.green),
            closest_point_on_segment(&point, &self.green, &self.blue),
            closest_point_on_segment(&point, &self.blue, &self.red),
        ];

        candidates
            .into_iter()
            .min_by(|a, b| {
                let da = distance_sq(&point, a);
                let db = distance_sq(&point, b);
                da.partial_cmp(&db).unwrap_or(core::cmp::Ordering::Equal)
            })
            .unwrap_or(point)
    }
}

/// Well-known Philips Hue gamuts.
///
/// Gamut A: Older Hue bulbs (LCT001, LCT007).
pub fn gamut_a() -> GamutTriangle {
    GamutTriangle::new(
        XyPoint::new(0.704, 0.296),
        XyPoint::new(0.2151, 0.7106),
        XyPoint::new(0.138, 0.08),
    )
}

/// Gamut B: Hue A19, BR30, etc. (LCT010, LCT014).
pub fn gamut_b() -> GamutTriangle {
    GamutTriangle::new(
        XyPoint::new(0.675, 0.322),
        XyPoint::new(0.409, 0.518),
        XyPoint::new(0.167, 0.04),
    )
}

/// Gamut C: Hue gen 3+ (LCT016, LCA001, etc.).
pub fn gamut_c() -> GamutTriangle {
    GamutTriangle::new(
        XyPoint::new(0.6915, 0.3038),
        XyPoint::new(0.17, 0.7),
        XyPoint::new(0.1532, 0.0475),
    )
}

/// Resolve a named gamut to its triangle vertices.
pub fn named_gamut(name: &str) -> Option<GamutTriangle> {
    match name.to_uppercase().as_str() {
        "A" => Some(gamut_a()),
        "B" => Some(gamut_b()),
        "C" => Some(gamut_c()),
        _ => None,
    }
}

// --- Internal math ---

/// 2D cross product sign for point-in-triangle test.
fn cross(p: &XyPoint, a: &XyPoint, b: &XyPoint) -> f32 {
    (p.x - b.x) * (a.y - b.y) - (a.x - b.x) * (p.y - b.y)
}

/// Squared distance between two points.
fn distance_sq(a: &XyPoint, b: &XyPoint) -> f32 {
    let dx = a.x - b.x;
    let dy = a.y - b.y;
    dx * dx + dy * dy
}

/// Closest point on line segment AB to point P.
fn closest_point_on_segment(p: &XyPoint, a: &XyPoint, b: &XyPoint) -> XyPoint {
    let ab_x = b.x - a.x;
    let ab_y = b.y - a.y;
    let ap_x = p.x - a.x;
    let ap_y = p.y - a.y;

    let ab_sq = ab_x * ab_x + ab_y * ab_y;
    if ab_sq == 0.0 {
        return *a;
    }

    let t = ((ap_x * ab_x + ap_y * ab_y) / ab_sq).clamp(0.0, 1.0);

    XyPoint::new(a.x + t * ab_x, a.y + t * ab_y)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_gamut_c_contains_white() {
        let gamut = gamut_c();
        // D65 white point (typical daylight)
        assert!(gamut.contains(&XyPoint::new(0.3127, 0.3290)));
    }

    #[test]
    fn test_gamut_c_excludes_extreme() {
        let gamut = gamut_c();
        // Pure spectral green — outside any gamut
        assert!(!gamut.contains(&XyPoint::new(0.0, 0.9)));
    }

    #[test]
    fn test_clamp_inside_returns_same() {
        let gamut = gamut_c();
        let point = XyPoint::new(0.3127, 0.3290);
        let clamped = gamut.clamp_xy(point);
        assert!((clamped.x - point.x).abs() < 1e-6);
        assert!((clamped.y - point.y).abs() < 1e-6);
    }

    #[test]
    fn test_clamp_outside_moves_to_boundary() {
        let gamut = gamut_c();
        let outside = XyPoint::new(0.0, 0.9);
        let clamped = gamut.clamp_xy(outside);
        assert!(gamut.contains(&clamped));
    }

    #[test]
    fn test_named_gamut_lookup() {
        assert!(named_gamut("A").is_some());
        assert!(named_gamut("B").is_some());
        assert!(named_gamut("C").is_some());
        assert!(named_gamut("c").is_some()); // case-insensitive
        assert!(named_gamut("D").is_none());
        assert!(named_gamut("").is_none());
    }

    #[test]
    fn test_vertex_on_boundary() {
        let gamut = gamut_c();
        // Vertices should be inside (on boundary)
        assert!(gamut.contains(&gamut.red));
        assert!(gamut.contains(&gamut.green));
        assert!(gamut.contains(&gamut.blue));
    }
}
