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
}

impl DeviceDatabase {
    /// Build a database from a list of entries.
    pub fn from_entries(entries: Vec<DeviceEntry>) -> Self {
        let mut by_model = HashMap::new();
        let mut by_zigbee_model = HashMap::new();
        let mut by_alias = HashMap::new();

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

            for alias in &entry.aliases {
                by_alias.insert(alias.to_lowercase(), i);
            }
        }

        Self {
            entries,
            by_model,
            by_zigbee_model,
            by_alias,
        }
    }

    /// Load the built-in database (compiled into the binary).
    #[cfg(feature = "serde")]
    pub fn builtin() -> Self {
        let entries: Vec<DeviceEntry> =
            serde_json::from_str(include_str!("../data/devices.json"))
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
    fn test_lookup_unknown() {
        let db = DeviceDatabase::builtin();
        assert!(db.lookup("Unknown Corp", "ZZZZZ").is_none());
        assert!(db.lookup_zigbee("NONEXISTENT").is_none());
    }
}
