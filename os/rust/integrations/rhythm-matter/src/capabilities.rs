//! Matter → rhythm-devices capability conversion.
//!
//! Converts Matter-specific capability data (discovered during commissioning)
//! into the protocol-agnostic [`LightCapabilities`] and device metadata used by
//! `rhythm-devices`.

use rhythm_devices::{
    ColorMode, DeviceDatabase, DeviceEntry, DeviceQuirk, LightCapabilities, LightType,
};

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
        control_corrections: rhythm_devices::ControlCorrections::default(),
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
pub fn lookup_db_entry<'a>(
    device: &CommissionedDevice,
    db: &'a DeviceDatabase,
) -> Option<&'a DeviceEntry> {
    db.lookup_matter(device.vendor_id, device.product_id)
        .or_else(|| db.lookup(&device.vendor_name, &device.product_name))
}

pub fn enrich_from_db(
    caps: &mut LightCapabilities,
    device: &CommissionedDevice,
    db: &DeviceDatabase,
) {
    if let Some(entry) = lookup_db_entry(device, db) {
        if caps.gamut.is_none() {
            caps.gamut = entry.gamut.clone();
        }
        if caps.min_brightness.is_none() {
            caps.min_brightness = entry.min_brightness;
        }
        if caps.control_corrections.is_empty() {
            caps.control_corrections = entry.control_corrections.clone();
        }
        // A measurement-derived correction profile makes the database's
        // logical CT envelope authoritative. The Matter cluster range still
        // describes accepted device commands; the correction curve may map a
        // logical endpoint to a command outside this narrower logical range.
        if !entry.control_corrections.color_temperature.is_empty() {
            caps.min_kelvin = entry.min_kelvin.or(caps.min_kelvin);
            caps.max_kelvin = entry.max_kelvin.or(caps.max_kelvin);
        }

        // A NeedsHueSaturationNotCt profile declares that the product renders
        // logical white points through its hue/saturation path because its
        // native CT path is unreliable. When that reviewed profile also has a
        // measured Kelvin envelope, use it instead of an absent or invalid
        // Matter cluster range so the logical CT surface survives restart.
        let entry_capabilities = entry.capabilities();
        let needs_profiled_hue_saturation_ct = entry.matter.as_ref().is_some_and(|matter| {
            matter
                .quirks
                .contains(&DeviceQuirk::NeedsHueSaturationNotCt)
        }) && caps.supports_hue_saturation()
            && caps.supports_color_temp()
            && entry_capabilities.supports_hue_saturation()
            && entry_capabilities.supports_color_temp();
        if needs_profiled_hue_saturation_ct {
            if let (Some(min_kelvin), Some(max_kelvin)) = (entry.min_kelvin, entry.max_kelvin) {
                if min_kelvin > 0 && min_kelvin <= max_kelvin {
                    caps.min_kelvin = Some(min_kelvin);
                    caps.max_kelvin = Some(max_kelvin);
                }
            }
        }

        // Some Matter bulbs omit XY from their descriptor even though XY is
        // the only reliable color path they implement. A NeedsXyNotCt profile
        // is therefore also authoritative capability evidence; without this
        // enrichment the preference cannot ever select XY and silently falls
        // back to the known-broken color-temperature command.
        let needs_xy = entry
            .matter
            .as_ref()
            .is_some_and(|matter| matter.quirks.contains(&DeviceQuirk::NeedsXyNotCt));
        if needs_xy
            && entry_capabilities.supports_xy_color()
            && !caps.color_modes.contains(&ColorMode::Xy)
        {
            caps.color_modes.push(ColorMode::Xy);
            caps.light_type = LightType::ExtendedColor;
        }
    }
}

/// Look up Matter-specific quirks for a commissioned device.
pub fn quirks_from_db(device: &CommissionedDevice, db: &DeviceDatabase) -> Vec<DeviceQuirk> {
    lookup_db_entry(device, db)
        .and_then(|entry| entry.matter.as_ref())
        .map(|matter| matter.quirks.clone())
        .unwrap_or_default()
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
            red: XyPoint {
                x: 0.6915,
                y: 0.3083,
            },
            green: XyPoint { x: 0.17, y: 0.7 },
            blue: XyPoint {
                x: 0.1532,
                y: 0.0475,
            },
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
            control_corrections: rhythm_devices::ControlCorrections::default(),
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

    #[test]
    fn profiled_hue_saturation_ct_uses_verified_logical_range() {
        let device = CommissionedDevice {
            node_id: 116,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H7056".to_string(),
            vendor_id: 4999,
            product_id: 28758,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        };

        let mut caps = capabilities_from_commissioned(&device);
        enrich_from_db(&mut caps, &device, rhythm_devices::builtin_db());

        assert!(caps.supports_hue_saturation());
        assert!(caps.supports_color_temp());
        assert_eq!(caps.min_kelvin, Some(3080));
        assert_eq!(caps.max_kelvin, Some(6120));
    }

    #[test]
    fn measured_profile_enriches_corrections_and_logical_ct_envelope() {
        use rhythm_devices::{
            BrightnessCorrectionPoint, ColorTemperatureCorrectionPoint, ControlCorrections,
            DeviceDatabase, DeviceEntry,
        };

        let device = CommissionedDevice {
            node_id: 7,
            vendor_name: "BudgetCo".to_string(),
            product_name: "Matter A19".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2200),
            max_kelvin: Some(7000),
        };
        let db = DeviceDatabase::from_entries(vec![DeviceEntry {
            manufacturer: "BudgetCo".to_string(),
            model: "Matter A19".to_string(),
            name: "BudgetCo Matter A19".to_string(),
            light_type: LightType::ColorTemperature,
            color_modes: vec![ColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
            gamut: None,
            min_brightness: Some(7),
            supports_transition: true,
            control_corrections: ControlCorrections {
                brightness: vec![
                    BrightnessCorrectionPoint {
                        logical_percent: 1,
                        command_percent: 7,
                    },
                    BrightnessCorrectionPoint {
                        logical_percent: 100,
                        command_percent: 100,
                    },
                ],
                color_temperature: vec![
                    ColorTemperatureCorrectionPoint {
                        logical_kelvin: 2700,
                        command_kelvin: 2400,
                    },
                    ColorTemperatureCorrectionPoint {
                        logical_kelvin: 6500,
                        command_kelvin: 6200,
                    },
                ],
            },
            aliases: vec![],
            zigbee: None,
            hue_api: None,
            matter: None,
        }]);

        let mut caps = capabilities_from_commissioned(&device);
        enrich_from_db(&mut caps, &device, &db);

        assert_eq!(caps.min_brightness, Some(7));
        assert_eq!(caps.min_kelvin, Some(2700));
        assert_eq!(caps.max_kelvin, Some(6500));
        assert_eq!(caps.control_corrections.map_brightness(1), 7);
        assert_eq!(caps.control_corrections.map_color_temperature(2700), 2400);
    }

    #[test]
    fn sengled_profile_uses_advertised_hue_saturation_instead_of_inventing_xy() {
        let device = CommissionedDevice {
            node_id: 104,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            vendor_id: 4448,
            product_id: 36866,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        };

        let mut caps = capabilities_from_commissioned(&device);
        assert!(!caps.supports_xy_color());

        enrich_from_db(&mut caps, &device, rhythm_devices::builtin_db());

        assert!(!caps.supports_xy_color());
        assert!(caps.supports_hue_saturation());
        assert_eq!(
            quirks_from_db(&device, rhythm_devices::builtin_db()),
            vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ]
        );
    }

    #[test]
    fn quirks_use_model_lookup_when_matter_ids_are_unknown() {
        use rhythm_devices::{DeviceDatabase, DeviceEntry, MatterDeviceData};

        let device = CommissionedDevice {
            node_id: 99,
            vendor_name: "TestCo".to_string(),
            product_name: "Quirky Bulb".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        };

        let db = DeviceDatabase::from_entries(vec![DeviceEntry {
            manufacturer: "TestCo".to_string(),
            model: "Quirky Bulb".to_string(),
            name: "Test Matter Light".to_string(),
            light_type: LightType::ColorTemperature,
            color_modes: vec![ColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
            gamut: None,
            min_brightness: None,
            supports_transition: true,
            control_corrections: rhythm_devices::ControlCorrections::default(),
            aliases: vec![],
            zigbee: None,
            hue_api: None,
            matter: Some(MatterDeviceData {
                vendor_id: None,
                product_id: None,
                quirks: vec![DeviceQuirk::NeedsExplicitOn],
            }),
        }]);

        assert_eq!(
            quirks_from_db(&device, &db),
            vec![DeviceQuirk::NeedsExplicitOn]
        );
    }
}
