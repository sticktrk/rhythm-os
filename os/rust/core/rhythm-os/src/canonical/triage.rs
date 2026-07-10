//! Cross-hub triage queue.
//!
//! All cross-hub merging (device deduplication and room binding) requires user
//! approval. The triage queue holds pending proposals until the user confirms,
//! rejects, or dismisses them. Previously-approved merges are recorded so they
//! re-apply silently on re-sync.

use serde::{Deserialize, Serialize};

use super::identity::{DiscoveredIdentity, HubKey};

/// What kind of triage this entry represents.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageKind {
    /// Cross-hub device deduplication: "Is this the same physical device?"
    #[default]
    DeviceMerge,
    /// Cross-hub room binding: "Should this hub's room merge into an existing Rhythm room?"
    RoomBinding,
    /// Device has no room assignment and needs one.
    UnassignedDevice,
    /// Device has native hub automation configured that conflicts with Rhythm.
    HubConfigured,
}

/// Why a candidate was matched (for triage evidence).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchReason {
    /// Exact hardware ID match (MAC address, serial number).
    ExactHardware,
    /// Exact name match (case-insensitive).
    ExactName,
    /// Partial/substring name match.
    PartialName,
    /// Same manufacturer.
    SameManufacturer,
    /// Same model.
    SameModel,
    /// Same device type.
    SameDeviceType,
    /// Discovered in a room with the same name as the candidate's source room.
    SameRoom,
}

/// A candidate match with score and reasons.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct CandidateMatch {
    /// Canonical device ID of the candidate.
    pub canonical_id: String,
    /// Human-readable name of the candidate device.
    #[serde(default)]
    pub name: String,
    /// Heuristic match score (higher = stronger match).
    pub score: u32,
    /// Why this candidate matched.
    pub reasons: Vec<MatchReason>,
}

/// Status of a triage entry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TriageStatus {
    /// Waiting for user resolution.
    Pending,
    /// Automatically resolved (exact HW match).
    AutoResolved,
    /// User confirmed the merge.
    Confirmed,
    /// User chose to create a new device (rejected merge).
    NewDevice,
    /// User chose to keep a proposed room binding as separate rooms.
    KeptSeparate,
    /// User dismissed (don't track this device).
    Dismissed,
}

/// A triage queue entry for cross-hub approval.
///
/// Covers both device deduplication (DeviceMerge) and room binding (RoomBinding).
/// Preserves full evidence: discovery snapshot, scored candidates with reason
/// codes, and resolution metadata.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriageEntry {
    /// Unique entry ID.
    pub id: String,
    /// What kind of triage this is (device merge vs room binding).
    #[serde(default)]
    pub kind: TriageKind,
    /// Full snapshot of the discovered device (present for DeviceMerge).
    pub discovered: TriageDiscoveredDevice,
    /// Hub that discovered this device/room.
    pub hub_key: HubKey,
    /// Scored candidate matches with reason codes (for DeviceMerge).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_matches: Vec<CandidateMatch>,
    /// Room binding proposal (present for RoomBinding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub room_binding: Option<RoomBindingProposal>,
    /// Confidence level for sorting (higher = more certain match).
    /// 100 = exact HW match, 80 = name match, lower = heuristic.
    #[serde(default)]
    pub confidence: u32,
    /// Current status.
    pub status: TriageStatus,
    /// Who resolved this entry ("auto" or "api").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_by: Option<String>,
    /// When this entry was created (Unix timestamp seconds).
    pub created_at: u64,
    /// When this entry was resolved (Unix timestamp seconds).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    /// Canonical device ID (for UnassignedDevice entries).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canonical_id: Option<String>,
}

/// Serializable subset of DiscoveredIdentity for triage persistence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TriageDiscoveredDevice {
    pub native_id: String,
    pub name: String,
    pub device_type: rhythm_core::runtime::hub_registry::DeviceType,
    /// Hub-native room ID (needed for post-triage topology update).
    #[serde(default)]
    pub room_id: String,
    pub room_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub manufacturer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Default for TriageDiscoveredDevice {
    fn default() -> Self {
        Self {
            native_id: String::new(),
            name: String::new(),
            device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
            room_id: String::new(),
            room_name: String::new(),
            manufacturer: None,
            model: None,
        }
    }
}

impl From<&DiscoveredIdentity> for TriageDiscoveredDevice {
    fn from(d: &DiscoveredIdentity) -> Self {
        // TriageDiscoveredDevice's persistence convention is empty-string-as-None
        // for room_id/room_name; downstream readers already check `.is_empty()`.
        Self {
            native_id: d.native_id.clone(),
            name: d.name.clone(),
            device_type: d.device_type.clone(),
            room_id: d.room_id.clone().unwrap_or_default(),
            room_name: d.room_name.clone().unwrap_or_default(),
            manufacturer: d.manufacturer.clone(),
            model: d.model.clone(),
        }
    }
}

/// A proposal to bind a hub room to an existing Rhythm room.
///
/// Created when a hub discovers a room whose name matches an existing Rhythm
/// room from a different hub. The user decides whether to merge (bind) or
/// keep them as separate rooms.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RoomBindingProposal {
    /// Hub-native room ID of the newly discovered room.
    pub hub_room_id: String,
    /// Name of the newly discovered room.
    pub hub_room_name: String,
    /// Hub-native control ID (grouped_light, area_id).
    pub control_id: String,
    /// Light device IDs in this room.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub light_device_ids: Vec<String>,
    /// Canonical device IDs already resolved in this room.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub canonical_device_ids: Vec<String>,
    /// Existing Rhythm room ID that this room could bind to (best match).
    pub target_rhythm_room_id: String,
    /// Existing Rhythm room name (for display).
    pub target_rhythm_room_name: String,
    /// All candidate Rhythm rooms that match by name (for 3+ hub case).
    /// Each entry is (rhythm_room_id, rhythm_room_name).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub candidate_rooms: Vec<(String, String)>,
}

/// The triage queue — manages pending cross-hub approval decisions.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TriageQueue {
    entries: Vec<TriageEntry>,
}

impl TriageQueue {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    /// Add a new triage entry.
    pub fn add(&mut self, entry: TriageEntry) {
        self.entries.push(entry);
    }

    /// Get all pending entries.
    pub fn pending(&self) -> Vec<&TriageEntry> {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending)
            .collect()
    }

    /// Get all entries (including resolved).
    pub fn all(&self) -> &[TriageEntry] {
        &self.entries
    }

    /// Find an entry by ID.
    pub fn get(&self, id: &str) -> Option<&TriageEntry> {
        self.entries.iter().find(|e| e.id == id)
    }

    /// Find an entry by ID (mutable).
    pub fn get_mut(&mut self, id: &str) -> Option<&mut TriageEntry> {
        self.entries.iter_mut().find(|e| e.id == id)
    }

    /// Check if a device from a specific hub is pending in the queue.
    ///
    /// Only matches `Pending` entries so that dismissed devices can re-enter
    /// triage if rediscovered.
    pub fn has_device(&self, hub_key: &HubKey, native_id: &str) -> bool {
        self.entries.iter().any(|e| {
            e.status == TriageStatus::Pending
                && e.hub_key == *hub_key
                && e.discovered.native_id == native_id
        })
    }

    /// Resolve an entry with a status and resolver identity.
    pub fn resolve(
        &mut self,
        entry_id: &str,
        status: TriageStatus,
        resolved_by: &str,
        now: u64,
    ) -> bool {
        if let Some(entry) = self.get_mut(entry_id) {
            entry.status = status;
            entry.resolved_by = Some(resolved_by.to_string());
            entry.resolved_at = Some(now);
            true
        } else {
            false
        }
    }

    /// Resolve an entry as new device (user rejected merge).
    pub fn resolve_new(&mut self, entry_id: &str, now: u64) -> bool {
        self.resolve(entry_id, TriageStatus::NewDevice, "api", now)
    }

    /// Resolve a room binding by keeping the rooms separate.
    pub fn resolve_keep_separate(&mut self, entry_id: &str, now: u64) -> bool {
        self.resolve(entry_id, TriageStatus::KeptSeparate, "api", now)
    }

    /// Dismiss an entry.
    pub fn dismiss(&mut self, entry_id: &str, now: u64) -> bool {
        self.resolve(entry_id, TriageStatus::Dismissed, "api", now)
    }

    /// Remove all resolved entries older than `before` timestamp.
    pub fn prune_resolved(&mut self, before: u64) {
        self.entries.retain(|e| {
            e.status == TriageStatus::Pending
                || matches!(
                    (&e.kind, &e.status),
                    (TriageKind::RoomBinding, TriageStatus::KeptSeparate)
                        | (TriageKind::RoomBinding, TriageStatus::NewDevice)
                        | (TriageKind::UnassignedDevice, TriageStatus::KeptSeparate)
                        | (TriageKind::UnassignedDevice, TriageStatus::Dismissed)
                )
                || e.resolved_at.map(|t| t >= before).unwrap_or(true)
        });
    }

    /// Number of pending entries.
    pub fn pending_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending)
            .count()
    }

    /// Total number of entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Get all pending entries of a specific kind.
    pub fn pending_by_kind(&self, kind: TriageKind) -> Vec<&TriageEntry> {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending && e.kind == kind)
            .collect()
    }

    /// Number of pending device merge entries.
    pub fn pending_device_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending && e.kind == TriageKind::DeviceMerge)
            .count()
    }

    /// Number of pending room binding entries.
    pub fn pending_room_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending && e.kind == TriageKind::RoomBinding)
            .count()
    }

    /// Number of pending unassigned device entries.
    pub fn pending_unassigned_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending && e.kind == TriageKind::UnassignedDevice)
            .count()
    }

    /// Check if a pending UnassignedDevice entry exists for a canonical device.
    pub fn has_unassigned_device(&self, canonical_id: &str) -> bool {
        self.entries.iter().any(|e| {
            e.status == TriageStatus::Pending
                && e.kind == TriageKind::UnassignedDevice
                && e.canonical_id.as_deref() == Some(canonical_id)
        })
    }

    /// Check whether an unassigned device already has a durable operator
    /// decision. Pending entries remain quarantined; `KeptSeparate` means the
    /// user explicitly enabled standalone operation; `Dismissed` means ignore.
    pub fn has_unassigned_decision(&self, canonical_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry.kind == TriageKind::UnassignedDevice
                && entry.canonical_id.as_deref() == Some(canonical_id)
                && matches!(
                    entry.status,
                    TriageStatus::Pending | TriageStatus::KeptSeparate | TriageStatus::Dismissed
                )
        })
    }

    /// Pending and ignored unassigned devices stay out of the automatic light
    /// runtime. Assigning a room or explicitly keeping a device standalone
    /// resolves the quarantine.
    pub fn quarantines_unassigned_device(&self, canonical_id: &str) -> bool {
        self.entries.iter().any(|entry| {
            entry.kind == TriageKind::UnassignedDevice
                && entry.canonical_id.as_deref() == Some(canonical_id)
                && matches!(
                    entry.status,
                    TriageStatus::Pending | TriageStatus::Dismissed
                )
        })
    }

    /// Auto-resolve any pending UnassignedDevice entry for a device (e.g. when assigned a room).
    pub fn resolve_unassigned_for_device(&mut self, canonical_id: &str, now: u64) -> bool {
        let mut resolved = false;
        for entry in &mut self.entries {
            if entry.status == TriageStatus::Pending
                && entry.kind == TriageKind::UnassignedDevice
                && entry.canonical_id.as_deref() == Some(canonical_id)
            {
                entry.status = TriageStatus::Confirmed;
                entry.resolved_by = Some("auto".to_string());
                entry.resolved_at = Some(now);
                resolved = true;
            }
        }
        resolved
    }

    /// Number of pending hub-configured device entries.
    pub fn pending_hub_configured_count(&self) -> usize {
        self.entries
            .iter()
            .filter(|e| e.status == TriageStatus::Pending && e.kind == TriageKind::HubConfigured)
            .count()
    }

    /// Check if a pending HubConfigured entry exists for a device from a specific hub.
    ///
    /// Only matches `Pending` entries so dismissed entries can re-enter triage.
    pub fn has_hub_configured(&self, hub_key: &HubKey, native_id: &str) -> bool {
        self.entries.iter().any(|e| {
            e.status == TriageStatus::Pending
                && e.kind == TriageKind::HubConfigured
                && e.hub_key == *hub_key
                && e.discovered.native_id == native_id
        })
    }

    /// Auto-resolve pending HubConfigured entries for a device (e.g. when behavior removed).
    pub fn resolve_hub_configured_for_device(
        &mut self,
        hub_key: &HubKey,
        native_id: &str,
        now: u64,
    ) -> bool {
        let mut resolved = false;
        for entry in &mut self.entries {
            if entry.status == TriageStatus::Pending
                && entry.kind == TriageKind::HubConfigured
                && entry.hub_key == *hub_key
                && entry.discovered.native_id == native_id
            {
                entry.status = TriageStatus::Confirmed;
                entry.resolved_by = Some("auto".to_string());
                entry.resolved_at = Some(now);
                resolved = true;
            }
        }
        resolved
    }

    /// Check if a room binding proposal already exists for a hub room.
    ///
    /// Dismissed proposals can re-enter triage, but confirmed and "kept separate"
    /// decisions should continue blocking repeat proposals for the same hub room.
    pub fn has_room_binding(&self, hub_key: &HubKey, hub_room_id: &str) -> bool {
        self.entries.iter().any(|e| {
            e.status != TriageStatus::Dismissed
                && e.kind == TriageKind::RoomBinding
                && e.hub_key == *hub_key
                && e.room_binding
                    .as_ref()
                    .map(|rb| rb.hub_room_id == hub_room_id)
                    .unwrap_or(false)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::identity::HubKey;
    use crate::hub::HubType;
    use rhythm_core::runtime::hub_registry::DeviceType;

    fn make_entry(id: &str, status: TriageStatus) -> TriageEntry {
        TriageEntry {
            id: id.to_string(),
            kind: TriageKind::DeviceMerge,
            discovered: TriageDiscoveredDevice {
                native_id: format!("native-{}", id),
                name: format!("Device {}", id),
                device_type: DeviceType::Light,
                room_id: "room-1".to_string(),
                room_name: "Kitchen".to_string(),
                manufacturer: None,
                model: None,
            },
            hub_key: HubKey::new(HubType::new("hue"), "192.168.1.1"),
            candidate_matches: vec![],
            room_binding: None,
            confidence: 0,
            status,
            resolved_by: None,
            created_at: 1000,
            resolved_at: None,
            canonical_id: None,
        }
    }

    fn make_room_binding_entry(id: &str, hub_room_id: &str, target_room_id: &str) -> TriageEntry {
        TriageEntry {
            id: id.to_string(),
            kind: TriageKind::RoomBinding,
            discovered: TriageDiscoveredDevice::default(),
            hub_key: HubKey::new(HubType::new("ha"), "192.168.1.2"),
            candidate_matches: vec![],
            room_binding: Some(RoomBindingProposal {
                hub_room_id: hub_room_id.to_string(),
                hub_room_name: "Kitchen".to_string(),
                control_id: "area-1".to_string(),
                light_device_ids: vec![],
                canonical_device_ids: vec![],
                target_rhythm_room_id: target_room_id.to_string(),
                target_rhythm_room_name: "Kitchen".to_string(),
                candidate_rooms: vec![],
            }),
            confidence: 80,
            status: TriageStatus::Pending,
            resolved_by: None,
            created_at: 1000,
            resolved_at: None,
            canonical_id: None,
        }
    }

    fn make_unassigned_entry(id: &str, canonical_id: &str) -> TriageEntry {
        TriageEntry {
            id: id.to_string(),
            kind: TriageKind::UnassignedDevice,
            discovered: TriageDiscoveredDevice {
                native_id: format!("native-{}", id),
                name: format!("Device {}", id),
                device_type: DeviceType::Light,
                room_id: String::new(),
                room_name: String::new(),
                manufacturer: None,
                model: None,
            },
            hub_key: HubKey::new(HubType::new("matter"), "local"),
            candidate_matches: vec![],
            room_binding: None,
            confidence: 0,
            status: TriageStatus::Pending,
            resolved_by: None,
            created_at: 1000,
            resolved_at: None,
            canonical_id: Some(canonical_id.to_string()),
        }
    }

    #[test]
    fn add_and_get() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        assert_eq!(q.len(), 1);
        assert!(q.get("e1").is_some());
        assert!(q.get("e2").is_none());
    }

    #[test]
    fn pending_filters_correctly() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        q.add(make_entry("e2", TriageStatus::AutoResolved));
        q.add(make_entry("e3", TriageStatus::Pending));

        let pending = q.pending();
        assert_eq!(pending.len(), 2);
        assert_eq!(q.pending_count(), 2);
    }

    #[test]
    fn resolve_merge() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));

        assert!(q.resolve("e1", TriageStatus::Confirmed, "api", 2000));
        let entry = q.get("e1").unwrap();
        assert_eq!(entry.status, TriageStatus::Confirmed);
        assert_eq!(entry.resolved_at, Some(2000));
        assert_eq!(entry.resolved_by.as_deref(), Some("api"));
        assert_eq!(q.pending_count(), 0);
    }

    #[test]
    fn resolve_new() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));

        assert!(q.resolve_new("e1", 2000));
        assert_eq!(q.get("e1").unwrap().status, TriageStatus::NewDevice);
    }

    #[test]
    fn resolve_keep_separate() {
        let mut q = TriageQueue::new();
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));

        assert!(q.resolve_keep_separate("rb1", 2000));
        assert_eq!(q.get("rb1").unwrap().status, TriageStatus::KeptSeparate);
    }

    #[test]
    fn dismiss() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));

        assert!(q.dismiss("e1", 2000));
        assert_eq!(q.get("e1").unwrap().status, TriageStatus::Dismissed);
    }

    #[test]
    fn has_device() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_entry("e1", TriageStatus::Pending));

        assert!(q.has_device(&hub, "native-e1"));
        assert!(!q.has_device(&hub, "native-e2"));
    }

    #[test]
    fn dismissed_device_not_blocked_by_has_device() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_entry("e1", TriageStatus::Pending));

        // Dismiss the entry
        q.dismiss("e1", 2000);
        // Dismissed device should no longer block re-triage
        assert!(!q.has_device(&hub, "native-e1"));
    }

    #[test]
    fn prune_resolved() {
        let mut q = TriageQueue::new();
        let mut e1 = make_entry("e1", TriageStatus::Confirmed);
        e1.resolved_at = Some(500);
        let mut e2 = make_entry("e2", TriageStatus::Confirmed);
        e2.resolved_at = Some(1500);
        q.add(e1);
        q.add(e2);
        q.add(make_entry("e3", TriageStatus::Pending));

        q.prune_resolved(1000);

        // e1 resolved at 500 < 1000 → pruned
        // e2 resolved at 1500 >= 1000 → kept
        // e3 pending → kept
        assert_eq!(q.len(), 2);
        assert!(q.get("e1").is_none());
        assert!(q.get("e2").is_some());
        assert!(q.get("e3").is_some());
    }

    #[test]
    fn pending_by_kind() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));
        q.add(make_entry("e2", TriageStatus::Pending));

        assert_eq!(q.pending_by_kind(TriageKind::DeviceMerge).len(), 2);
        assert_eq!(q.pending_by_kind(TriageKind::RoomBinding).len(), 1);
        assert_eq!(q.pending_device_count(), 2);
        assert_eq!(q.pending_room_count(), 1);
        assert_eq!(q.pending_count(), 3);
    }

    #[test]
    fn has_room_binding() {
        let mut q = TriageQueue::new();
        let ha_key = HubKey::new(HubType::new("ha"), "192.168.1.2");
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));

        assert!(q.has_room_binding(&ha_key, "ha-area-1"));
        assert!(!q.has_room_binding(&ha_key, "ha-area-2"));
    }

    #[test]
    fn dismissed_room_binding_not_blocked() {
        let mut q = TriageQueue::new();
        let ha_key = HubKey::new(HubType::new("ha"), "192.168.1.2");
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));
        q.dismiss("rb1", 2000);

        // Dismissed binding should not block re-triage
        assert!(!q.has_room_binding(&ha_key, "ha-area-1"));
    }

    #[test]
    fn kept_separate_room_binding_blocks_retriage() {
        let mut q = TriageQueue::new();
        let ha_key = HubKey::new(HubType::new("ha"), "192.168.1.2");
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));
        q.resolve_keep_separate("rb1", 2000);

        assert!(q.has_room_binding(&ha_key, "ha-area-1"));
    }

    #[test]
    fn legacy_room_binding_new_device_blocks_retriage() {
        let mut q = TriageQueue::new();
        let ha_key = HubKey::new(HubType::new("ha"), "192.168.1.2");
        let mut entry = make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1");
        entry.status = TriageStatus::NewDevice;
        entry.resolved_at = Some(2000);
        q.add(entry);

        assert!(q.has_room_binding(&ha_key, "ha-area-1"));
    }

    #[test]
    fn unassigned_device_tracking() {
        let mut q = TriageQueue::new();
        q.add(make_unassigned_entry("u1", "canonical-abc"));

        assert!(q.has_unassigned_device("canonical-abc"));
        assert!(!q.has_unassigned_device("canonical-xyz"));
        assert_eq!(q.pending_unassigned_count(), 1);
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn resolve_unassigned_on_room_assign() {
        let mut q = TriageQueue::new();
        q.add(make_unassigned_entry("u1", "canonical-abc"));
        q.add(make_entry("e1", TriageStatus::Pending));

        assert!(q.resolve_unassigned_for_device("canonical-abc", 2000));
        assert_eq!(q.pending_unassigned_count(), 0);
        // Other entries unaffected
        assert_eq!(q.pending_count(), 1);
        let entry = q.get("u1").unwrap();
        assert_eq!(entry.status, TriageStatus::Confirmed);
        assert_eq!(entry.resolved_by.as_deref(), Some("auto"));
    }

    #[test]
    fn resolve_unassigned_returns_false_when_not_found() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        assert!(!q.resolve_unassigned_for_device("canonical-xyz", 2000));
    }

    #[test]
    fn dismissed_unassigned_is_a_durable_ignore_decision() {
        let mut q = TriageQueue::new();
        q.add(make_unassigned_entry("u1", "canonical-abc"));
        q.dismiss("u1", 2000);

        // It is no longer pending, but it keeps the device quarantined and
        // prevents the same roomless discovery from nagging again.
        assert!(!q.has_unassigned_device("canonical-abc"));
        assert!(q.has_unassigned_decision("canonical-abc"));
        assert!(q.quarantines_unassigned_device("canonical-abc"));
    }

    #[test]
    fn explicit_standalone_decision_enables_unassigned_device() {
        let mut q = TriageQueue::new();
        q.add(make_unassigned_entry("u1", "canonical-abc"));
        q.resolve("u1", TriageStatus::KeptSeparate, "api", 2000);

        assert!(q.has_unassigned_decision("canonical-abc"));
        assert!(!q.quarantines_unassigned_device("canonical-abc"));
    }

    #[test]
    fn durable_unassigned_decisions_survive_resolved_pruning() {
        let mut q = TriageQueue::new();
        let mut standalone = make_unassigned_entry("standalone", "canonical-a");
        standalone.status = TriageStatus::KeptSeparate;
        standalone.resolved_at = Some(500);
        let mut ignored = make_unassigned_entry("ignored", "canonical-b");
        ignored.status = TriageStatus::Dismissed;
        ignored.resolved_at = Some(500);
        q.add(standalone);
        q.add(ignored);

        q.prune_resolved(1000);

        assert!(q.get("standalone").is_some());
        assert!(q.get("ignored").is_some());
    }

    #[test]
    fn pending_by_kind_includes_unassigned() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        q.add(make_unassigned_entry("u1", "canonical-abc"));
        q.add(make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1"));

        assert_eq!(q.pending_by_kind(TriageKind::UnassignedDevice).len(), 1);
        assert_eq!(q.pending_unassigned_count(), 1);
        assert_eq!(q.pending_device_count(), 1);
        assert_eq!(q.pending_room_count(), 1);
        assert_eq!(q.pending_count(), 3);
    }

    // ---- HubConfigured triage tests ----

    fn make_hub_configured_entry(id: &str, native_id: &str) -> TriageEntry {
        TriageEntry {
            id: id.to_string(),
            kind: TriageKind::HubConfigured,
            discovered: TriageDiscoveredDevice {
                native_id: native_id.to_string(),
                name: format!("Switch {}", id),
                device_type: DeviceType::Button,
                room_id: "room-1".to_string(),
                room_name: "Kitchen".to_string(),
                manufacturer: None,
                model: None,
            },
            hub_key: HubKey::new(HubType::new("hue"), "192.168.1.1"),
            candidate_matches: vec![],
            room_binding: None,
            confidence: 0,
            status: TriageStatus::Pending,
            resolved_by: None,
            created_at: 1000,
            resolved_at: None,
            canonical_id: None,
        }
    }

    #[test]
    fn hub_configured_tracking() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_hub_configured_entry("hc1", "device-abc"));

        assert!(q.has_hub_configured(&hub, "device-abc"));
        assert!(!q.has_hub_configured(&hub, "device-xyz"));
        assert_eq!(q.pending_hub_configured_count(), 1);
        assert_eq!(q.pending_count(), 1);
    }

    #[test]
    fn hub_configured_dedup() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_hub_configured_entry("hc1", "device-abc"));

        // Second entry for same device — has_hub_configured returns true
        assert!(q.has_hub_configured(&hub, "device-abc"));
    }

    #[test]
    fn hub_configured_dismissed_allows_re_triage() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_hub_configured_entry("hc1", "device-abc"));
        q.dismiss("hc1", 2000);

        // Dismissed entry should not block re-triage
        assert!(!q.has_hub_configured(&hub, "device-abc"));
    }

    #[test]
    fn hub_configured_auto_resolve() {
        let mut q = TriageQueue::new();
        let hub = HubKey::new(HubType::new("hue"), "192.168.1.1");
        q.add(make_hub_configured_entry("hc1", "device-abc"));

        assert!(q.resolve_hub_configured_for_device(&hub, "device-abc", 2000));
        assert_eq!(q.pending_hub_configured_count(), 0);
        let entry = q.get("hc1").unwrap();
        assert_eq!(entry.status, TriageStatus::Confirmed);
        assert_eq!(entry.resolved_by.as_deref(), Some("auto"));
    }

    #[test]
    fn pending_by_kind_includes_hub_configured() {
        let mut q = TriageQueue::new();
        q.add(make_entry("e1", TriageStatus::Pending));
        q.add(make_hub_configured_entry("hc1", "device-abc"));
        q.add(make_unassigned_entry("u1", "canonical-abc"));

        assert_eq!(q.pending_by_kind(TriageKind::HubConfigured).len(), 1);
        assert_eq!(q.pending_hub_configured_count(), 1);
        assert_eq!(q.pending_count(), 3);
    }

    #[test]
    fn prune_resolved_keeps_room_binding_keep_separate() {
        let mut q = TriageQueue::new();
        let mut kept_separate = make_room_binding_entry("rb1", "ha-area-1", "rhythm-room-1");
        kept_separate.status = TriageStatus::KeptSeparate;
        kept_separate.resolved_at = Some(500);
        q.add(kept_separate);

        q.prune_resolved(1000);

        assert!(q.get("rb1").is_some());
    }
}
