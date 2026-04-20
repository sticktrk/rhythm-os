//! Canonical device identity types.
//!
//! Every physical device gets a stable Rhythm UUID, independent of any hub.
//! Hub-native IDs become "endpoints" — ways to reach the device, not the
//! device's identity.

use serde::{Deserialize, Serialize};

use rhythm_core::runtime::hub_registry::DeviceType;

use crate::hub::HubType;

fn integration_endpoint_active_default() -> bool {
    true
}

fn integration_endpoint_active_is_true(value: &bool) -> bool {
    *value
}

/// A hardware identifier used for cross-hub deduplication.
///
/// IEEE and MAC addresses are the same thing for Zigbee devices — the dedup
/// key treats them equivalently after normalization via `rhythm_core::device::ieee::normalize()`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum HardwareId {
    /// MAC / IEEE address (normalized to `00:17:88:01:09:ab:cd:ef` format).
    Mac(String),
    /// Serial number (manufacturer-assigned).
    Serial(String),
    /// Matter device ID.
    MatterId(String),
}

impl HardwareId {
    /// Create a MAC/IEEE hardware ID, normalizing the address.
    pub fn mac(addr: &str) -> Self {
        Self::Mac(rhythm_core::device::ieee::normalize(addr))
    }

    /// Create a serial number hardware ID.
    pub fn serial(serial: &str) -> Self {
        Self::Serial(serial.to_string())
    }

    /// Create a Matter device ID.
    pub fn matter(id: &str) -> Self {
        Self::MatterId(id.to_string())
    }

    /// Get the inner value regardless of variant.
    pub fn value(&self) -> &str {
        match self {
            Self::Mac(v) | Self::Serial(v) | Self::MatterId(v) => v,
        }
    }
}

/// A hub instance key: (hub_type, address) uniquely identifies a hub.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HubKey {
    pub hub_type: HubType,
    pub address: String,
}

impl HubKey {
    pub fn new(hub_type: HubType, address: impl Into<String>) -> Self {
        Self {
            hub_type,
            address: address.into(),
        }
    }
}

impl std::fmt::Display for HubKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.hub_type.as_str(), self.address)
    }
}

/// An integration endpoint — one way to reach a device through a specific hub.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct IntegrationEndpoint {
    /// Which hub instance provides this endpoint.
    pub hub_key: HubKey,
    /// Hub-native device ID (Hue UUID, HA entity_id, etc.).
    pub native_id: String,
    /// Whether this is the preferred endpoint for control.
    ///
    /// Deliberately simple for now. The struct has room to evolve toward richer
    /// policy (control vs state preference, health-based failover) without
    /// breaking changes.
    pub preferred: bool,
    /// Whether this endpoint is currently present in the latest hub discovery.
    ///
    /// Missing endpoints are marked inactive instead of being removed so the
    /// same canonical device can reactivate cleanly on a later reconnect.
    #[serde(
        default = "integration_endpoint_active_default",
        skip_serializing_if = "integration_endpoint_active_is_true"
    )]
    pub active: bool,
    /// When this device was last seen on this hub (Unix timestamp seconds).
    pub last_seen: u64,
    /// Hub-native room name at discovery time (for cross-hub matching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_room_name: Option<String>,
    /// Device capabilities (color modes, button count, etc.). Reserved for future use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<serde_json::Value>,
}

/// A record of when/how a device was merged with another.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MergeRecord {
    /// When the merge happened (Unix timestamp seconds).
    pub merged_at: u64,
    /// Hub that triggered the merge.
    pub source_hub: HubKey,
    /// Native device ID that was merged in.
    pub source_native_id: String,
    /// Why the merge happened.
    pub reason: String,
}

/// Canonical device — a physical device with a stable Rhythm identity.
///
/// Design principle: The canonical registry is device-first. Rooms are assigned
/// to devices, not devices to rooms. This is the true ontology even when
/// control routing operates at room level.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CanonicalDevice {
    /// Stable Rhythm UUID, generated once, never changes.
    pub id: String,
    /// User-overridable display name.
    pub name: String,
    /// Device type (Light, Button, Motion).
    pub device_type: DeviceType,
    /// Hardware identifiers for deduplication (MAC, serial, etc.).
    pub hardware_ids: Vec<HardwareId>,
    /// Manufacturer name (for heuristic matching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    /// Model identifier (for heuristic matching).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Integration endpoints — one per hub that can reach this device.
    pub endpoints: Vec<IntegrationEndpoint>,
    /// Rhythm room assignment (Rhythm room UUID, not hub room).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_id: Option<String>,
    /// When/how this device was merged with another. Enables debugging bad merges.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub merge_history: Vec<MergeRecord>,
    /// Soft-delete tombstone. Never hard-delete canonical devices.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<u64>,
    /// When this device was first created (Unix timestamp seconds).
    pub created_at: u64,
}

impl CanonicalDevice {
    /// Create a new canonical device with a generated UUID.
    pub fn new(
        name: impl Into<String>,
        device_type: DeviceType,
        hardware_ids: Vec<HardwareId>,
        now: u64,
    ) -> Self {
        Self {
            id: generate_uuid(),
            name: name.into(),
            device_type,
            hardware_ids,
            manufacturer: None,
            model: None,
            endpoints: Vec::new(),
            room_id: None,
            merge_history: Vec::new(),
            removed_at: None,
            created_at: now,
        }
    }

    /// Check if this device has been soft-deleted.
    pub fn is_removed(&self) -> bool {
        self.removed_at.is_some()
    }

    /// Find an endpoint for a specific hub.
    pub fn endpoint_for_hub(&self, hub_key: &HubKey) -> Option<&IntegrationEndpoint> {
        self.endpoints.iter().find(|e| &e.hub_key == hub_key)
    }

    /// Iterate only currently-active endpoints.
    pub fn active_endpoints(&self) -> impl Iterator<Item = &IntegrationEndpoint> {
        self.endpoints.iter().filter(|endpoint| endpoint.active)
    }

    /// Check whether any active endpoint remains.
    pub fn has_active_endpoint(&self) -> bool {
        self.active_endpoints().next().is_some()
    }

    /// Find an endpoint by native ID.
    pub fn endpoint_by_native_id(&self, native_id: &str) -> Option<&IntegrationEndpoint> {
        self.endpoints.iter().find(|e| e.native_id == native_id)
    }

    /// Get the preferred endpoint, or the first one if none is preferred.
    pub fn preferred_endpoint(&self) -> Option<&IntegrationEndpoint> {
        self.endpoints
            .iter()
            .find(|e| e.active && e.preferred)
            .or_else(|| self.endpoints.iter().find(|e| e.active))
    }

    /// Add or update an endpoint for a hub.
    ///
    /// If an endpoint for this hub+native_id already exists, updates last_seen.
    /// Otherwise adds a new endpoint. If this is the first endpoint, marks it preferred.
    pub fn upsert_endpoint(
        &mut self,
        hub_key: HubKey,
        native_id: String,
        now: u64,
        source_room_name: Option<String>,
    ) {
        if let Some(ep) = self
            .endpoints
            .iter_mut()
            .find(|e| e.hub_key == hub_key && e.native_id == native_id)
        {
            ep.active = true;
            ep.last_seen = now;
            if source_room_name.is_some() {
                ep.source_room_name = source_room_name;
            }
        } else {
            let preferred = self.endpoints.is_empty();
            self.endpoints.push(IntegrationEndpoint {
                hub_key,
                native_id,
                preferred,
                active: true,
                last_seen: now,
                source_room_name,
                capabilities: None,
            });
        }
    }

    /// Mark an endpoint active/inactive. Returns `true` if it changed.
    pub fn set_endpoint_active(&mut self, hub_key: &HubKey, native_id: &str, active: bool) -> bool {
        let Some(endpoint) = self
            .endpoints
            .iter_mut()
            .find(|endpoint| &endpoint.hub_key == hub_key && endpoint.native_id == native_id)
        else {
            return false;
        };

        if endpoint.active == active {
            return false;
        }
        endpoint.active = active;
        true
    }

    /// Record a merge event in the device's history.
    pub fn record_merge(
        &mut self,
        source_hub: HubKey,
        source_native_id: String,
        reason: String,
        now: u64,
    ) {
        self.merge_history.push(MergeRecord {
            merged_at: now,
            source_hub,
            source_native_id,
            reason,
        });
    }

    /// Check if any hardware ID matches.
    pub fn has_hardware_id(&self, hw_id: &HardwareId) -> bool {
        self.hardware_ids.contains(hw_id)
    }

    /// Check if any hardware ID value matches (regardless of type).
    pub fn has_hardware_value(&self, value: &str) -> bool {
        self.hardware_ids.iter().any(|h| h.value() == value)
    }
}

/// A device discovered with hardware identity information.
///
/// Extends `DiscoveredDevice` with hardware IDs, manufacturer/model, and
/// the hub room name for cross-hub matching.
#[derive(Clone, Debug)]
pub struct DiscoveredIdentity {
    /// Hub-native device ID.
    pub native_id: String,
    /// Hub-native room ID this device belongs to.
    pub room_id: String,
    /// Hub-native room name (for heuristic room matching).
    pub room_name: String,
    /// Human-readable device name.
    pub name: String,
    /// Device type.
    pub device_type: DeviceType,
    /// Hardware identifiers (MAC, IEEE, serial).
    pub hardware_ids: Vec<HardwareId>,
    /// Manufacturer name.
    pub manufacturer: Option<String>,
    /// Model identifier.
    pub model: Option<String>,
}

/// Public UUID generator for use by other modules.
pub fn generate_uuid_public() -> String {
    generate_uuid()
}

/// Generate a UUID v4 string without external dependencies.
///
/// Uses a simple xorshift-based PRNG seeded from system time. Not
/// cryptographically secure, but fine for device identity UUIDs.
fn generate_uuid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();

    // Mix in thread ID + a counter for uniqueness within the same nanosecond
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    let count = COUNTER.fetch_add(1, Ordering::Relaxed);
    let thread_id = std::thread::current().id();
    let thread_hash = format!("{:?}", thread_id).len() as u128;

    let mut state = seed ^ (count as u128) ^ (thread_hash << 64);

    let mut bytes = [0u8; 16];
    for byte in &mut bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = (state & 0xFF) as u8;
    }

    // Set version 4 and variant bits
    bytes[6] = (bytes[6] & 0x0F) | 0x40;
    bytes[8] = (bytes[8] & 0x3F) | 0x80;

    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3],
        bytes[4], bytes[5],
        bytes[6], bytes[7],
        bytes[8], bytes[9],
        bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15],
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hardware_id_mac_normalizes() {
        let hw = HardwareId::mac("00:17:88:01:09:AB:CD:EF");
        assert_eq!(hw.value(), "00:17:88:01:09:ab:cd:ef");
    }

    #[test]
    fn hardware_id_equality() {
        let a = HardwareId::mac("00:17:88:01:09:AB:CD:EF");
        let b = HardwareId::mac("00-17-88-01-09-ab-cd-ef");
        assert_eq!(a, b);
    }

    #[test]
    fn hub_key_display() {
        let key = HubKey::new(HubType::new("hue"), "192.168.1.100");
        assert_eq!(key.to_string(), "hue@192.168.1.100");
    }

    #[test]
    fn canonical_device_new_has_uuid() {
        let dev = CanonicalDevice::new("Test Light", DeviceType::Light, vec![], 1000);
        assert!(!dev.id.is_empty());
        assert_eq!(dev.name, "Test Light");
        assert_eq!(dev.device_type, DeviceType::Light);
        assert_eq!(dev.created_at, 1000);
        assert!(dev.removed_at.is_none());
        assert!(dev.merge_history.is_empty());
    }

    #[test]
    fn canonical_device_uuids_are_unique() {
        let a = CanonicalDevice::new("A", DeviceType::Light, vec![], 1000);
        let b = CanonicalDevice::new("B", DeviceType::Light, vec![], 1000);
        assert_ne!(a.id, b.id);
    }

    #[test]
    fn upsert_endpoint_first_is_preferred() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key = HubKey::new(HubType::new("hue"), "192.168.1.1");
        dev.upsert_endpoint(key.clone(), "native-1".into(), 1000, Some("Kitchen".into()));

        assert_eq!(dev.endpoints.len(), 1);
        assert!(dev.endpoints[0].preferred);
        assert!(dev.endpoints[0].active);
        assert_eq!(dev.endpoints[0].native_id, "native-1");
        assert_eq!(
            dev.endpoints[0].source_room_name.as_deref(),
            Some("Kitchen")
        );
    }

    #[test]
    fn upsert_endpoint_second_not_preferred() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key1 = HubKey::new(HubType::new("hue"), "192.168.1.1");
        let key2 = HubKey::new(HubType::new("ha"), "192.168.1.2");
        dev.upsert_endpoint(key1, "native-1".into(), 1000, None);
        dev.upsert_endpoint(key2, "native-2".into(), 1001, None);

        assert_eq!(dev.endpoints.len(), 2);
        assert!(dev.endpoints[0].preferred);
        assert!(!dev.endpoints[1].preferred);
    }

    #[test]
    fn upsert_endpoint_updates_last_seen() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key = HubKey::new(HubType::new("hue"), "192.168.1.1");
        dev.upsert_endpoint(key.clone(), "native-1".into(), 1000, None);
        dev.upsert_endpoint(key.clone(), "native-1".into(), 2000, None);

        assert_eq!(dev.endpoints.len(), 1);
        assert_eq!(dev.endpoints[0].last_seen, 2000);
    }

    #[test]
    fn has_hardware_id_matches() {
        let dev = CanonicalDevice::new(
            "Light",
            DeviceType::Light,
            vec![HardwareId::mac("00:17:88:01:09:ab:cd:ef")],
            1000,
        );
        assert!(dev.has_hardware_id(&HardwareId::mac("00:17:88:01:09:AB:CD:EF")));
        assert!(!dev.has_hardware_id(&HardwareId::mac("00:11:22:33:44:55:66:77")));
    }

    #[test]
    fn preferred_endpoint_returns_preferred() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key1 = HubKey::new(HubType::new("hue"), "192.168.1.1");
        let key2 = HubKey::new(HubType::new("ha"), "192.168.1.2");
        dev.upsert_endpoint(key1, "native-1".into(), 1000, None);
        dev.upsert_endpoint(key2, "native-2".into(), 1001, None);

        let pref = dev.preferred_endpoint().unwrap();
        assert_eq!(pref.native_id, "native-1");
    }

    #[test]
    fn preferred_endpoint_skips_inactive() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key1 = HubKey::new(HubType::new("hue"), "192.168.1.1");
        let key2 = HubKey::new(HubType::new("ha"), "192.168.1.2");
        dev.upsert_endpoint(key1.clone(), "native-1".into(), 1000, None);
        dev.upsert_endpoint(key2.clone(), "native-2".into(), 1001, None);
        assert!(dev.set_endpoint_active(&key1, "native-1", false));

        let pref = dev.preferred_endpoint().unwrap();
        assert_eq!(pref.native_id, "native-2");
        assert!(pref.active);
        assert!(!dev.endpoints[0].active);
    }

    #[test]
    fn merge_history_records() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        let key = HubKey::new(HubType::new("ha"), "192.168.1.2");
        dev.record_merge(key, "light.kitchen".into(), "exact_hw_match".into(), 2000);
        assert_eq!(dev.merge_history.len(), 1);
        assert_eq!(dev.merge_history[0].reason, "exact_hw_match");
    }

    #[test]
    fn soft_delete() {
        let mut dev = CanonicalDevice::new("Light", DeviceType::Light, vec![], 1000);
        assert!(!dev.is_removed());
        dev.removed_at = Some(2000);
        assert!(dev.is_removed());
    }

    #[test]
    fn generate_uuid_format() {
        let uuid = generate_uuid();
        // 8-4-4-4-12 format with hyphens
        assert_eq!(uuid.len(), 36);
        assert_eq!(uuid.chars().filter(|c| *c == '-').count(), 4);
        // Version 4 marker
        assert_eq!(&uuid[14..15], "4");
    }
}
