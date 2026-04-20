//! Device database lookup.

use std::collections::HashMap;

use crate::entry::DeviceEntry;

/// The compiled device database with indexed lookups.
pub struct DeviceDatabase {
    entries: Vec<DeviceEntry>,
    /// Primary index: (manufacturer_lower, model_lower) -> entry index.
    by_model: HashMap<(String, String), usize>,
    /// Secondary index: zigbee model_id -> entry index.
    by_zigbee_model: HashMap<String, usize>,
    /// Tertiary index: alias -> entry index.
    by_alias: HashMap<String, usize>,
    /// Matter index: (vendor_id, product_id) -> entry index.
    by_matter: HashMap<(u16, u16), usize>,
}

impl DeviceDatabase {
    /// Build a database from a list of entries.
    pub fn from_entries(entries: Vec<DeviceEntry>) -> Self {
        let mut by_model = HashMap::new();
        let mut by_zigbee_model = HashMap::new();
        let mut by_alias = HashMap::new();
        let mut by_matter = HashMap::new();

        for (i, entry) in entries.iter().enumerate() {
            by_model.insert(
                (
                    entry.manufacturer.to_lowercase(),
                    entry.model.to_lowercase(),
                ),
                i,
            );

            if let Some(ref zigbee) = entry.zigbee {
                if let Some(ref model_id) = zigbee.model_id {
                    by_zigbee_model.insert(model_id.to_lowercase(), i);
                }
            }

            if let Some(ref matter) = entry.matter {
                if let (Some(vid), Some(pid)) = (matter.vendor_id, matter.product_id) {
                    by_matter.insert((vid, pid), i);
                }
            }

            for alias in &entry.aliases {
                by_alias.insert(alias.to_lowercase(), i);
            }
        }

        Self {
            entries,
            by_model,
            by_zigbee_model,
            by_alias,
            by_matter,
        }
    }

    /// Load the built-in database (compiled into the binary).
    #[cfg(feature = "serde")]
    pub fn builtin() -> Self {
        let entries: Vec<DeviceEntry> = serde_json::from_str(include_str!("../data/devices.json"))
            .expect("built-in devices.json must be valid");
        Self::from_entries(entries)
    }

    /// Look up a device by manufacturer + model.
    pub fn lookup(&self, manufacturer: &str, model: &str) -> Option<&DeviceEntry> {
        let key = (manufacturer.to_lowercase(), model.to_lowercase());
        self.by_model.get(&key).map(|&i| &self.entries[i])
    }

    /// Look up a device by Zigbee model identifier.
    pub fn lookup_zigbee(&self, model_id: &str) -> Option<&DeviceEntry> {
        self.by_zigbee_model
            .get(&model_id.to_lowercase())
            .map(|&i| &self.entries[i])
    }

    /// Look up a device by Matter vendor ID + product ID.
    pub fn lookup_matter(&self, vendor_id: u16, product_id: u16) -> Option<&DeviceEntry> {
        self.by_matter
            .get(&(vendor_id, product_id))
            .map(|&i| &self.entries[i])
    }

    /// Look up a device by alias (EAN, alternate model string, etc.).
    pub fn lookup_alias(&self, alias: &str) -> Option<&DeviceEntry> {
        self.by_alias
            .get(&alias.to_lowercase())
            .map(|&i| &self.entries[i])
    }

    /// Number of devices in the database.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the database is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over all entries.
    pub fn iter(&self) -> impl Iterator<Item = &DeviceEntry> {
        self.entries.iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(feature = "serde")]
    #[test]
    fn test_builtin_db_loads() {
        let db = DeviceDatabase::builtin();
        assert!(!db.is_empty(), "built-in database should not be empty");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_by_model() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup("Signify Netherlands B.V.", "LCT016");
        assert!(entry.is_some(), "LCT016 should be in the database");
        let entry = entry.unwrap();
        assert_eq!(entry.model, "LCT016");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_case_insensitive() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup("signify netherlands b.v.", "lct016");
        assert!(entry.is_some(), "lookup should be case-insensitive");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_by_zigbee_model() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup_zigbee("LCT016");
        assert!(entry.is_some(), "Zigbee model LCT016 should be findable");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_cync_by_model() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup("Savant Systems, Inc.", "93128983");
        assert!(
            entry.is_some(),
            "GE Cync Full Color A19 should be in the database"
        );
        let entry = entry.unwrap();
        assert_eq!(
            entry.light_type,
            crate::capabilities::LightType::ExtendedColor
        );
        assert!(entry.matter.is_some());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_cync_by_matter_id() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup_matter(4921, 171);
        assert!(
            entry.is_some(),
            "GE Cync Full Color A19 Matter IDs should match"
        );
        let entry = entry.unwrap();
        assert_eq!(entry.name, "GE Cync Full Color A19");
        assert_eq!(entry.model, "93128983");
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_h6004_by_matter_id() {
        let db = DeviceDatabase::builtin();
        let entry = db.lookup_matter(4999, 24580);
        assert!(
            entry.is_some(),
            "Shenzhen Qianyan H6004 Matter IDs should match"
        );
        let entry = entry.unwrap();
        assert_eq!(entry.manufacturer, "Shenzhen Qianyan Technology");
        assert_eq!(entry.model, "H6004");
        assert_eq!(
            entry.light_type,
            crate::capabilities::LightType::ExtendedColor
        );
        assert_eq!(
            entry
                .matter
                .as_ref()
                .map(|matter| matter.quirks.clone())
                .unwrap_or_default(),
            vec![
                crate::quirks::DeviceQuirk::NeedsExplicitOn,
                crate::quirks::DeviceQuirk::CommandThrottleMs(250),
            ]
        );
    }

    #[test]
    fn test_lookup_matter_from_entries() {
        use crate::capabilities::{ColorMode, LightType};
        use crate::quirks::MatterDeviceData;

        let entry = DeviceEntry {
            manufacturer: "Test".to_string(),
            model: "T001".to_string(),
            name: "Test Matter Light".to_string(),
            light_type: LightType::ExtendedColor,
            color_modes: vec![ColorMode::Xy, ColorMode::ColorTemperature],
            min_kelvin: Some(2000),
            max_kelvin: Some(7000),
            gamut: None,
            min_brightness: Some(3),
            supports_transition: true,
            aliases: vec![],
            zigbee: None,
            hue_api: None,
            matter: Some(MatterDeviceData {
                vendor_id: Some(0x1384),
                product_id: Some(1),
                quirks: vec![],
            }),
        };
        let db = DeviceDatabase::from_entries(vec![entry]);
        let found = db.lookup_matter(0x1384, 1);
        assert!(found.is_some());
        assert_eq!(found.unwrap().model, "T001");

        // Unknown matter IDs return None
        assert!(db.lookup_matter(0x1384, 99).is_none());
        assert!(db.lookup_matter(0, 0).is_none());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn test_lookup_unknown() {
        let db = DeviceDatabase::builtin();
        assert!(db.lookup("Unknown Corp", "ZZZZZ").is_none());
        assert!(db.lookup_zigbee("NONEXISTENT").is_none());
        assert!(db.lookup_matter(0xFFFF, 0xFFFF).is_none());
    }
}
