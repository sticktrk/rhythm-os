//! Durable, privacy-bounded device liveness evidence.
//!
//! This state deliberately lives outside canonical topology. A previous
//! binary can ignore or discard it without corrupting device identity, room
//! assignment, or backup/restore state.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::canonical::identity::HubKey;
use crate::hub::DeviceReachabilityFailureClass;
use rhythm_core::runtime::hub_registry::DeviceType;

pub const DEVICE_HEALTH_SCHEMA_VERSION: u32 = 1;
pub const UNREACHABLE_NO_PROOF_SECS: u64 = 24 * 60 * 60;
pub const UNREACHABLE_FAILURE_SPAN_SECS: u64 = 24 * 60 * 60;
pub const UNREACHABLE_MIN_FAILURES: u32 = 3;
pub const UNREACHABLE_FAILURE_SEPARATION_SECS: u64 = 8 * 60 * 60;
pub const CONTROLLER_RESTART_GRACE_SECS: u64 = 15 * 60;
pub const HEALTHY_PEER_MAX_AGE_SECS: u64 = 30 * 60;
pub const UNREACHABLE_SNOOZE_SECS: u64 = 24 * 60 * 60;
pub const RESOLVED_RETENTION_SECS: u64 = 35 * 24 * 60 * 60;
pub const MAX_DEVICE_HEALTH_RECORDS: usize = 4_096;
const PROOF_PERSIST_INTERVAL_SECS: u64 = 15 * 60;

fn schema_version() -> u32 {
    DEVICE_HEALTH_SCHEMA_VERSION
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeviceHealthStatus {
    Monitoring,
    Pending,
    AwaitingRecovery,
    Snoozed,
    Recovered,
    Removed,
}

impl DeviceHealthStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Monitoring => "monitoring",
            Self::Pending => "pending",
            Self::AwaitingRecovery => "awaiting_recovery",
            Self::Snoozed => "snoozed",
            Self::Recovered => "recovered",
            Self::Removed => "removed",
        }
    }

    fn is_attention(self) -> bool {
        matches!(self, Self::Pending | Self::AwaitingRecovery | Self::Snoozed)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceHealthRecord {
    pub canonical_id: String,
    pub hub_key: HubKey,
    pub native_id: String,
    /// One-way fingerprint of the Matter fabric identity. Raw fabric IDs are
    /// never written to disk or included in debug bundles.
    pub fabric_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controller_stream_fingerprint: Option<String>,
    pub last_proof_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_failure_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_failure_at: Option<u64>,
    #[serde(default)]
    pub failure_count: u32,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub failure_classes: BTreeSet<DeviceReachabilityFailureClass>,
    pub status: DeviceHealthStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resolved_at: Option<u64>,
    /// Opaque app journey used only to correlate a later authoritative
    /// recovery with the user's power-cycle action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_correlation_id: Option<String>,
}

impl DeviceHealthRecord {
    fn key(hub_key: &HubKey, native_id: &str) -> String {
        format!("{}|{}", hub_key, native_id)
    }

    fn reset_failure_window(&mut self) {
        self.first_failure_at = None;
        self.last_failure_at = None;
        self.failure_count = 0;
        self.failure_classes.clear();
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceHealthLedger {
    #[serde(default = "schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    records: BTreeMap<String, DeviceHealthRecord>,
    #[serde(skip)]
    controllers: HashMap<String, ControllerHealth>,
    #[serde(skip)]
    last_persisted_proof: HashMap<String, u64>,
}

#[derive(Clone, Copy, Debug, Default)]
struct ControllerHealth {
    connected: bool,
    grace_until: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeviceHealthTransition {
    Admitted,
    Recovered,
    Removed,
}

#[derive(Clone, Debug, Default)]
pub struct DeviceHealthUpdate {
    pub persist: bool,
    pub transitions: Vec<(String, DeviceHealthTransition)>,
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ActiveDeviceHealthIdentity {
    pub canonical_id: String,
    pub hub_key: HubKey,
    pub native_id: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceAttentionEntryDto {
    pub id: String,
    pub kind: &'static str,
    pub status: &'static str,
    pub device: DeviceAttentionDeviceDto,
    pub evidence: DeviceAttentionEvidenceDto,
    pub guidance: &'static str,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceAttentionDeviceDto {
    pub name: String,
    pub native_id: String,
    pub hub_type: String,
    pub hub_address: String,
    pub device_type: DeviceType,
}

#[derive(Clone, Debug, Serialize)]
pub struct DeviceAttentionEvidenceDto {
    pub last_proof_at: u64,
    pub first_failure_at: u64,
    pub last_failure_at: u64,
    pub failure_count: u32,
    pub failure_classes: Vec<DeviceReachabilityFailureClass>,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snoozed_until: Option<u64>,
}

impl DeviceHealthLedger {
    pub fn normalized(mut self) -> Self {
        if self.schema_version != DEVICE_HEALTH_SCHEMA_VERSION {
            return Self::default_with_schema();
        }
        self.schema_version = DEVICE_HEALTH_SCHEMA_VERSION;
        self.records.retain(|_, record| {
            !record.canonical_id.is_empty()
                && !record.native_id.is_empty()
                && !record.fabric_fingerprint.is_empty()
                && record.last_proof_at > 0
        });
        for record in self.records.values_mut() {
            record.failure_count = record.failure_count.min(10_000);
            if record.failure_count == 0 {
                record.reset_failure_window();
            }
        }
        while self.records.len() > MAX_DEVICE_HEALTH_RECORDS {
            let Some(oldest) = self
                .records
                .iter()
                .min_by_key(|(_, record)| {
                    record
                        .resolved_at
                        .or(record.last_failure_at)
                        .unwrap_or(record.last_proof_at)
                })
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.records.remove(&oldest);
        }
        self.controllers.clear();
        self.last_persisted_proof = self
            .records
            .iter()
            .map(|(key, record)| (key.clone(), record.last_proof_at))
            .collect();
        self
    }

    pub fn default_with_schema() -> Self {
        Self {
            schema_version: DEVICE_HEALTH_SCHEMA_VERSION,
            records: BTreeMap::new(),
            controllers: HashMap::new(),
            last_persisted_proof: HashMap::new(),
        }
    }

    pub fn records(&self) -> impl Iterator<Item = &DeviceHealthRecord> {
        self.records.values()
    }

    pub fn note_controller_connected(&mut self, hub_key: &HubKey, now: u64) {
        let controller = self.controllers.entry(hub_key.to_string()).or_default();
        controller.connected = true;
        controller.grace_until = controller
            .grace_until
            .max(now.saturating_add(CONTROLLER_RESTART_GRACE_SECS));
    }

    pub fn note_controller_disconnected(&mut self, hub_key: &HubKey) {
        self.controllers
            .entry(hub_key.to_string())
            .or_default()
            .connected = false;
    }

    pub fn note_controller_stream_reset(&mut self, hub_key: &HubKey, now: u64) {
        let controller = self.controllers.entry(hub_key.to_string()).or_default();
        controller.grace_until = now.saturating_add(CONTROLLER_RESTART_GRACE_SECS);
    }

    pub fn note_proof(
        &mut self,
        canonical_id: &str,
        hub_key: &HubKey,
        native_id: &str,
        fabric_id: &str,
        controller_stream_id: Option<&str>,
        now: u64,
    ) -> DeviceHealthUpdate {
        if canonical_id.is_empty() || native_id.is_empty() || fabric_id.is_empty() || now == 0 {
            return DeviceHealthUpdate::default();
        }
        let fabric_fingerprint = privacy_fingerprint(fabric_id);
        let controller_stream_fingerprint = controller_stream_id
            .filter(|value| !value.is_empty())
            .map(privacy_fingerprint);
        let key = DeviceHealthRecord::key(hub_key, native_id);
        let identity_changed = self.records.get(&key).is_some_and(|record| {
            record.canonical_id != canonical_id || record.fabric_fingerprint != fabric_fingerprint
        });
        if identity_changed {
            self.records.remove(&key);
            self.last_persisted_proof.remove(&key);
        }

        let record = self
            .records
            .entry(key.clone())
            .or_insert_with(|| DeviceHealthRecord {
                canonical_id: canonical_id.to_string(),
                hub_key: hub_key.clone(),
                native_id: native_id.to_string(),
                fabric_fingerprint: fabric_fingerprint.clone(),
                controller_stream_fingerprint: controller_stream_fingerprint.clone(),
                last_proof_at: now,
                first_failure_at: None,
                last_failure_at: None,
                failure_count: 0,
                failure_classes: BTreeSet::new(),
                status: DeviceHealthStatus::Monitoring,
                entry_id: None,
                created_at: None,
                snoozed_until: None,
                resolved_at: None,
                recovery_correlation_id: None,
            });
        let was_attention = record.status.is_attention();
        record.last_proof_at = record.last_proof_at.max(now);
        if let Some(stream_id) = controller_stream_fingerprint {
            record.controller_stream_fingerprint = Some(stream_id);
        }

        let mut update = DeviceHealthUpdate::default();
        if was_attention {
            record.status = DeviceHealthStatus::Recovered;
            record.resolved_at = Some(now);
            record.snoozed_until = None;
            update.transitions.push((
                record.entry_id.clone().unwrap_or_else(|| key.clone()),
                DeviceHealthTransition::Recovered,
            ));
            update.persist = true;
        } else if record.status == DeviceHealthStatus::Monitoring {
            record.reset_failure_window();
        }

        let last_persisted = self.last_persisted_proof.get(&key).copied().unwrap_or(0);
        if update.persist
            || identity_changed
            || last_persisted == 0
            || now.saturating_sub(last_persisted) >= PROOF_PERSIST_INTERVAL_SECS
        {
            self.last_persisted_proof.insert(key, now);
            update.persist = true;
        }
        update
    }

    pub fn note_failure(
        &mut self,
        canonical_id: &str,
        hub_key: &HubKey,
        native_id: &str,
        fabric_id: &str,
        controller_stream_id: Option<&str>,
        class: DeviceReachabilityFailureClass,
        now: u64,
    ) -> DeviceHealthUpdate {
        let key = DeviceHealthRecord::key(hub_key, native_id);
        let fabric_fingerprint = privacy_fingerprint(fabric_id);
        let controller_stream_fingerprint = controller_stream_id
            .filter(|value| !value.is_empty())
            .map(privacy_fingerprint);
        let Some(record) = self.records.get_mut(&key) else {
            // Prior proof on this exact fabric is an admission prerequisite.
            return DeviceHealthUpdate::default();
        };
        if record.canonical_id != canonical_id || record.fabric_fingerprint != fabric_fingerprint {
            return DeviceHealthUpdate::default();
        }
        if let Some(stream_id) = controller_stream_fingerprint {
            record.controller_stream_fingerprint = Some(stream_id);
        }
        if matches!(
            record.status,
            DeviceHealthStatus::Recovered | DeviceHealthStatus::Removed
        ) {
            record.status = DeviceHealthStatus::Monitoring;
            record.entry_id = None;
            record.created_at = None;
            record.resolved_at = None;
            record.snoozed_until = None;
            record.recovery_correlation_id = None;
            record.reset_failure_window();
        }

        let last_failure = record.last_failure_at.unwrap_or(0);
        let class_is_new = !record.failure_classes.contains(&class);
        let separated = now.saturating_sub(last_failure) >= UNREACHABLE_FAILURE_SEPARATION_SECS;
        if !class_is_new && !separated {
            return DeviceHealthUpdate::default();
        }
        record.first_failure_at.get_or_insert(now);
        record.last_failure_at = Some(now);
        record.failure_count = record.failure_count.saturating_add(1).min(10_000);
        record.failure_classes.insert(class);
        DeviceHealthUpdate {
            persist: true,
            transitions: Vec::new(),
        }
    }

    pub fn evaluate(&mut self, now: u64) -> DeviceHealthUpdate {
        let snapshot: Vec<(String, DeviceHealthRecord)> = self
            .records
            .iter()
            .map(|(key, record)| (key.clone(), record.clone()))
            .collect();
        let mut update = DeviceHealthUpdate::default();
        for (key, candidate) in &snapshot {
            if !matches!(
                candidate.status,
                DeviceHealthStatus::Monitoring | DeviceHealthStatus::Snoozed
            ) {
                continue;
            }
            if candidate.status == DeviceHealthStatus::Snoozed
                && candidate.snoozed_until.is_some_and(|until| until > now)
            {
                continue;
            }
            let Some(first_failure) = candidate.first_failure_at else {
                continue;
            };
            let Some(last_failure) = candidate.last_failure_at else {
                continue;
            };
            let controller_healthy = self
                .controllers
                .get(&candidate.hub_key.to_string())
                .is_some_and(|controller| controller.connected && now >= controller.grace_until);
            let peer_healthy = snapshot.iter().any(|(peer_key, peer)| {
                peer_key != key
                    && peer.hub_key == candidate.hub_key
                    && peer.fabric_fingerprint == candidate.fabric_fingerprint
                    && peer.last_proof_at >= first_failure
                    && peer.last_proof_at <= now
                    && now.saturating_sub(peer.last_proof_at) <= HEALTHY_PEER_MAX_AGE_SECS
            });
            if !controller_healthy
                || !peer_healthy
                || now.saturating_sub(candidate.last_proof_at) < UNREACHABLE_NO_PROOF_SECS
                || last_failure.saturating_sub(first_failure) < UNREACHABLE_FAILURE_SPAN_SECS
                || candidate.failure_count < UNREACHABLE_MIN_FAILURES
            {
                continue;
            }
            let Some(record) = self.records.get_mut(key) else {
                continue;
            };
            record.status = DeviceHealthStatus::Pending;
            record.snoozed_until = None;
            let entry_id = record
                .entry_id
                .get_or_insert_with(generate_entry_id)
                .clone();
            record.created_at.get_or_insert(now);
            update
                .transitions
                .push((entry_id, DeviceHealthTransition::Admitted));
            update.persist = true;
        }
        update
    }

    pub fn reconcile_active(
        &mut self,
        active: &HashSet<ActiveDeviceHealthIdentity>,
        now: u64,
    ) -> DeviceHealthUpdate {
        let mut update = DeviceHealthUpdate::default();
        for record in self.records.values_mut() {
            let identity = ActiveDeviceHealthIdentity {
                canonical_id: record.canonical_id.clone(),
                hub_key: record.hub_key.clone(),
                native_id: record.native_id.clone(),
            };
            if active.contains(&identity) || record.status == DeviceHealthStatus::Removed {
                continue;
            }
            if record.status.is_attention() {
                update.transitions.push((
                    record.entry_id.clone().unwrap_or_else(generate_entry_id),
                    DeviceHealthTransition::Removed,
                ));
            }
            record.status = DeviceHealthStatus::Removed;
            record.resolved_at = Some(now);
            record.snoozed_until = None;
            update.persist = true;
        }
        let before = self.records.len();
        self.records.retain(|_, record| {
            !matches!(
                record.status,
                DeviceHealthStatus::Recovered | DeviceHealthStatus::Removed
            ) || record
                .resolved_at
                .is_some_and(|resolved| now.saturating_sub(resolved) <= RESOLVED_RETENTION_SECS)
        });
        update.persist |= self.records.len() != before;
        update
    }

    pub fn visible_records(&self, _now: u64) -> Vec<&DeviceHealthRecord> {
        let mut records: Vec<_> = self
            .records
            .values()
            .filter(|record| match record.status {
                DeviceHealthStatus::Pending | DeviceHealthStatus::AwaitingRecovery => true,
                DeviceHealthStatus::Snoozed => false,
                _ => false,
            })
            .collect();
        records.sort_by(|left, right| {
            left.created_at
                .cmp(&right.created_at)
                .then_with(|| left.entry_id.cmp(&right.entry_id))
        });
        records
    }

    pub fn snooze(&mut self, entry_id: &str, now: u64) -> bool {
        let Some(record) = self
            .records
            .values_mut()
            .find(|record| record.entry_id.as_deref() == Some(entry_id))
        else {
            return false;
        };
        if !record.status.is_attention() {
            return false;
        }
        record.status = DeviceHealthStatus::Snoozed;
        record.snoozed_until = Some(now.saturating_add(UNREACHABLE_SNOOZE_SECS));
        true
    }

    pub fn await_recovery(&mut self, entry_id: &str, correlation_id: Option<&str>) -> bool {
        let Some(record) = self
            .records
            .values_mut()
            .find(|record| record.entry_id.as_deref() == Some(entry_id))
        else {
            return false;
        };
        if !record.status.is_attention() {
            return false;
        }
        record.status = DeviceHealthStatus::AwaitingRecovery;
        record.snoozed_until = None;
        record.recovery_correlation_id = correlation_id.map(str::to_string);
        true
    }

    pub fn mark_removal_selected(&mut self, entry_id: &str, correlation_id: Option<&str>) -> bool {
        let Some(record) = self
            .records
            .values_mut()
            .find(|record| record.entry_id.as_deref() == Some(entry_id))
        else {
            return false;
        };
        if !record.status.is_attention() {
            return false;
        }
        record.recovery_correlation_id = correlation_id.map(str::to_string);
        true
    }

    pub fn is_actionable(&self, entry_id: &str) -> bool {
        self.records.values().any(|record| {
            record.entry_id.as_deref() == Some(entry_id) && record.status.is_attention()
        })
    }

    pub fn recovery_correlation_for_entry(&self, entry_id: &str) -> Option<String> {
        self.records
            .values()
            .find(|record| record.entry_id.as_deref() == Some(entry_id))
            .and_then(|record| record.recovery_correlation_id.clone())
    }
}

impl Default for DeviceHealthLedger {
    fn default() -> Self {
        Self::default_with_schema()
    }
}

fn generate_entry_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    let mut value = String::with_capacity(32);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(value, "{byte:02x}");
    }
    format!("unreachable-{value}")
}

fn privacy_fingerprint(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut encoded = String::with_capacity(24);
    for byte in digest.iter().take(12) {
        use std::fmt::Write;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hub::HubType;

    fn key() -> HubKey {
        HubKey::new(HubType::new("matter"), "local")
    }

    fn admit_failure(
        ledger: &mut DeviceHealthLedger,
        native_id: &str,
        class: DeviceReachabilityFailureClass,
        now: u64,
    ) {
        let canonical_id = format!("canonical-{native_id}");
        ledger.note_failure(
            &canonical_id,
            &key(),
            native_id,
            "fabric-a",
            Some("stream-a"),
            class,
            now,
        );
    }

    fn ledger_with_entry() -> (DeviceHealthLedger, String, u64) {
        let mut ledger = DeviceHealthLedger::default();
        ledger.note_controller_connected(&key(), 1);
        for native_id in ["matter-1-1", "matter-2-1"] {
            ledger.note_proof(
                &format!("canonical-{native_id}"),
                &key(),
                native_id,
                "fabric-a",
                Some("stream-a"),
                1,
            );
        }
        let admitted_at = CONTROLLER_RESTART_GRACE_SECS + UNREACHABLE_FAILURE_SPAN_SECS + 2;
        for (class, at) in [
            (
                DeviceReachabilityFailureClass::Subscription,
                CONTROLLER_RESTART_GRACE_SECS + 2,
            ),
            (
                DeviceReachabilityFailureClass::Read,
                CONTROLLER_RESTART_GRACE_SECS + 10,
            ),
            (DeviceReachabilityFailureClass::Command, admitted_at),
        ] {
            admit_failure(&mut ledger, "matter-1-1", class, at);
        }
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            admitted_at,
        );
        ledger.evaluate(admitted_at);
        let entry_id = ledger.visible_records(admitted_at)[0]
            .entry_id
            .clone()
            .unwrap();
        (ledger, entry_id, admitted_at)
    }

    #[test]
    fn healthy_peer_and_spanning_failures_admit_exactly_one_entry() {
        let mut ledger = DeviceHealthLedger::default();
        ledger.note_controller_connected(&key(), 1);
        ledger.note_proof(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-a",
            Some("stream-a"),
            1,
        );
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            1,
        );

        let grace = CONTROLLER_RESTART_GRACE_SECS;
        admit_failure(
            &mut ledger,
            "matter-1-1",
            DeviceReachabilityFailureClass::Subscription,
            grace + 2,
        );
        admit_failure(
            &mut ledger,
            "matter-1-1",
            DeviceReachabilityFailureClass::Read,
            grace + 10,
        );
        admit_failure(
            &mut ledger,
            "matter-1-1",
            DeviceReachabilityFailureClass::Command,
            grace + UNREACHABLE_FAILURE_SPAN_SECS + 2,
        );
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            grace + UNREACHABLE_FAILURE_SPAN_SECS + 2,
        );

        let first = ledger.evaluate(grace + UNREACHABLE_FAILURE_SPAN_SECS + 2);
        assert_eq!(first.transitions.len(), 1);
        assert_eq!(ledger.visible_records(u64::MAX).len(), 1);
        assert!(ledger.evaluate(u64::MAX).transitions.is_empty());
        assert_eq!(ledger.visible_records(u64::MAX).len(), 1);
    }

    #[test]
    fn no_peer_or_controller_grace_suppresses_admission() {
        let mut ledger = DeviceHealthLedger::default();
        ledger.note_controller_connected(&key(), 1);
        ledger.note_proof(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-a",
            Some("stream-a"),
            1,
        );
        for (class, at) in [
            (DeviceReachabilityFailureClass::Subscription, 2),
            (DeviceReachabilityFailureClass::Read, 3),
            (
                DeviceReachabilityFailureClass::Command,
                UNREACHABLE_FAILURE_SPAN_SECS + 2,
            ),
        ] {
            admit_failure(&mut ledger, "matter-1-1", class, at);
        }
        assert!(ledger
            .evaluate(UNREACHABLE_FAILURE_SPAN_SECS + 2)
            .transitions
            .is_empty());

        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            UNREACHABLE_FAILURE_SPAN_SECS + 2,
        );
        ledger.note_controller_stream_reset(&key(), UNREACHABLE_FAILURE_SPAN_SECS + 2);
        assert!(ledger
            .evaluate(UNREACHABLE_FAILURE_SPAN_SECS + 3)
            .transitions
            .is_empty());
    }

    #[test]
    fn proof_resolves_snoozed_or_recovery_waiting_entry() {
        let mut ledger = DeviceHealthLedger::default();
        ledger.note_controller_connected(&key(), 1);
        ledger.note_proof(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-a",
            Some("stream-a"),
            1,
        );
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            1,
        );
        for (class, at) in [
            (DeviceReachabilityFailureClass::Subscription, 2),
            (DeviceReachabilityFailureClass::Read, 3),
            (
                DeviceReachabilityFailureClass::Command,
                UNREACHABLE_FAILURE_SPAN_SECS + 2,
            ),
        ] {
            admit_failure(&mut ledger, "matter-1-1", class, at);
        }
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            UNREACHABLE_FAILURE_SPAN_SECS + 2,
        );
        ledger.evaluate(UNREACHABLE_FAILURE_SPAN_SECS + 2);
        let entry_id = ledger.visible_records(u64::MAX)[0]
            .entry_id
            .clone()
            .unwrap();
        assert!(ledger.snooze(&entry_id, UNREACHABLE_FAILURE_SPAN_SECS + 3));
        assert!(ledger
            .visible_records(UNREACHABLE_FAILURE_SPAN_SECS + 4)
            .is_empty());
        assert!(ledger.await_recovery(&entry_id, Some("unreachable-device-test")));

        let update = ledger.note_proof(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-a",
            Some("stream-a"),
            UNREACHABLE_FAILURE_SPAN_SECS + 5,
        );
        assert_eq!(
            update.transitions,
            vec![(entry_id, DeviceHealthTransition::Recovered)]
        );
        assert!(ledger.visible_records(u64::MAX).is_empty());
    }

    #[test]
    fn persistence_fingerprints_fabric_and_stream_and_restart_requires_live_controller() {
        let (ledger, _, admitted_at) = ledger_with_entry();
        let json = serde_json::to_string(&ledger).unwrap();
        assert!(!json.contains("fabric-a"));
        assert!(!json.contains("stream-a"));

        let mut restarted: DeviceHealthLedger = serde_json::from_str(&json).unwrap();
        restarted = restarted.normalized();
        assert_eq!(restarted.visible_records(admitted_at).len(), 1);

        // A restart never restores ephemeral controller health. Even an
        // expired snooze cannot re-admit until a current controller stream
        // completes its grace period.
        let entry_id = restarted.visible_records(admitted_at)[0]
            .entry_id
            .clone()
            .unwrap();
        assert!(restarted.snooze(&entry_id, admitted_at));
        let after_snooze = admitted_at + UNREACHABLE_SNOOZE_SECS + 1;
        assert!(restarted.evaluate(after_snooze).transitions.is_empty());
        restarted.note_controller_connected(&key(), after_snooze);
        assert!(restarted
            .evaluate(after_snooze + CONTROLLER_RESTART_GRACE_SECS - 1)
            .transitions
            .is_empty());
    }

    #[test]
    fn snooze_expiry_resurfaces_the_same_entry_without_duplication() {
        let (mut ledger, entry_id, admitted_at) = ledger_with_entry();
        assert!(ledger.snooze(&entry_id, admitted_at));
        assert!(ledger
            .visible_records(admitted_at + UNREACHABLE_SNOOZE_SECS - 1)
            .is_empty());

        let resurfaced_at = admitted_at + UNREACHABLE_SNOOZE_SECS;
        assert!(ledger.evaluate(resurfaced_at).transitions.is_empty());
        assert!(ledger.visible_records(resurfaced_at).is_empty());
        ledger.note_proof(
            "canonical-matter-2-1",
            &key(),
            "matter-2-1",
            "fabric-a",
            Some("stream-a"),
            resurfaced_at,
        );
        let update = ledger.evaluate(resurfaced_at);
        assert_eq!(
            update.transitions,
            vec![(entry_id.clone(), DeviceHealthTransition::Admitted)]
        );
        let visible = ledger.visible_records(resurfaced_at);
        assert_eq!(visible.len(), 1);
        assert_eq!(visible[0].entry_id.as_deref(), Some(entry_id.as_str()));
        assert_eq!(visible[0].status, DeviceHealthStatus::Pending);
    }

    #[test]
    fn fabric_replacement_discards_old_failures_and_attention_state() {
        let (mut ledger, entry_id, admitted_at) = ledger_with_entry();
        assert!(ledger.is_actionable(&entry_id));

        let update = ledger.note_proof(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-b",
            Some("stream-b"),
            admitted_at + 1,
        );
        assert!(update.persist);
        assert!(update.transitions.is_empty());
        assert!(!ledger.is_actionable(&entry_id));
        assert!(ledger.visible_records(u64::MAX).is_empty());

        let ignored = ledger.note_failure(
            "canonical-matter-1-1",
            &key(),
            "matter-1-1",
            "fabric-a",
            Some("stream-a"),
            DeviceReachabilityFailureClass::Command,
            admitted_at + 2,
        );
        assert!(!ignored.persist);
    }

    #[test]
    fn topology_removal_resolves_with_the_saved_journey_correlation() {
        let (mut ledger, entry_id, admitted_at) = ledger_with_entry();
        assert!(ledger.mark_removal_selected(&entry_id, Some("journey-1")));
        let update = ledger.reconcile_active(&HashSet::new(), admitted_at + 1);

        assert_eq!(
            update.transitions,
            vec![(entry_id.clone(), DeviceHealthTransition::Removed)]
        );
        assert_eq!(
            ledger.recovery_correlation_for_entry(&entry_id).as_deref(),
            Some("journey-1")
        );
        assert!(ledger.visible_records(u64::MAX).is_empty());
    }
}
