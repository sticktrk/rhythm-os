//! Platform-agnostic input events.
//!
//! This module defines the event types that can trigger rhythm actions,
//! abstracting away the specific event sources (ZHA, custom integration, HTTP API, etc.)

#[cfg(feature = "serde")]
use serde::{Deserialize, Serialize};

// ============================================================================
// ZHA Event Types
// ============================================================================

/// ZHA event arguments for step/move commands.
///
/// Some ZHA commands (like step_with_on_off and move_with_on_off) include
/// additional arguments that determine the direction (up/down).
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct ZhaEventArgs {
    /// Step mode for step_with_on_off: 0 = up, 1 = down
    #[cfg_attr(feature = "serde", serde(default))]
    pub step_mode: Option<u8>,

    /// Move mode for move_with_on_off: 0 = up, 1 = down
    #[cfg_attr(feature = "serde", serde(default))]
    pub move_mode: Option<u8>,

    /// Step size (optional, for step commands)
    #[cfg_attr(feature = "serde", serde(default))]
    pub step_size: Option<u8>,

    /// Transition time (optional, for move commands)
    #[cfg_attr(feature = "serde", serde(default))]
    pub transition_time: Option<u16>,
}

/// Platform-agnostic button/remote actions.
///
/// These actions map to the service primitives in RhythmEngine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum ButtonAction {
    /// Turn on lights with adaptive values (maps to turn_on)
    OnPress,

    /// Toggle lights on/off (always adaptive, maps to toggle)
    Toggle,

    /// Turn off lights (rhythm state unchanged, maps to turn_off)
    OffPress,

    /// Reset to current time position (maps to reset)
    Reset,

    /// Dim up (brightness only)
    UpPress,

    /// Dim down (brightness only)
    DownPress,

    /// Step up (brighten and cool) along the curve
    UpHold,

    /// Step down (dim and warm) along the curve
    DownHold,

    /// Stop continuous dimming
    Stop,

    /// Enable rhythm timer participation (no light commands, maps to rhythm_on)
    RhythmOn,

    /// Disable rhythm timer participation (no light commands, maps to rhythm_off)
    RhythmOff,

    /// Turn lights fully off (bypasses soft-off, maps to lights_off)
    LightsOff,
}

impl ButtonAction {
    /// Convert from a ZHA command string.
    ///
    /// Common Hue dimmer switch commands:
    /// - on_press / on_short_release -> OnPress (turn on)
    /// - off_press / off_short_release -> Reset
    /// - up_press / up_short_release -> UpPress (dim up, brightness only)
    /// - down_press / down_short_release -> DownPress (dim down, brightness only)
    /// - up_hold -> UpHold (step up along curve)
    /// - down_hold -> DownHold (step down along curve)
    /// - stop -> Stop
    pub fn from_zha_command(command: &str) -> Option<Self> {
        match command {
            "on_press" | "on" => Some(Self::OnPress),
            "off_press" | "off" => Some(Self::Reset),
            "up_press" | "up" => Some(Self::UpPress),
            "down_press" | "down" => Some(Self::DownPress),
            "up_hold" => Some(Self::UpHold),
            "down_hold" => Some(Self::DownHold),
            "stop" | "stop_with_on_off" => Some(Self::Stop),
            // Ignore release events to avoid double-triggering
            "on_short_release" | "off_short_release" | "up_short_release"
            | "down_short_release" => None,
            _ => None,
        }
    }

    /// Convert from a ZHA event with command and optional args.
    ///
    /// This method handles special ZHA commands that need args to determine
    /// direction (step_with_on_off and move_with_on_off). For other commands,
    /// it delegates to `from_zha_command`.
    ///
    /// # Arguments
    ///
    /// * `command` - The ZHA command string
    /// * `args` - Optional ZhaEventArgs containing step_mode/move_mode
    ///
    /// # Examples
    ///
    /// ```
    /// use rhythm_core::runtime::events::{ButtonAction, ZhaEventArgs};
    ///
    /// // step_with_on_off with step_mode 0 = up
    /// let args = ZhaEventArgs { step_mode: Some(0), ..Default::default() };
    /// assert_eq!(
    ///     ButtonAction::from_zha_event("step_with_on_off", Some(&args)),
    ///     Some(ButtonAction::UpPress)
    /// );
    ///
    /// // step_with_on_off with step_mode 1 = down
    /// let args = ZhaEventArgs { step_mode: Some(1), ..Default::default() };
    /// assert_eq!(
    ///     ButtonAction::from_zha_event("step_with_on_off", Some(&args)),
    ///     Some(ButtonAction::DownPress)
    /// );
    ///
    /// // Regular command without args
    /// assert_eq!(
    ///     ButtonAction::from_zha_event("on_press", None),
    ///     Some(ButtonAction::OnPress)
    /// );
    /// ```
    pub fn from_zha_event(command: &str, args: Option<&ZhaEventArgs>) -> Option<Self> {
        match command {
            "step_with_on_off" => {
                // step_mode 0 = up, 1 = down (default to down if missing)
                let step_mode = args.and_then(|a| a.step_mode).unwrap_or(1);
                if step_mode == 0 {
                    Some(Self::UpPress)
                } else {
                    Some(Self::DownPress)
                }
            }
            "move_with_on_off" => {
                // move_mode 0 = up, 1 = down (default to down if missing)
                let move_mode = args.and_then(|a| a.move_mode).unwrap_or(1);
                if move_mode == 0 {
                    Some(Self::UpHold)
                } else {
                    Some(Self::DownHold)
                }
            }
            _ => Self::from_zha_command(command),
        }
    }

    /// Convert from a service name.
    ///
    /// Service names from the custom integration:
    /// - rhythm_on, rhythm_off, rhythm_toggle
    /// - step_up, step_down, dim_up, dim_down
    /// - reset, lights_off
    pub fn from_service_name(service: &str) -> Option<Self> {
        match service {
            "rhythm_on" => Some(Self::RhythmOn),
            "rhythm_off" => Some(Self::RhythmOff),
            "rhythm_toggle" => Some(Self::Toggle),
            "step_up" => Some(Self::UpHold),
            "step_down" => Some(Self::DownHold),
            "dim_up" => Some(Self::UpPress),
            "dim_down" => Some(Self::DownPress),
            "reset" => Some(Self::Reset),
            "lights_off" => Some(Self::LightsOff),
            _ => None,
        }
    }
}

/// An input event from any source (button, service call, HTTP API).
#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(Serialize, Deserialize))]
pub struct InputEvent {
    /// The room/area ID this event applies to.
    pub room_id: String,

    /// The action to perform.
    pub action: ButtonAction,

    /// Optional device ID that triggered the event (for logging/debugging).
    pub device_id: Option<String>,
}

impl InputEvent {
    /// Create a new input event.
    pub fn new(room_id: impl Into<String>, action: ButtonAction) -> Self {
        Self {
            room_id: room_id.into(),
            action,
            device_id: None,
        }
    }

    /// Create a new input event with device ID.
    pub fn with_device(
        room_id: impl Into<String>,
        action: ButtonAction,
        device_id: impl Into<String>,
    ) -> Self {
        Self {
            room_id: room_id.into(),
            action,
            device_id: Some(device_id.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_button_action_from_zha() {
        assert_eq!(
            ButtonAction::from_zha_command("on_press"),
            Some(ButtonAction::OnPress)
        );
        assert_eq!(
            ButtonAction::from_zha_command("up_press"),
            Some(ButtonAction::UpPress)
        );
        assert_eq!(
            ButtonAction::from_zha_command("down_hold"),
            Some(ButtonAction::DownHold)
        );
        // Release events should be ignored
        assert_eq!(ButtonAction::from_zha_command("on_short_release"), None);
    }

    #[test]
    fn test_button_action_from_zha_event() {
        // step_with_on_off
        let args_up = ZhaEventArgs {
            step_mode: Some(0),
            ..Default::default()
        };
        assert_eq!(
            ButtonAction::from_zha_event("step_with_on_off", Some(&args_up)),
            Some(ButtonAction::UpPress)
        );

        let args_down = ZhaEventArgs {
            step_mode: Some(1),
            ..Default::default()
        };
        assert_eq!(
            ButtonAction::from_zha_event("step_with_on_off", Some(&args_down)),
            Some(ButtonAction::DownPress)
        );

        // move_with_on_off
        let args_move_up = ZhaEventArgs {
            move_mode: Some(0),
            ..Default::default()
        };
        assert_eq!(
            ButtonAction::from_zha_event("move_with_on_off", Some(&args_move_up)),
            Some(ButtonAction::UpHold)
        );

        let args_move_down = ZhaEventArgs {
            move_mode: Some(1),
            ..Default::default()
        };
        assert_eq!(
            ButtonAction::from_zha_event("move_with_on_off", Some(&args_move_down)),
            Some(ButtonAction::DownHold)
        );

        // Default behavior without args (should default to down)
        assert_eq!(
            ButtonAction::from_zha_event("step_with_on_off", None),
            Some(ButtonAction::DownPress)
        );
        assert_eq!(
            ButtonAction::from_zha_event("move_with_on_off", None),
            Some(ButtonAction::DownHold)
        );

        // Regular commands delegate to from_zha_command
        assert_eq!(
            ButtonAction::from_zha_event("on_press", None),
            Some(ButtonAction::OnPress)
        );
        assert_eq!(
            ButtonAction::from_zha_event("off_press", None),
            Some(ButtonAction::Reset)
        );
    }

    #[test]
    fn test_button_action_from_service() {
        assert_eq!(
            ButtonAction::from_service_name("rhythm_on"),
            Some(ButtonAction::RhythmOn)
        );
        assert_eq!(
            ButtonAction::from_service_name("step_up"),
            Some(ButtonAction::UpHold)
        );
        assert_eq!(
            ButtonAction::from_service_name("step_down"),
            Some(ButtonAction::DownHold)
        );
        assert_eq!(
            ButtonAction::from_service_name("dim_up"),
            Some(ButtonAction::UpPress)
        );
        assert_eq!(
            ButtonAction::from_service_name("dim_down"),
            Some(ButtonAction::DownPress)
        );
        assert_eq!(
            ButtonAction::from_service_name("lights_off"),
            Some(ButtonAction::LightsOff)
        );
    }

    #[test]
    fn test_input_event() {
        let event = InputEvent::new("living_room", ButtonAction::OnPress);
        assert_eq!(event.room_id, "living_room");
        assert_eq!(event.action, ButtonAction::OnPress);
        assert!(event.device_id.is_none());

        let event_with_device =
            InputEvent::with_device("bedroom", ButtonAction::UpPress, "00:11:22:33:44:55");
        assert_eq!(
            event_with_device.device_id,
            Some("00:11:22:33:44:55".into())
        );
    }

    #[test]
    fn test_all_service_name_mappings() {
        // Exhaustive test of all service names
        let mappings = [
            ("rhythm_on", ButtonAction::RhythmOn),
            ("rhythm_off", ButtonAction::RhythmOff),
            ("rhythm_toggle", ButtonAction::Toggle),
            ("step_up", ButtonAction::UpHold),
            ("step_down", ButtonAction::DownHold),
            ("dim_up", ButtonAction::UpPress),
            ("dim_down", ButtonAction::DownPress),
            ("reset", ButtonAction::Reset),
            ("lights_off", ButtonAction::LightsOff),
        ];

        for (service, expected) in &mappings {
            assert_eq!(
                ButtonAction::from_service_name(service),
                Some(*expected),
                "Service '{}' should map to {:?}",
                service,
                expected
            );
        }
    }

    #[test]
    fn test_unknown_service_name() {
        assert_eq!(ButtonAction::from_service_name("unknown_service"), None);
        assert_eq!(ButtonAction::from_service_name(""), None);
        assert_eq!(ButtonAction::from_service_name("turn_on"), None);
    }

    #[test]
    fn test_all_zha_command_mappings() {
        let mappings = [
            ("on_press", Some(ButtonAction::OnPress)),
            ("on", Some(ButtonAction::OnPress)),
            ("off_press", Some(ButtonAction::Reset)),
            ("off", Some(ButtonAction::Reset)),
            ("up_press", Some(ButtonAction::UpPress)),
            ("up", Some(ButtonAction::UpPress)),
            ("down_press", Some(ButtonAction::DownPress)),
            ("down", Some(ButtonAction::DownPress)),
            ("up_hold", Some(ButtonAction::UpHold)),
            ("down_hold", Some(ButtonAction::DownHold)),
            ("stop", Some(ButtonAction::Stop)),
            ("stop_with_on_off", Some(ButtonAction::Stop)),
            // Release events should be ignored
            ("on_short_release", None),
            ("off_short_release", None),
            ("up_short_release", None),
            ("down_short_release", None),
            // Unknown
            ("random_command", None),
        ];

        for (cmd, expected) in &mappings {
            assert_eq!(
                ButtonAction::from_zha_command(cmd),
                *expected,
                "ZHA command '{}' should map to {:?}",
                cmd,
                expected
            );
        }
    }

    #[test]
    fn test_button_action_debug_format() {
        let action = ButtonAction::OnPress;
        let debug_str = format!("{:?}", action);
        assert_eq!(debug_str, "OnPress");
    }

    #[test]
    fn test_input_event_debug_format() {
        let event = InputEvent::new("room1", ButtonAction::Toggle);
        let debug_str = format!("{:?}", event);
        assert!(debug_str.contains("room1"));
        assert!(debug_str.contains("Toggle"));
    }

    #[test]
    fn test_button_action_eq_and_hash() {
        use std::collections::HashSet;
        let mut set = HashSet::new();
        set.insert(ButtonAction::OnPress);
        set.insert(ButtonAction::OffPress);
        set.insert(ButtonAction::OnPress); // duplicate
        assert_eq!(set.len(), 2);
    }
}
