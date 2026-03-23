//! Hue button event mapping — re-exports from rhythm-os.
//!
//! The canonical implementation now lives in `rhythm_os::hue_buttons` so
//! both `rhythm-hue` and `rhythm-ha` can share the same mapping without
//! depending on each other. This module re-exports for backward compatibility.

pub use rhythm_os::hue_buttons::{map_hue_button, map_hue_button_str, HueButtonEventType};
