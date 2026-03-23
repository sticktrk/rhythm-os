//! Hue Button Event DTOs for Flutter.

/// Hue V2 API button event types for FFI.
///
/// These map directly to the `last_event` field in Hue V2 button resources
/// from the SSE event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HueButtonEventTypeDto {
    /// Button was initially pressed down.
    InitialPress,
    /// Button is being held down (repeat events).
    Repeat,
    /// Button released after a short press.
    ShortRelease,
    /// Button released after a long press.
    LongRelease,
    /// Button has been held for an extended time.
    LongPress,
}

impl HueButtonEventTypeDto {
    /// Parse from Hue V2 API string value.
    ///
    /// Returns `None` for unknown values.
    pub fn from_api_value(value: &str) -> Option<Self> {
        match value {
            "initial_press" => Some(Self::InitialPress),
            "repeat" => Some(Self::Repeat),
            "short_release" => Some(Self::ShortRelease),
            "long_release" => Some(Self::LongRelease),
            "long_press" => Some(Self::LongPress),
            _ => None,
        }
    }
}
