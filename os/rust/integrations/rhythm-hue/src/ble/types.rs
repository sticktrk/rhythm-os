//! Typed Hue BLE device, state, pairing, and command models.

use serde::{Deserialize, Serialize};

use super::protocol::HueBleEffect;

/// Hue BLE features discovered from the bulb's GATT characteristics.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HueBleCapabilities {
    pub dimming: bool,
    pub color_temperature: bool,
    pub xy_color: bool,
    pub combined_control: bool,
    pub effects: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_mired: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_mired: Option<u16>,
}

impl HueBleCapabilities {
    /// Build capabilities from the vendor GATT surface, enriching ranges with
    /// the model database when Hue reports a wider-than-legacy CT envelope.
    pub fn from_gatt(
        manufacturer: &str,
        model: &str,
        dimming: bool,
        color_temperature: bool,
        xy_color: bool,
        combined_control: bool,
    ) -> Self {
        let (min_mired, max_mired) = color_temperature
            .then(|| {
                rhythm_devices::builtin_db()
                    .lookup(manufacturer, model)
                    .and_then(|entry| {
                        let min_kelvin = entry.min_kelvin?;
                        let max_kelvin = entry.max_kelvin?;
                        kelvin_range_to_mired(min_kelvin, max_kelvin)
                    })
                    .unwrap_or((
                        super::protocol::HUE_DEFAULT_MIN_MIRED,
                        super::protocol::HUE_DEFAULT_MAX_MIRED,
                    ))
            })
            .map_or((None, None), |(min, max)| (Some(min), Some(max)));

        Self {
            dimming,
            color_temperature,
            xy_color,
            combined_control,
            effects: combined_control && xy_color,
            min_mired,
            max_mired,
        }
    }

    pub fn min_kelvin(&self) -> Option<u16> {
        self.max_mired
            .filter(|mired| *mired != 0)
            .map(|mired| (1_000_000u32 / u32::from(mired)) as u16)
    }

    pub fn max_kelvin(&self) -> Option<u16> {
        self.min_mired
            .filter(|mired| *mired != 0)
            .map(|mired| (1_000_000u32 / u32::from(mired)) as u16)
    }
}

fn kelvin_range_to_mired(min_kelvin: u16, max_kelvin: u16) -> Option<(u16, u16)> {
    if min_kelvin == 0 || max_kelvin == 0 || min_kelvin > max_kelvin {
        return None;
    }
    let min_mired = (1_000_000u32 / u32::from(max_kelvin)).clamp(1, u32::from(u16::MAX)) as u16;
    let max_mired = (1_000_000u32 / u32::from(min_kelvin)).clamp(1, u32::from(u16::MAX)) as u16;
    Some((min_mired, max_mired))
}

/// A bonded Hue BLE light persisted independently of BlueZ's bond database.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HueBleDevice {
    /// Stable hub-native ID derived from the bulb EUI-64.
    pub id: String,
    /// Last known BlueZ address. Hue uses a random address that can change
    /// after factory reset, so this is a locator rather than identity.
    pub address: String,
    pub address_type: String,
    /// Canonical, big-endian EUI-64 (hex, no separators).
    pub eui64: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    pub firmware: String,
    pub capabilities: HueBleCapabilities,
    pub paired_at_epoch_secs: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_state: Option<HueBleState>,
}

impl HueBleDevice {
    pub fn stable_id(eui64: &str) -> String {
        format!(
            "hue-ble-{}",
            eui64
                .chars()
                .filter(|c| c.is_ascii_hexdigit())
                .flat_map(char::to_lowercase)
                .collect::<String>()
        )
    }

    pub fn display_name(&self) -> String {
        let name = self.name.trim();
        if name.is_empty() || name.eq_ignore_ascii_case("hue lamp") {
            format!("Hue {}", self.model)
        } else {
            name.to_string()
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueBleColor {
    ColorTemperature { mired: u16 },
    Xy { x: f32, y: f32 },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HueBleState {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub on: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brightness: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<HueBleColor>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<HueBleEffect>,
}

/// One direct control operation. Values use Hue's native units.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HueBleCommand {
    pub on: Option<bool>,
    /// Hue brightness level, 1..=254.
    pub brightness: Option<u8>,
    pub color: Option<HueBleColor>,
    pub effect: Option<HueBleEffect>,
    pub effect_speed: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HueBlePairingRequest {
    pub session_id: Option<String>,
    /// Return the exact, durable stale-bond recovery candidates without
    /// scanning, pairing, or mutating BlueZ. The app uses this auxiliary
    /// request to make the user choose one bulb before any old key is removed.
    pub list_stale_bond_candidates: bool,
    /// Optional explicit candidate selected from a nearby-device UI. During
    /// stale-bond recovery this identifies the old quarantined key only; it
    /// must not filter discovery because a reset bulb may rotate its address.
    pub candidate_address: Option<String>,
    /// User confirmation that one quarantined Hue bulb was physically
    /// factory-reset. The lifecycle layer resolves this to exactly one stale
    /// BlueZ address; transports must never use it to remove an unknown bond.
    pub replace_stale_bonds: bool,
    /// Bonded addresses already represented by the device store. Discovery
    /// excludes them while still allowing an orphan bond to be adopted after
    /// a crash between BlueZ pairing and metadata persistence.
    pub known_addresses: Vec<String>,
    /// Addresses whose Rhythm metadata was explicitly force-removed while
    /// BlueZ bond deletion failed. Paired objects at these addresses must not
    /// be adopted as crash orphans; an unpaired/factory-reset candidate may be
    /// associated again by an explicit scan.
    pub blocked_paired_addresses: Vec<String>,
    /// Exact tombstoned addresses selected by an explicit re-association.
    /// These may be adopted even when unknown paired-orphan adoption is
    /// globally disabled.
    pub explicit_reassociation_addresses: Vec<String>,
    /// Exact quarantined BlueZ addresses that may be removed before scanning
    /// because the user confirmed the corresponding bulb was factory-reset.
    /// This is populated internally and never accepted directly from API JSON.
    pub replace_stale_bond_addresses: Vec<String>,
    /// False after corrupt metadata recovery, when Rhythm cannot know which
    /// remaining BlueZ bonds were previously force-removed.
    pub allow_paired_orphan_adoption: bool,
    pub scan_timeout_secs: u64,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HueBlePairingOutcome {
    pub devices: Vec<HueBleDevice>,
    /// Nonfatal per-candidate failures when at least one nearby bulb paired.
    pub warnings: Vec<String>,
    /// Addresses where BlueZ may own a bond even though validation did not
    /// produce durable device metadata. Explicit scans may safely retry these
    /// exact addresses without opening adoption of unrelated paired objects.
    pub retained_bond_addresses: Vec<String>,
}

impl HueBlePairingRequest {
    pub fn from_value(value: &serde_json::Value) -> anyhow::Result<Self> {
        let session_id = value
            .get("session_id")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let candidate_address = value
            .get("candidate_address")
            .and_then(serde_json::Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string);
        let scan_timeout_secs = value
            .get("scan_timeout_secs")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(15)
            .clamp(5, 60);
        let replace_stale_bonds = value
            .get("replace_stale_bonds")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let list_stale_bond_candidates = value
            .get("list_stale_bond_candidates")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        if list_stale_bond_candidates && replace_stale_bonds {
            anyhow::bail!(
                "Hue Bluetooth stale-bond listing and replacement are separate operations"
            );
        }

        Ok(Self {
            session_id,
            list_stale_bond_candidates,
            candidate_address,
            replace_stale_bonds,
            known_addresses: Vec::new(),
            blocked_paired_addresses: Vec::new(),
            explicit_reassociation_addresses: Vec::new(),
            replace_stale_bond_addresses: Vec::new(),
            allow_paired_orphan_adoption: true,
            scan_timeout_secs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direct_ble_pairing_requires_no_printed_code() {
        let request = HueBlePairingRequest::from_value(&serde_json::json!({})).unwrap();
        assert_eq!(request.scan_timeout_secs, 15);
        assert!(!request.list_stale_bond_candidates);
        assert!(request.candidate_address.is_none());
        assert!(!request.replace_stale_bonds);
        assert!(request.explicit_reassociation_addresses.is_empty());
        assert!(request.replace_stale_bond_addresses.is_empty());
    }

    #[test]
    fn stale_bond_replacement_requires_an_explicit_boolean_confirmation() {
        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "replace_stale_bonds": true
        }))
        .unwrap();
        assert!(request.replace_stale_bonds);
        assert!(
            request.replace_stale_bond_addresses.is_empty(),
            "API callers cannot nominate arbitrary BlueZ addresses"
        );
    }

    #[test]
    fn stale_bond_listing_is_non_destructive_and_separate_from_replacement() {
        let request = HueBlePairingRequest::from_value(&serde_json::json!({
            "list_stale_bond_candidates": true
        }))
        .unwrap();
        assert!(request.list_stale_bond_candidates);
        assert!(!request.replace_stale_bonds);

        let error = HueBlePairingRequest::from_value(&serde_json::json!({
            "list_stale_bond_candidates": true,
            "replace_stale_bonds": true
        }))
        .unwrap_err();
        assert!(error.to_string().contains("separate operations"));
    }

    #[test]
    fn stable_id_uses_normalized_eui() {
        assert_eq!(
            HueBleDevice::stable_id("00:17:88:01:0C:76:5B:A7"),
            "hue-ble-001788010c765ba7"
        );
    }

    #[test]
    fn live_lca013_uses_its_extended_temperature_range() {
        let capabilities = HueBleCapabilities::from_gatt(
            "Signify Netherlands B.V.",
            "LCA013",
            true,
            true,
            true,
            true,
        );
        assert_eq!(capabilities.min_mired, Some(50));
        assert_eq!(capabilities.max_mired, Some(1000));
        assert_eq!(capabilities.min_kelvin(), Some(1000));
        assert_eq!(capabilities.max_kelvin(), Some(20000));
    }

    #[test]
    fn unknown_ct_models_keep_the_legacy_hue_range() {
        let capabilities = HueBleCapabilities::from_gatt(
            "Signify Netherlands B.V.",
            "UNKNOWN",
            true,
            true,
            false,
            true,
        );
        assert_eq!(
            capabilities.min_mired,
            Some(super::super::protocol::HUE_DEFAULT_MIN_MIRED)
        );
        assert_eq!(
            capabilities.max_mired,
            Some(super::super::protocol::HUE_DEFAULT_MAX_MIRED)
        );
    }
}
