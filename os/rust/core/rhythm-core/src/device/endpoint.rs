//! ZigBee endpoint determination by manufacturer.
//!
//! Different ZigBee manufacturers use different endpoints for light control.
//! This module provides functions to determine the correct endpoint.

use super::ieee;

/// ZigBee endpoint for Philips Hue lights.
pub const ENDPOINT_HUE: u8 = 11;

/// ZigBee endpoint for IKEA lights.
pub const ENDPOINT_IKEA: u8 = 1;

/// Default ZigBee endpoint (most common for modern lights).
pub const ENDPOINT_DEFAULT: u8 = 11;

/// Determine the correct endpoint from manufacturer and model strings.
///
/// Different manufacturers use different ZigBee endpoints:
/// - Philips/Signify/Hue: endpoint 11
/// - IKEA: endpoint 1
/// - Others: endpoint 11 (most common default)
///
/// # Examples
///
/// ```
/// use rhythm_core::device::endpoint::for_manufacturer;
///
/// assert_eq!(for_manufacturer("Signify Netherlands B.V.", "LCT015"), 11);
/// assert_eq!(for_manufacturer("IKEA of Sweden", "TRADFRI bulb"), 1);
/// assert_eq!(for_manufacturer("Unknown", "Unknown"), 11);
/// ```
pub fn for_manufacturer(manufacturer: &str, model: &str) -> u8 {
    let manufacturer_lower = manufacturer.to_lowercase();
    let model_lower = model.to_lowercase();

    if manufacturer_lower.contains("signify")
        || manufacturer_lower.contains("philips")
        || model_lower.contains("hue")
        || model_lower.contains("signify")
    {
        ENDPOINT_HUE
    } else if manufacturer_lower.contains("ikea") {
        ENDPOINT_IKEA
    } else {
        ENDPOINT_DEFAULT
    }
}

/// Determine the endpoint from an IEEE address using OUI lookup.
///
/// Uses the IEEE address OUI prefix to identify the manufacturer:
/// - Philips Hue OUI (00:17:88:01:09): endpoint 11
/// - Others: default endpoint 11
///
/// Note: This is less accurate than `for_manufacturer` since it only
/// works for manufacturers with known OUI prefixes. Prefer using
/// `for_manufacturer` when manufacturer/model info is available.
///
/// # Examples
///
/// ```
/// use rhythm_core::device::endpoint::for_ieee;
///
/// assert_eq!(for_ieee("00:17:88:01:09:AB:CD:EF"), 11); // Hue
/// assert_eq!(for_ieee("00:11:22:33:44:55:66:77"), 11); // Unknown, default
/// ```
pub fn for_ieee(ieee_address: &str) -> u8 {
    if ieee::is_hue(ieee_address) {
        ENDPOINT_HUE
    } else {
        // TODO: Add more OUI lookups as needed (IKEA, etc.)
        ENDPOINT_DEFAULT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_for_manufacturer_hue() {
        assert_eq!(
            for_manufacturer("Signify Netherlands B.V.", "LCT015"),
            ENDPOINT_HUE
        );
        assert_eq!(for_manufacturer("Philips", "Hue White"), ENDPOINT_HUE);
        assert_eq!(for_manufacturer("Unknown", "Hue bulb"), ENDPOINT_HUE);
        assert_eq!(for_manufacturer("SIGNIFY", "whatever"), ENDPOINT_HUE);
    }

    #[test]
    fn test_for_manufacturer_ikea() {
        assert_eq!(
            for_manufacturer("IKEA of Sweden", "TRADFRI bulb"),
            ENDPOINT_IKEA
        );
        assert_eq!(for_manufacturer("ikea", "some light"), ENDPOINT_IKEA);
    }

    #[test]
    fn test_for_manufacturer_default() {
        assert_eq!(for_manufacturer("Unknown", "Unknown"), ENDPOINT_DEFAULT);
        assert_eq!(for_manufacturer("", ""), ENDPOINT_DEFAULT);
        assert_eq!(for_manufacturer("Sengled", "E11-G13"), ENDPOINT_DEFAULT);
    }

    #[test]
    fn test_for_ieee() {
        // Hue IEEE
        assert_eq!(for_ieee("00:17:88:01:09:AB:CD:EF"), ENDPOINT_HUE);

        // Non-Hue IEEE
        assert_eq!(for_ieee("00:11:22:33:44:55:66:77"), ENDPOINT_DEFAULT);
    }
}
