//! Solar DTOs for Flutter.

/// Solar information DTO.
#[derive(Debug, Clone)]
pub struct SolarInfoDto {
    pub sunrise: Option<f64>,
    pub sunset: Option<f64>,
    pub solar_noon: f64,
    pub solar_midnight: f64,
    pub day_length: Option<f64>,
    /// Dawn twilight times (before sunrise)
    pub dawn: Option<TwilightPhaseDto>,
    /// Dusk twilight times (after sunset)
    pub dusk: Option<TwilightPhaseDto>,
}

/// Sun times DTO for sunrise/sunset calculations.
#[derive(Debug, Clone)]
pub struct SunTimesDto {
    /// Sunrise time in local decimal hours (0-24)
    pub sunrise: f64,
    /// Sunset time in local decimal hours (0-24)
    pub sunset: f64,
    /// Solar noon in local decimal hours (0-24)
    pub solar_noon: f64,
    /// Solar midnight in local decimal hours (0-24)
    pub solar_midnight: f64,
    /// Day length in hours
    pub day_length: f64,
}

/// Twilight phase times DTO for Flutter.
///
/// Times are in local decimal hours (0-24).
/// `None` indicates the event doesn't occur (polar regions).
#[derive(Debug, Clone)]
pub struct TwilightPhaseDto {
    /// Civil twilight: sun 6° below horizon
    pub civil: Option<f64>,
    /// Nautical twilight: sun 12° below horizon
    pub nautical: Option<f64>,
    /// Astronomical twilight: sun 18° below horizon
    pub astronomical: Option<f64>,
}

/// Full twilight times DTO for Flutter.
#[derive(Debug, Clone)]
pub struct TwilightTimesDto {
    /// Dawn twilight times (morning, before sunrise)
    pub dawn: TwilightPhaseDto,
    /// Dusk twilight times (evening, after sunset)
    pub dusk: TwilightPhaseDto,
}
