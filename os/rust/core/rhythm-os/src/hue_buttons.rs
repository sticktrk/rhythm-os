//! Hue button event mapping.
//!
//! Maps Hue V2 button events (from SSE or HA `hue_event`) to generic
//! `ButtonAction`s. Lives in rhythm-os so both `rhythm-hue` and `rhythm-ha`
//! can share the same mapping without depending on each other.

use rhythm_core::ButtonAction;

/// Hue V2 API button event types.
///
/// These map directly to the `last_event` field in Hue V2 button resources
/// from the SSE event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HueButtonEventType {
    InitialPress,
    Repeat,
    ShortRelease,
    LongRelease,
    LongPress,
}

impl HueButtonEventType {
    /// Parse from Hue V2 API string value.
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

/// Map a Hue dimmer switch button press to a generic `ButtonAction`.
///
/// # Hue Dimmer Switch (RWL02x) Button Layout
///
/// - Button 1 (top): "On" button
/// - Button 2: "Dim up" button
/// - Button 3: "Dim down" button
/// - Button 4 (bottom): "Off" button
pub fn map_hue_button(button_index: u8, event_type: HueButtonEventType) -> Option<ButtonAction> {
    match (button_index, event_type) {
        // Button 1 (top): short press = reset (clears offsets, enables rhythm, turns on),
        // long press = plain on (adaptive lighting with current offsets preserved).
        (1, HueButtonEventType::InitialPress) => Some(ButtonAction::Reset),
        (1, HueButtonEventType::LongRelease) => Some(ButtonAction::OnPress),
        // Buttons 1-3 trigger on button-down for immediate response.
        (2, HueButtonEventType::InitialPress) => Some(ButtonAction::UpPress),
        (2, HueButtonEventType::Repeat) => Some(ButtonAction::UpHold),
        (3, HueButtonEventType::InitialPress) => Some(ButtonAction::DownPress),
        (3, HueButtonEventType::Repeat) => Some(ButtonAction::DownHold),
        // Button 4 (bottom): short press = soft off (dim to 1%), long press = lights fully off.
        // Keep short press on release so a hold can promote cleanly to LongPress.
        (4, HueButtonEventType::ShortRelease) => Some(ButtonAction::OffPress),
        (4, HueButtonEventType::LongPress) => Some(ButtonAction::LightsOff),
        _ => None,
    }
}

/// Convenience: parse event type from string, then map to `ButtonAction`.
///
/// Combines `HueButtonEventType::from_api_value` + `map_hue_button` in one call.
/// Returns `None` if the event type string is unrecognized or the button/event
/// combination has no mapping.
pub fn map_hue_button_str(button_index: u8, event_type: &str) -> Option<ButtonAction> {
    let hue_event = HueButtonEventType::from_api_value(event_type)?;
    map_hue_button(button_index, hue_event)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::ButtonAction;

    #[test]
    fn from_api_value_initial_press() {
        assert_eq!(
            HueButtonEventType::from_api_value("initial_press"),
            Some(HueButtonEventType::InitialPress)
        );
    }

    #[test]
    fn from_api_value_repeat() {
        assert_eq!(
            HueButtonEventType::from_api_value("repeat"),
            Some(HueButtonEventType::Repeat)
        );
    }

    #[test]
    fn from_api_value_short_release() {
        assert_eq!(
            HueButtonEventType::from_api_value("short_release"),
            Some(HueButtonEventType::ShortRelease)
        );
    }

    #[test]
    fn from_api_value_long_release() {
        assert_eq!(
            HueButtonEventType::from_api_value("long_release"),
            Some(HueButtonEventType::LongRelease)
        );
    }

    #[test]
    fn from_api_value_long_press() {
        assert_eq!(
            HueButtonEventType::from_api_value("long_press"),
            Some(HueButtonEventType::LongPress)
        );
    }

    #[test]
    fn from_api_value_unknown() {
        assert_eq!(HueButtonEventType::from_api_value("garbage"), None);
    }

    #[test]
    fn map_button1_initial_press_reset() {
        assert_eq!(
            map_hue_button(1, HueButtonEventType::InitialPress),
            Some(ButtonAction::Reset)
        );
    }

    #[test]
    fn map_button1_long_release_on() {
        assert_eq!(
            map_hue_button(1, HueButtonEventType::LongRelease),
            Some(ButtonAction::OnPress)
        );
    }

    #[test]
    fn map_button2_initial_press_up() {
        assert_eq!(
            map_hue_button(2, HueButtonEventType::InitialPress),
            Some(ButtonAction::UpPress)
        );
    }

    #[test]
    fn map_button2_repeat_up_hold() {
        assert_eq!(
            map_hue_button(2, HueButtonEventType::Repeat),
            Some(ButtonAction::UpHold)
        );
    }

    #[test]
    fn map_button3_initial_press_down() {
        assert_eq!(
            map_hue_button(3, HueButtonEventType::InitialPress),
            Some(ButtonAction::DownPress)
        );
    }

    #[test]
    fn map_button3_repeat_down_hold() {
        assert_eq!(
            map_hue_button(3, HueButtonEventType::Repeat),
            Some(ButtonAction::DownHold)
        );
    }

    #[test]
    fn map_button4_short_release_off() {
        assert_eq!(
            map_hue_button(4, HueButtonEventType::ShortRelease),
            Some(ButtonAction::OffPress)
        );
    }

    #[test]
    fn map_button4_long_press_lights_off() {
        assert_eq!(
            map_hue_button(4, HueButtonEventType::LongPress),
            Some(ButtonAction::LightsOff)
        );
    }

    #[test]
    fn map_unmapped_combo_returns_none() {
        assert_eq!(map_hue_button(1, HueButtonEventType::Repeat), None);
    }

    #[test]
    fn map_invalid_button_index() {
        assert_eq!(map_hue_button(5, HueButtonEventType::InitialPress), None);
    }

    #[test]
    fn map_str_success() {
        assert_eq!(
            map_hue_button_str(1, "initial_press"),
            Some(ButtonAction::Reset)
        );
    }

    #[test]
    fn map_str_invalid_event() {
        assert_eq!(map_hue_button_str(1, "invalid"), None);
    }
}
