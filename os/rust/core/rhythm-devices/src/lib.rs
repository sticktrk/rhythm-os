//! Device capability database for Rhythm OS.
//!
//! Provides a structured database of known light device models with their
//! capabilities (color modes, kelvin range, gamut) and protocol-specific
//! quirks (Zigbee endpoints, Hue API workarounds).
//!
//! # Usage
//!
//! ```
//! use rhythm_devices::{builtin_db, LightCapabilities, LightType};
//!
//! let db = builtin_db();
//!
//! // Look up a known device
//! if let Some(entry) = db.lookup("Signify Netherlands B.V.", "LCT016") {
//!     let caps = entry.capabilities();
//!     assert!(caps.supports_xy_color());
//!     assert_eq!(caps.min_kelvin, Some(2000));
//! }
//!
//! // Fall back to defaults for unknown devices
//! let caps = LightCapabilities::defaults_for(LightType::ColorTemperature);
//! assert!(caps.supports_color_temp());
//! ```

pub mod adapt;
pub mod capabilities;
pub mod correction;
pub mod entry;
pub mod gamut;
pub mod lookup;
pub mod quirks;

pub use adapt::{adapt_command, AdaptedCommand, ColorPreference, ColorRequest};
pub use capabilities::{ColorMode, LightCapabilities, LightType};
pub use correction::{
    BrightnessCorrectionPoint, ColorTemperatureCorrectionPoint, ControlCorrections,
};
pub use entry::DeviceEntry;
pub use gamut::{GamutTriangle, XyPoint};
pub use lookup::DeviceDatabase;
pub use quirks::{DeviceQuirk, HueApiData, MatterDeviceData, ZigbeeDeviceData};

use std::sync::OnceLock;

static BUILTIN_DB: OnceLock<DeviceDatabase> = OnceLock::new();

/// Get the built-in device database (singleton, parsed once).
#[cfg(feature = "serde")]
pub fn builtin_db() -> &'static DeviceDatabase {
    BUILTIN_DB.get_or_init(DeviceDatabase::builtin)
}
