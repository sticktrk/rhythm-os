//! Color DTOs for Flutter.

/// RGB color DTO.
#[derive(Debug, Clone)]
pub struct RgbDto {
    pub r: i32,
    pub g: i32,
    pub b: i32,
}

/// CIE xy color coordinates DTO.
#[derive(Debug, Clone)]
pub struct XyDto {
    pub x: f64,
    pub y: f64,
}
