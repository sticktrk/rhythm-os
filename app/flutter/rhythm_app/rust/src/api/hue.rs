//! Hue button mapping functions.

use super::dto::{HueButtonEventTypeDto, RhythmActionDto};

/// Map a Hue V2 button event to a rhythm action.
///
/// This function takes raw button event data from the Hue V2 API SSE stream
/// and maps it to a `RhythmActionDto` that the Dart room-state layer can handle.
///
/// # Hue Dimmer Switch (RWL02x) Button Layout
///
/// - Button 1 (top): "On" button
/// - Button 2: "Dim up" button
/// - Button 3: "Dim down" button
/// - Button 4 (bottom): "Off" button
///
/// # Button Mapping
///
/// | Button | Event | Action |
/// |--------|-------|--------|
/// | 1 (On) | short_release | `OnPress` (toggle) |
/// | 2 (Dim Up) | short_release | `DimUp` (brightness only) |
/// | 2 (Dim Up) | repeat | `StepUp` (step along curve) |
/// | 3 (Dim Down) | short_release | `DimDown` (brightness only) |
/// | 3 (Dim Down) | repeat | `StepDown` (step along curve) |
/// | 4 (Off) | short_release | `Reset` |
/// | 4 (Off) | long_release | `RhythmOff` |
///
/// # Arguments
///
/// * `button_index` - Button number (1-4 for Hue Dimmer, from `metadata.control_id`)
/// * `event_type` - The Hue V2 API button event type (from `button.last_event`)
///
/// # Returns
///
/// The corresponding `RhythmActionDto`, or `None` if the event should be ignored.
/// Events like `initial_press` are ignored to avoid duplicate actions.
///
/// # Example
///
/// ```ignore
/// // From Hue SSE event JSON:
/// // { "metadata": {"control_id": 2}, "button": {"last_event": "short_release"} }
///
/// let action = map_hue_button_event(2, HueButtonEventTypeDto::ShortRelease);
/// assert_eq!(action, Some(RhythmActionDto::DimUp));
/// ```
pub fn map_hue_button_event(
    button_index: u8,
    event_type: HueButtonEventTypeDto,
) -> Option<RhythmActionDto> {
    match (button_index, event_type) {
        // Button 1 (On): toggle on short press
        (1, HueButtonEventTypeDto::ShortRelease) => Some(RhythmActionDto::OnPress),

        // Button 2 (Dim Up): brightness only on short, step along curve on hold
        (2, HueButtonEventTypeDto::ShortRelease) => Some(RhythmActionDto::DimUp),
        (2, HueButtonEventTypeDto::Repeat) => Some(RhythmActionDto::StepUp),

        // Button 3 (Dim Down): brightness only on short, step along curve on hold
        (3, HueButtonEventTypeDto::ShortRelease) => Some(RhythmActionDto::DimDown),
        (3, HueButtonEventTypeDto::Repeat) => Some(RhythmActionDto::StepDown),

        // Button 4 (Off): reset on short, rhythm_off on long
        (4, HueButtonEventTypeDto::ShortRelease) => Some(RhythmActionDto::Reset),
        (4, HueButtonEventTypeDto::LongRelease) => Some(RhythmActionDto::RhythmOff),

        // Ignore other events (initial_press, long_press on other buttons, etc.)
        _ => None,
    }
}

/// Parse Hue V2 API event type string to DTO.
///
/// This is a convenience function for parsing the `last_event` field
/// from Hue V2 API button events.
///
/// # Arguments
///
/// * `api_value` - The API string value (e.g., "short_release", "repeat")
///
/// # Returns
///
/// The corresponding `HueButtonEventTypeDto`, or `None` for unknown values.
///
/// # Example
///
/// ```ignore
/// let event_type = parse_hue_button_event_type("short_release".to_string());
/// assert_eq!(event_type, Some(HueButtonEventTypeDto::ShortRelease));
/// ```
pub fn parse_hue_button_event_type(api_value: String) -> Option<HueButtonEventTypeDto> {
    HueButtonEventTypeDto::from_api_value(&api_value)
}
