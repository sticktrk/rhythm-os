//! Runner action DTOs that still cross the Rust bridge.

/// Action type for rhythm runner button events.
///
/// The app handles room state locally in Dart, but Hue button mapping still
/// returns this enum from Rust.
#[flutter_rust_bridge::frb(unignore)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RhythmActionDto {
    /// Toggle lights on/off with rhythm mode
    OnPress,
    /// Turn off lights
    OffPress,
    /// Reset to current time position
    Reset,
    /// Step up (brighten and cool) along the curve
    StepUp,
    /// Step down (dim and warm) along the curve
    StepDown,
    /// Enable rhythm mode and turn on
    RhythmOn,
    /// Disable rhythm mode (lights unchanged)
    RhythmOff,
    /// Dim up (brightness only, no color temp change)
    DimUp,
    /// Dim down (brightness only, no color temp change)
    DimDown,
}
