//! Matter → rhythm-devices capability conversion.
//!
//! Converts Matter-specific capability data (discovered during commissioning)
//! into the protocol-agnostic [`LightCapabilities`] used by `rhythm-devices`.

use rhythm_devices::{ColorMode, LightCapabilities, LightType};

use crate::transport::{CommissionedDevice, MatterColorMode};

impl From<MatterColorMode> for ColorMode {
    fn from(mode: MatterColorMode) -> Self {
        match mode {
            MatterColorMode::HueSaturation => ColorMode::HueSaturation,
            MatterColorMode::Xy => ColorMode::Xy,
            MatterColorMode::ColorTemperature => ColorMode::ColorTemperature,
        }
    }
}

/// Build [`LightCapabilities`] from a commissioned Matter device.
///
/// Uses the color modes and kelvin range discovered during commissioning
/// (via Color Control cluster probing) to construct capabilities.
pub fn capabilities_from_commissioned(device: &CommissionedDevice) -> LightCapabilities {
    let color_modes: Vec<ColorMode> = device.color_modes.iter().map(|m| (*m).into()).collect();

    let light_type = infer_light_type(&color_modes);

    LightCapabilities {
        light_type,
        color_modes,
        min_kelvin: device.min_kelvin,
        max_kelvin: device.max_kelvin,
        gamut: None, // Matter doesn't report gamut; can be enriched from DB later
        min_brightness: None, // Not reported by Matter; can be enriched from DB later
        supports_transition: true, // Matter Level Control supports transitions
    }
}

/// Infer [`LightType`] from the set of supported color modes.
fn infer_light_type(color_modes: &[ColorMode]) -> LightType {
    let has_xy = color_modes.contains(&ColorMode::Xy);
    let has_hs = color_modes.contains(&ColorMode::HueSaturation);
    let has_ct = color_modes.contains(&ColorMode::ColorTemperature);

    if has_xy || has_hs {
        LightType::ExtendedColor
    } else if has_ct {
        LightType::ColorTemperature
    } else {
        // No color capabilities — at minimum it's dimmable (Matter Level Control)
        LightType::Dimmable
    }
}

/// Enrich Matter-probed capabilities with data from the device database.
///
/// Matter commissioning discovers color modes and kelvin range but not gamut
/// or min_brightness. This looks up the device by (vendor_id, product_id) or
/// (vendor_name, product_name) and fills in fields the DB knows about.
pub fn enrich_from_db(
    caps: &mut LightCapabilities,
    device: &CommissionedDevice,
    db: &rhythm_devices::DeviceDatabase,
) {
    let entry = db
        .lookup_matter(device.vendor_id, device.product_id)
        .or_else(|| db.lookup(&device.vendor_name, &device.product_name));

    if let Some(entry) = entry {
        if caps.gamut.is_none() {
            caps.gamut = entry.gamut.clone();
        }
        if caps.min_brightness.is_none() {
            caps.min_brightness = entry.min_brightness;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matter_color_mode_to_color_mode() {
        assert_eq!(
            ColorMode::from(MatterColorMode::HueSaturation),
            ColorMode::HueSaturation
        );
        assert_eq!(ColorMode::from(MatterColorMode::Xy), ColorMode::Xy);
        assert_eq!(
            ColorMode::from(MatterColorMode::ColorTemperature),
            ColorMode::ColorTemperature
        );
    }

    #[test]
    fn commissioned_extended_color() {
        let device = CommissionedDevice {
            node_id: 1,
            vendor_name: "Test".to_string(),
            product_name: "Color Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::Xy, MatterColorMode::ColorTemperature],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        };

        let caps = capabilities_from_commissioned(&device);
        assert_eq!(caps.light_type, LightType::ExtendedColor);
        assert!(caps.supports_color_temp());
        assert!(caps.supports_xy_color());
        assert_eq!(caps.min_kelvin, Some(2000));
        assert_eq!(caps.max_kelvin, Some(6500));
    }

    #[test]
    fn commissioned_color_temp_only() {
        let device = CommissionedDevice {
            node_id: 2,
            vendor_name: "Test".to_string(),
            product_name: "CT Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        };

        let caps = capabilities_from_commissioned(&device);
        assert_eq!(caps.light_type, LightType::ColorTemperature);
        assert!(caps.supports_color_temp());
        assert!(!caps.supports_xy_color());
    }

    #[test]
    fn commissioned_dimmable_only() {
        let device = CommissionedDevice {
            node_id: 3,
            vendor_name: "Test".to_string(),
            product_name: "Dim Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![],
            min_kelvin: None,
            max_kelvin: None,
        };

        let caps = capabilities_from_commissioned(&device);
        assert_eq!(caps.light_type, LightType::Dimmable);
        assert!(!caps.supports_color_temp());
        assert!(!caps.supports_xy_color());
        assert!(caps.supports_dimming());
    }

    #[test]
    fn commissioned_hue_saturation() {
        let device = CommissionedDevice {
            node_id: 4,
            vendor_name: "Test".to_string(),
            product_name: "HS Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::HueSaturation],
            min_kelvin: None,
            max_kelvin: None,
        };

        let caps = capabilities_from_commissioned(&device);
        assert_eq!(caps.light_type, LightType::ExtendedColor);
    }

    #[test]
    fn enrich_fills_db_fields() {
        use rhythm_devices::{DeviceDatabase, DeviceEntry, GamutTriangle, XyPoint};

        let device = CommissionedDevice {
            node_id: 1,
            vendor_name: "TestCo".to_string(),
            product_name: "Color Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::Xy, MatterColorMode::ColorTemperature],
            min_kelvin: Some(2000),
            max_kelvin: Some(7000),
        };

        let mut caps = capabilities_from_commissioned(&device);
        assert_eq!(caps.gamut, None);
        assert_eq!(caps.min_brightness, None);

        let gamut = GamutTriangle {
            red: XyPoint { x: 0.6915, y: 0.3083 },
            green: XyPoint { x: 0.17, y: 0.7 },
            blue: XyPoint { x: 0.1532, y: 0.0475 },
        };
        let entry = DeviceEntry {
            manufacturer: "TestCo".to_string(),
            model: "Color Bulb".to_string(),
            name: "Test Color Bulb".to_string(),
            light_type: LightType::ExtendedColor,
            color_modes: vec![ColorMode::Xy, ColorMode::ColorTemperature],
            min_kelvin: Some(2000),
            max_kelvin: Some(7000),
            gamut: Some(gamut.clone()),
            min_brightness: Some(3),
            supports_transition: true,
            aliases: vec![],
            zigbee: None,
            hue_api: None,
            matter: None,
        };
        let db = DeviceDatabase::from_entries(vec![entry]);

        enrich_from_db(&mut caps, &device, &db);
        assert_eq!(caps.gamut, Some(gamut));
        assert_eq!(caps.min_brightness, Some(3));
        // Probed values are NOT overwritten
        assert_eq!(caps.min_kelvin, Some(2000));
        assert_eq!(caps.max_kelvin, Some(7000));
    }

    #[test]
    fn enrich_noop_for_unknown_device() {
        use rhythm_devices::DeviceDatabase;

        let device = CommissionedDevice {
            node_id: 99,
            vendor_name: "Unknown".to_string(),
            product_name: "Mystery".to_string(),
            vendor_id: 0xFFFF,
            product_id: 0xFFFF,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        };

        let mut caps = capabilities_from_commissioned(&device);
        let db = DeviceDatabase::from_entries(vec![]);

        enrich_from_db(&mut caps, &device, &db);
        // Nothing changed
        assert_eq!(caps.gamut, None);
        assert_eq!(caps.min_brightness, None);
        assert_eq!(caps.min_kelvin, Some(2700));
    }
}
