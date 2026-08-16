//! Low-write aggregation of authoritative observed light power.
//!
//! The hot path performs bounded in-memory work at the existing observed-power
//! cache boundary. Persistence and cloud delivery operate on absolute segment
//! snapshots outside the global state lock.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{TimeZone, Utc};
use rand::RngCore;
use serde::{Deserialize, Serialize};

use crate::state::SharedState;

pub const LIGHT_USAGE_SCHEMA_VERSION: u8 = 1;
pub const LIGHT_USAGE_LEDGER_BYTES_LIMIT: u64 = 2 * 1024 * 1024;
pub const LIGHT_USAGE_CLOUD_BATCH_LIMIT: usize = 512;

const LIGHT_USAGE_SEGMENT_LIMIT: usize = 4096;
const LIGHT_USAGE_RETENTION_MS: u64 = 35 * 24 * 60 * 60 * 1000;
const LIGHT_USAGE_CHECKPOINT_INTERVAL: Duration = Duration::from_secs(15 * 60);
const LIGHT_USAGE_CLOUD_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const LIGHT_USAGE_CLOCK_TOLERANCE_MS: u64 = 20 * 60 * 1000;

fn default_schema_version() -> u8 {
    LIGHT_USAGE_SCHEMA_VERSION
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LightUsageSubjectKind {
    Bulb,
    RoomAggregate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LightUsageObservationSource {
    Periodic,
    SyncPoll,
    LiveSubscription,
    AuthoritativeRefresh,
}

impl LightUsageObservationSource {
    fn is_live(self) -> bool {
        self == Self::LiveSubscription
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LightUsageSourceSummary {
    #[serde(default)]
    pub periodic: u64,
    #[serde(default)]
    pub sync_poll: u64,
    #[serde(default)]
    pub live_subscription: u64,
    #[serde(default)]
    pub authoritative_refresh: u64,
}

impl LightUsageSourceSummary {
    fn record(&mut self, source: LightUsageObservationSource) {
        let count = match source {
            LightUsageObservationSource::Periodic => &mut self.periodic,
            LightUsageObservationSource::SyncPoll => &mut self.sync_poll,
            LightUsageObservationSource::LiveSubscription => &mut self.live_subscription,
            LightUsageObservationSource::AuthoritativeRefresh => &mut self.authoritative_refresh,
        };
        *count = count.saturating_add(1);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LightUsageSegment {
    pub segment_id: String,
    pub subject_id: String,
    pub subject_kind: LightUsageSubjectKind,
    pub usage_date: String,
    pub revision: u64,
    #[serde(default)]
    pub synced_revision: u64,
    pub on_ms: u64,
    pub covered_ms: u64,
    pub transition_uncertainty_ms: u64,
    pub observation_count: u64,
    pub transition_count: u64,
    pub first_observed_at_epoch_ms: u64,
    pub last_observed_at_epoch_ms: u64,
    #[serde(default)]
    pub source_summary: LightUsageSourceSummary,
}

impl LightUsageSegment {
    fn normalize(&mut self) {
        self.revision = self.revision.max(1);
        self.synced_revision = self.synced_revision.min(self.revision);
        self.on_ms = self.on_ms.min(self.covered_ms);
        self.transition_uncertainty_ms = self.transition_uncertainty_ms.min(self.covered_ms);
        if self.last_observed_at_epoch_ms < self.first_observed_at_epoch_ms {
            self.last_observed_at_epoch_ms = self.first_observed_at_epoch_ms;
        }
    }

    fn is_dirty(&self) -> bool {
        self.revision > self.synced_revision
    }
}

#[derive(Clone, Debug)]
struct LightUsageBaseline {
    lights_on: bool,
    observed_at_epoch_ms: u64,
    observed_at_instant: Instant,
    segment_id: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LightUsageLedger {
    #[serde(default = "default_schema_version")]
    pub schema_version: u8,
    #[serde(default)]
    pub segments: HashMap<String, LightUsageSegment>,
    #[serde(default)]
    pub segment_order: VecDeque<String>,
    #[serde(default)]
    pub dropped_segment_count: u64,
    #[serde(default)]
    pub clock_discontinuity_count: u64,
    #[serde(default)]
    pub excluded_source_count: u64,
    #[serde(default)]
    pub last_checkpoint_epoch_ms: Option<u64>,
    #[serde(default)]
    pub last_cloud_attempt_epoch_ms: Option<u64>,
    #[serde(default)]
    pub last_cloud_ack_epoch_ms: Option<u64>,
    #[serde(default)]
    pub cloud_status: Option<String>,
    #[serde(default)]
    pub pending_batch_id: Option<String>,

    #[serde(skip)]
    baselines: HashMap<String, LightUsageBaseline>,
    #[serde(skip)]
    dirty: bool,
    #[serde(skip)]
    generation: u64,
    #[serde(skip)]
    checkpoint_in_progress: bool,
    #[serde(skip)]
    checkpoint_anchor: Option<Instant>,
    #[serde(skip)]
    force_checkpoint: bool,
    #[serde(skip)]
    cloud_in_progress: bool,
    #[serde(skip)]
    cloud_anchor: Option<Instant>,
    #[serde(skip)]
    force_cloud: bool,
    #[serde(skip)]
    shutdown_storage: Option<Arc<dyn crate::storage::Storage>>,
}

impl Default for LightUsageLedger {
    fn default() -> Self {
        Self {
            schema_version: LIGHT_USAGE_SCHEMA_VERSION,
            segments: HashMap::new(),
            segment_order: VecDeque::new(),
            dropped_segment_count: 0,
            clock_discontinuity_count: 0,
            excluded_source_count: 0,
            last_checkpoint_epoch_ms: None,
            last_cloud_attempt_epoch_ms: None,
            last_cloud_ack_epoch_ms: None,
            cloud_status: None,
            pending_batch_id: None,
            baselines: HashMap::new(),
            dirty: false,
            generation: 0,
            checkpoint_in_progress: false,
            checkpoint_anchor: None,
            force_checkpoint: false,
            cloud_in_progress: false,
            cloud_anchor: None,
            force_cloud: false,
            shutdown_storage: None,
        }
    }
}

impl Drop for LightUsageLedger {
    fn drop(&mut self) {
        let Some(storage) = self.shutdown_storage.take() else {
            return;
        };
        if !self.dirty {
            return;
        }
        let snapshot = self.persisted_snapshot(crate::state::current_epoch_ms());
        if let Err(error) = storage.save_light_usage_ledger(&snapshot) {
            log::warn!(
                target: "sys",
                "Failed to flush light usage ledger during clean shutdown: {error:#}"
            );
        }
    }
}

impl LightUsageLedger {
    pub fn normalized(mut self, now_epoch_ms: u64) -> Result<Self> {
        if self.schema_version > LIGHT_USAGE_SCHEMA_VERSION {
            anyhow::bail!(
                "light usage ledger schema {} is newer than supported schema {}",
                self.schema_version,
                LIGHT_USAGE_SCHEMA_VERSION
            );
        }
        self.schema_version = LIGHT_USAGE_SCHEMA_VERSION;
        self.segments.retain(|segment_id, segment| {
            segment.segment_id == *segment_id
                && valid_opaque_id(segment_id)
                && valid_subject_id(&segment.subject_id)
                && valid_usage_date(&segment.usage_date)
        });
        for segment in self.segments.values_mut() {
            segment.normalize();
        }

        let mut seen = HashSet::new();
        self.segment_order.retain(|segment_id| {
            self.segments.contains_key(segment_id) && seen.insert(segment_id.clone())
        });
        let mut missing = self
            .segments
            .values()
            .filter(|segment| !seen.contains(&segment.segment_id))
            .map(|segment| {
                (
                    segment.first_observed_at_epoch_ms,
                    segment.segment_id.clone(),
                )
            })
            .collect::<Vec<_>>();
        missing.sort();
        self.segment_order
            .extend(missing.into_iter().map(|(_, segment_id)| segment_id));

        self.baselines.clear();
        self.dirty = self.segments.values().any(LightUsageSegment::is_dirty);
        self.generation = 0;
        self.checkpoint_in_progress = false;
        self.checkpoint_anchor = None;
        self.force_checkpoint = false;
        self.cloud_in_progress = false;
        self.cloud_anchor = None;
        self.force_cloud = self.dirty;
        self.shutdown_storage = None;
        self.prune_closed_segments(now_epoch_ms);
        Ok(self)
    }

    pub fn configure_shutdown_flush(&mut self, storage: Arc<dyn crate::storage::Storage>) {
        self.shutdown_storage = Some(storage);
    }

    pub fn disable_shutdown_flush(&mut self) {
        self.shutdown_storage = None;
    }

    pub fn record_excluded_source(&mut self, now: Instant) {
        self.excluded_source_count = self.excluded_source_count.saturating_add(1);
        self.mark_dirty(now);
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_observation(
        &mut self,
        subject_id: &str,
        subject_kind: LightUsageSubjectKind,
        lights_on: bool,
        source: LightUsageObservationSource,
        observed_at_instant: Instant,
        observed_at_epoch_ms: u64,
        continuity_budget: Duration,
    ) {
        if !valid_subject_id(subject_id) {
            return;
        }

        let Some(previous) = self.baselines.get(subject_id).cloned() else {
            self.start_baseline(
                subject_id,
                subject_kind,
                lights_on,
                source,
                observed_at_instant,
                observed_at_epoch_ms,
            );
            return;
        };
        if self
            .segments
            .get(&previous.segment_id)
            .is_none_or(|segment| segment.subject_kind != subject_kind)
        {
            self.start_baseline(
                subject_id,
                subject_kind,
                lights_on,
                source,
                observed_at_instant,
                observed_at_epoch_ms,
            );
            return;
        }

        let elapsed = observed_at_instant.saturating_duration_since(previous.observed_at_instant);
        let elapsed_ms = elapsed.as_millis().min(u128::from(u64::MAX)) as u64;
        let wall_delta_ms = observed_at_epoch_ms.checked_sub(previous.observed_at_epoch_ms);
        let wall_discontinuous = wall_delta_ms.is_none_or(|wall_delta_ms| {
            wall_delta_ms.abs_diff(elapsed_ms) > LIGHT_USAGE_CLOCK_TOLERANCE_MS
        });
        if elapsed > continuity_budget || wall_discontinuous {
            log::debug!(
                target: "cmd",
                "light_usage stage=gap_discarded reason={} subject_kind={:?}",
                if wall_discontinuous { "wall_clock" } else { "stale" },
                subject_kind
            );
            if wall_discontinuous {
                self.clock_discontinuity_count = self.clock_discontinuity_count.saturating_add(1);
            }
            self.start_baseline(
                subject_id,
                subject_kind,
                lights_on,
                source,
                observed_at_instant,
                observed_at_epoch_ms,
            );
            return;
        }

        let changed = previous.lights_on != lights_on;
        let (on_ms, uncertainty_ms) = if !changed || source.is_live() {
            (u64::from(previous.lights_on).saturating_mul(elapsed_ms), 0)
        } else {
            let midpoint = elapsed_ms / 2;
            (midpoint, midpoint)
        };

        let previous_date = usage_date(previous.observed_at_epoch_ms);
        let current_date = usage_date(observed_at_epoch_ms);
        if previous_date == current_date {
            self.accrue_segment(
                &previous.segment_id,
                elapsed_ms,
                on_ms,
                uncertainty_ms,
                changed,
                source,
                observed_at_epoch_ms,
                true,
            );
            self.baselines.insert(
                subject_id.to_string(),
                LightUsageBaseline {
                    lights_on,
                    observed_at_epoch_ms,
                    observed_at_instant,
                    segment_id: previous.segment_id,
                },
            );
            if changed {
                log::debug!(
                    target: "cmd",
                    "light_usage stage=interval_accrued subject_kind={:?} transition=true covered_ms={} uncertainty_ms={}",
                    subject_kind,
                    elapsed_ms,
                    uncertainty_ms
                );
            }
        } else {
            let wall_total = wall_delta_ms.unwrap_or(elapsed_ms).max(1);
            let boundary = next_utc_midnight_epoch_ms(previous.observed_at_epoch_ms)
                .unwrap_or(observed_at_epoch_ms);
            let before_wall = boundary
                .saturating_sub(previous.observed_at_epoch_ms)
                .min(wall_total);
            let before_covered = proportional(elapsed_ms, before_wall, wall_total);
            let before_on = proportional(on_ms, before_covered, elapsed_ms.max(1));
            let before_uncertainty =
                proportional(uncertainty_ms, before_covered, elapsed_ms.max(1));
            self.accrue_segment(
                &previous.segment_id,
                before_covered,
                before_on,
                before_uncertainty,
                false,
                source,
                boundary.saturating_sub(1),
                false,
            );

            let Some(current_segment_id) = self.new_segment(
                subject_id,
                subject_kind,
                &current_date,
                source,
                observed_at_epoch_ms,
                false,
            ) else {
                self.baselines.remove(subject_id);
                self.mark_dirty(observed_at_instant);
                return;
            };
            self.accrue_segment(
                &current_segment_id,
                elapsed_ms.saturating_sub(before_covered),
                on_ms.saturating_sub(before_on),
                uncertainty_ms.saturating_sub(before_uncertainty),
                changed,
                source,
                observed_at_epoch_ms,
                true,
            );
            self.force_checkpoint = true;
            self.force_cloud = true;
            self.baselines.insert(
                subject_id.to_string(),
                LightUsageBaseline {
                    lights_on,
                    observed_at_epoch_ms,
                    observed_at_instant,
                    segment_id: current_segment_id,
                },
            );
        }
        self.mark_dirty(observed_at_instant);
    }

    pub fn close_subject(&mut self, subject_id: &str) {
        self.baselines.remove(subject_id);
    }

    pub fn force_cloud_upload(&mut self, now: Instant) {
        if self.segments.values().any(LightUsageSegment::is_dirty) {
            self.force_cloud = true;
            self.cloud_anchor.get_or_insert(now);
        }
    }

    pub fn mark_cloud_unavailable(&mut self, status: &str, now: Instant) {
        self.cloud_in_progress = false;
        self.force_cloud = false;
        self.cloud_status = Some(
            match status {
                "disabled" => "disabled",
                _ => "not_configured",
            }
            .to_string(),
        );
        self.mark_dirty(now);
    }

    pub fn cloud_upload_due(&mut self, now: Instant) -> bool {
        if self.cloud_in_progress || !self.segments.values().any(LightUsageSegment::is_dirty) {
            return false;
        }
        let anchor = self.cloud_anchor.get_or_insert(now);
        self.force_cloud || now.saturating_duration_since(*anchor) >= LIGHT_USAGE_CLOUD_INTERVAL
    }

    pub fn begin_cloud_upload(
        &mut self,
        now: Instant,
        now_epoch_ms: u64,
    ) -> Option<LightUsageCloudBatch> {
        if !self.cloud_upload_due(now) {
            return None;
        }
        let batch_id = self
            .pending_batch_id
            .clone()
            .unwrap_or_else(random_opaque_id);
        self.pending_batch_id = Some(batch_id.clone());
        let mut segments = self
            .segments
            .values()
            .filter(|segment| segment.is_dirty())
            .cloned()
            .collect::<Vec<_>>();
        segments.sort_by(|left, right| {
            left.usage_date
                .cmp(&right.usage_date)
                .then(left.segment_id.cmp(&right.segment_id))
        });
        if segments.is_empty() {
            return None;
        }
        let backlog_capped = segments.len() > LIGHT_USAGE_CLOUD_BATCH_LIMIT;
        self.cloud_in_progress = true;
        self.force_cloud = false;
        self.last_cloud_attempt_epoch_ms = Some(now_epoch_ms);
        self.cloud_status = Some("pending".to_string());
        self.mark_dirty(now);
        Some(LightUsageCloudBatch {
            schema_version: LIGHT_USAGE_SCHEMA_VERSION,
            batch_id,
            backlog_capped,
            segments: segments
                .into_iter()
                .take(LIGHT_USAGE_CLOUD_BATCH_LIMIT)
                .map(LightUsageUploadSegment::from)
                .collect(),
        })
    }

    pub fn complete_cloud_upload(
        &mut self,
        batch: &LightUsageCloudBatch,
        acknowledged: bool,
        now: Instant,
        now_epoch_ms: u64,
    ) {
        self.cloud_in_progress = false;
        self.cloud_anchor = Some(now);
        if acknowledged && self.pending_batch_id.as_deref() == Some(batch.batch_id.as_str()) {
            for uploaded in &batch.segments {
                if let Some(segment) = self.segments.get_mut(&uploaded.segment_id) {
                    segment.synced_revision = segment
                        .synced_revision
                        .max(uploaded.revision.min(segment.revision));
                }
            }
            self.last_cloud_ack_epoch_ms = Some(now_epoch_ms);
            self.cloud_status = Some(
                if batch.backlog_capped {
                    "backlog_capped"
                } else {
                    "ok"
                }
                .to_string(),
            );
            self.pending_batch_id = None;
            self.force_cloud = self.segments.values().any(LightUsageSegment::is_dirty);
            log::debug!(
                target: "cmd",
                "light_usage stage=batch_acked segment_count={} backlog_capped={}",
                batch.segments.len(),
                batch.backlog_capped
            );
        } else {
            self.cloud_status = Some("schema_unacknowledged".to_string());
            log::debug!(
                target: "cmd",
                "light_usage stage=batch_retained reason=schema_unacknowledged segment_count={}",
                batch.segments.len()
            );
        }
        self.mark_dirty(now);
    }

    pub fn fail_cloud_upload(&mut self, status: &str, now: Instant) {
        self.cloud_in_progress = false;
        self.cloud_anchor = Some(now);
        self.cloud_status = Some(
            match status {
                "auth_failed" => "auth_failed",
                _ => "failed",
            }
            .to_string(),
        );
        log::debug!(
            target: "cmd",
            "light_usage stage=batch_retained reason={}",
            self.cloud_status.as_deref().unwrap_or("failed")
        );
        self.mark_dirty(now);
    }

    pub fn diagnostics(&self, now_epoch_ms: u64) -> LightUsageDiagnostics {
        let dirty_segment_count = self
            .segments
            .values()
            .filter(|segment| segment.is_dirty())
            .count();
        let acknowledged_segment_count = self.segments.len().saturating_sub(dirty_segment_count);
        let bulb_subject_count = self
            .segments
            .values()
            .filter(|segment| segment.subject_kind == LightUsageSubjectKind::Bulb)
            .map(|segment| segment.subject_id.as_str())
            .collect::<HashSet<_>>()
            .len();
        let room_aggregate_subject_count = self
            .segments
            .values()
            .filter(|segment| segment.subject_kind == LightUsageSubjectKind::RoomAggregate)
            .map(|segment| segment.subject_id.as_str())
            .collect::<HashSet<_>>()
            .len();
        LightUsageDiagnostics {
            schema_version: self.schema_version,
            subject_count: bulb_subject_count.saturating_add(room_aggregate_subject_count),
            bulb_subject_count,
            room_aggregate_subject_count,
            open_subject_count: self.baselines.len(),
            segment_count: self.segments.len(),
            dirty_segment_count,
            acknowledged_segment_count,
            on_ms: self
                .segments
                .values()
                .fold(0_u64, |total, segment| total.saturating_add(segment.on_ms)),
            covered_ms: self.segments.values().fold(0_u64, |total, segment| {
                total.saturating_add(segment.covered_ms)
            }),
            transition_uncertainty_ms: self.segments.values().fold(0_u64, |total, segment| {
                total.saturating_add(segment.transition_uncertainty_ms)
            }),
            observation_count: self.segments.values().fold(0_u64, |total, segment| {
                total.saturating_add(segment.observation_count)
            }),
            dropped_segment_count: self.dropped_segment_count,
            clock_discontinuity_count: self.clock_discontinuity_count,
            excluded_source_count: self.excluded_source_count,
            checkpoint_age_ms: age_ms(now_epoch_ms, self.last_checkpoint_epoch_ms),
            cloud_attempt_age_ms: age_ms(now_epoch_ms, self.last_cloud_attempt_epoch_ms),
            cloud_ack_age_ms: age_ms(now_epoch_ms, self.last_cloud_ack_epoch_ms),
            cloud_status: self
                .cloud_status
                .clone()
                .unwrap_or_else(|| "not_configured".to_string()),
            pending_batch: self.pending_batch_id.is_some(),
            oldest_unsynced_day: self
                .segments
                .values()
                .filter(|segment| segment.is_dirty())
                .map(|segment| segment.usage_date.as_str())
                .min()
                .map(str::to_string),
            segment_limit: LIGHT_USAGE_SEGMENT_LIMIT,
            ledger_bytes_limit: LIGHT_USAGE_LEDGER_BYTES_LIMIT,
            checkpoint_interval_ms: LIGHT_USAGE_CHECKPOINT_INTERVAL.as_millis() as u64,
            cloud_interval_ms: LIGHT_USAGE_CLOUD_INTERVAL.as_millis() as u64,
            cloud_batch_limit: LIGHT_USAGE_CLOUD_BATCH_LIMIT,
        }
    }

    pub fn persisted_snapshot(&self, checkpoint_epoch_ms: u64) -> Self {
        let mut snapshot = self.clone();
        snapshot.last_checkpoint_epoch_ms = Some(checkpoint_epoch_ms);
        snapshot.baselines.clear();
        snapshot.dirty = false;
        snapshot.generation = 0;
        snapshot.checkpoint_in_progress = false;
        snapshot.checkpoint_anchor = None;
        snapshot.force_checkpoint = false;
        snapshot.cloud_in_progress = false;
        snapshot.cloud_anchor = None;
        snapshot.force_cloud = false;
        snapshot.shutdown_storage = None;
        snapshot
    }

    fn begin_checkpoint(&mut self, now: Instant, now_epoch_ms: u64) -> Option<(u64, Self)> {
        if self.checkpoint_in_progress || !self.dirty {
            return None;
        }
        let anchor = self.checkpoint_anchor.get_or_insert(now);
        if !self.force_checkpoint
            && now.saturating_duration_since(*anchor) < LIGHT_USAGE_CHECKPOINT_INTERVAL
        {
            return None;
        }
        self.checkpoint_in_progress = true;
        self.force_checkpoint = false;
        Some((self.generation, self.persisted_snapshot(now_epoch_ms)))
    }

    fn complete_checkpoint(
        &mut self,
        generation: u64,
        succeeded: bool,
        now: Instant,
        checkpoint_epoch_ms: u64,
    ) {
        self.checkpoint_in_progress = false;
        self.checkpoint_anchor = Some(now);
        if succeeded {
            self.last_checkpoint_epoch_ms = Some(checkpoint_epoch_ms);
            if self.generation == generation {
                self.dirty = false;
            }
        }
    }

    fn start_baseline(
        &mut self,
        subject_id: &str,
        subject_kind: LightUsageSubjectKind,
        lights_on: bool,
        source: LightUsageObservationSource,
        observed_at_instant: Instant,
        observed_at_epoch_ms: u64,
    ) {
        let Some(segment_id) = self.new_segment(
            subject_id,
            subject_kind,
            &usage_date(observed_at_epoch_ms),
            source,
            observed_at_epoch_ms,
            true,
        ) else {
            self.mark_dirty(observed_at_instant);
            return;
        };
        self.baselines.insert(
            subject_id.to_string(),
            LightUsageBaseline {
                lights_on,
                observed_at_epoch_ms,
                observed_at_instant,
                segment_id,
            },
        );
        log::debug!(
            target: "cmd",
            "light_usage stage=baseline_started subject_kind={:?} source={:?}",
            subject_kind,
            source
        );
        self.mark_dirty(observed_at_instant);
    }

    fn new_segment(
        &mut self,
        subject_id: &str,
        subject_kind: LightUsageSubjectKind,
        usage_date: &str,
        source: LightUsageObservationSource,
        observed_at_epoch_ms: u64,
        count_observation: bool,
    ) -> Option<String> {
        self.prune_closed_segments(observed_at_epoch_ms);
        if self.segments.len() >= LIGHT_USAGE_SEGMENT_LIMIT {
            let active_ids = self
                .baselines
                .values()
                .map(|baseline| baseline.segment_id.as_str())
                .collect::<HashSet<_>>();
            if let Some(index) = self
                .segment_order
                .iter()
                .position(|segment_id| !active_ids.contains(segment_id.as_str()))
            {
                if let Some(segment_id) = self.segment_order.remove(index) {
                    self.segments.remove(&segment_id);
                    self.dropped_segment_count = self.dropped_segment_count.saturating_add(1);
                }
            }
        }
        if self.segments.len() >= LIGHT_USAGE_SEGMENT_LIMIT {
            self.dropped_segment_count = self.dropped_segment_count.saturating_add(1);
            return None;
        }
        let mut segment_id = random_opaque_id();
        while self.segments.contains_key(&segment_id) {
            segment_id = random_opaque_id();
        }
        let mut source_summary = LightUsageSourceSummary::default();
        if count_observation {
            source_summary.record(source);
        }
        self.segments.insert(
            segment_id.clone(),
            LightUsageSegment {
                segment_id: segment_id.clone(),
                subject_id: subject_id.to_string(),
                subject_kind,
                usage_date: usage_date.to_string(),
                revision: 1,
                synced_revision: 0,
                on_ms: 0,
                covered_ms: 0,
                transition_uncertainty_ms: 0,
                observation_count: u64::from(count_observation),
                transition_count: 0,
                first_observed_at_epoch_ms: observed_at_epoch_ms,
                last_observed_at_epoch_ms: observed_at_epoch_ms,
                source_summary,
            },
        );
        self.segment_order.push_back(segment_id.clone());
        Some(segment_id)
    }

    #[allow(clippy::too_many_arguments)]
    fn accrue_segment(
        &mut self,
        segment_id: &str,
        covered_ms: u64,
        on_ms: u64,
        uncertainty_ms: u64,
        transition: bool,
        source: LightUsageObservationSource,
        observed_at_epoch_ms: u64,
        count_observation: bool,
    ) {
        let Some(segment) = self.segments.get_mut(segment_id) else {
            return;
        };
        segment.covered_ms = segment.covered_ms.saturating_add(covered_ms);
        segment.on_ms = segment.on_ms.saturating_add(on_ms).min(segment.covered_ms);
        segment.transition_uncertainty_ms = segment
            .transition_uncertainty_ms
            .saturating_add(uncertainty_ms)
            .min(segment.covered_ms);
        segment.transition_count = segment
            .transition_count
            .saturating_add(u64::from(transition));
        if count_observation {
            segment.observation_count = segment.observation_count.saturating_add(1);
            segment.source_summary.record(source);
        }
        segment.last_observed_at_epoch_ms =
            segment.last_observed_at_epoch_ms.max(observed_at_epoch_ms);
        segment.revision = segment.revision.saturating_add(1);
    }

    fn prune_closed_segments(&mut self, now_epoch_ms: u64) {
        let active_ids = self
            .baselines
            .values()
            .map(|baseline| baseline.segment_id.as_str())
            .collect::<HashSet<_>>();
        let retention_floor = now_epoch_ms.saturating_sub(LIGHT_USAGE_RETENTION_MS);
        let scan_limit = self.segment_order.len();
        for _ in 0..scan_limit {
            let Some(segment_id) = self.segment_order.pop_front() else {
                break;
            };
            let remove_for_age = self
                .segments
                .get(&segment_id)
                .is_some_and(|segment| segment.last_observed_at_epoch_ms < retention_floor);
            let remove_for_cap = self.segments.len() > LIGHT_USAGE_SEGMENT_LIMIT;
            if (remove_for_age || remove_for_cap) && !active_ids.contains(segment_id.as_str()) {
                self.segments.remove(&segment_id);
                self.dropped_segment_count = self.dropped_segment_count.saturating_add(1);
            } else {
                self.segment_order.push_back(segment_id);
            }
            if self.segments.len() <= LIGHT_USAGE_SEGMENT_LIMIT
                && self
                    .segment_order
                    .front()
                    .and_then(|id| self.segments.get(id))
                    .is_none_or(|segment| segment.last_observed_at_epoch_ms >= retention_floor)
            {
                break;
            }
        }
    }

    fn mark_dirty(&mut self, now: Instant) {
        self.dirty = true;
        self.generation = self.generation.saturating_add(1);
        self.checkpoint_anchor.get_or_insert(now);
        self.cloud_anchor.get_or_insert(now);
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LightUsageUploadSegment {
    pub segment_id: String,
    pub subject_id: String,
    pub subject_kind: LightUsageSubjectKind,
    pub usage_date: String,
    pub revision: u64,
    pub on_ms: u64,
    pub covered_ms: u64,
    pub transition_uncertainty_ms: u64,
    pub observation_count: u64,
    pub transition_count: u64,
    pub first_observed_at_epoch_ms: u64,
    pub last_observed_at_epoch_ms: u64,
    pub source_summary: LightUsageSourceSummary,
}

impl From<LightUsageSegment> for LightUsageUploadSegment {
    fn from(segment: LightUsageSegment) -> Self {
        Self {
            segment_id: segment.segment_id,
            subject_id: segment.subject_id,
            subject_kind: segment.subject_kind,
            usage_date: segment.usage_date,
            revision: segment.revision,
            on_ms: segment.on_ms,
            covered_ms: segment.covered_ms,
            transition_uncertainty_ms: segment.transition_uncertainty_ms,
            observation_count: segment.observation_count,
            transition_count: segment.transition_count,
            first_observed_at_epoch_ms: segment.first_observed_at_epoch_ms,
            last_observed_at_epoch_ms: segment.last_observed_at_epoch_ms,
            source_summary: segment.source_summary,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LightUsageCloudBatch {
    pub schema_version: u8,
    pub batch_id: String,
    pub backlog_capped: bool,
    pub segments: Vec<LightUsageUploadSegment>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct LightUsageDiagnostics {
    pub schema_version: u8,
    pub subject_count: usize,
    pub bulb_subject_count: usize,
    pub room_aggregate_subject_count: usize,
    pub open_subject_count: usize,
    pub segment_count: usize,
    pub dirty_segment_count: usize,
    pub acknowledged_segment_count: usize,
    pub on_ms: u64,
    pub covered_ms: u64,
    pub transition_uncertainty_ms: u64,
    pub observation_count: u64,
    pub dropped_segment_count: u64,
    pub clock_discontinuity_count: u64,
    pub excluded_source_count: u64,
    pub checkpoint_age_ms: Option<u64>,
    pub cloud_attempt_age_ms: Option<u64>,
    pub cloud_ack_age_ms: Option<u64>,
    pub cloud_status: String,
    pub pending_batch: bool,
    pub oldest_unsynced_day: Option<String>,
    pub segment_limit: usize,
    pub ledger_bytes_limit: u64,
    pub checkpoint_interval_ms: u64,
    pub cloud_interval_ms: u64,
    pub cloud_batch_limit: usize,
}

pub fn enqueue_due_maintenance(state: &SharedState) {
    enqueue_due_checkpoint(state);
    let cloud_due = {
        let now = Instant::now();
        state
            .lock()
            .ok()
            .is_some_and(|mut state| state.light_usage.cloud_upload_due(now))
    };
    if cloud_due {
        crate::activity_cloud::enqueue_recent_activity_upload(state);
    }
}

pub fn request_cloud_upload(state: &SharedState) {
    if let Ok(mut state) = state.lock() {
        state.light_usage.force_cloud_upload(Instant::now());
    }
    crate::activity_cloud::enqueue_recent_activity_upload(state);
}

pub fn flush_now(state: &SharedState) -> Result<()> {
    let now_epoch_ms = crate::state::current_epoch_ms();
    let (storage, snapshot) = {
        let state = state.lock().map_err(|_| anyhow::anyhow!("lock"))?;
        let storage = state
            .storage
            .clone()
            .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
        (storage, state.light_usage.persisted_snapshot(now_epoch_ms))
    };
    storage.save_light_usage_ledger(&snapshot)
}

fn enqueue_due_checkpoint(state: &SharedState) {
    let now = Instant::now();
    let checkpoint_epoch_ms = crate::state::current_epoch_ms();
    let prepared = {
        let Ok(mut state) = state.lock() else { return };
        let Some(storage) = state.storage.clone() else {
            return;
        };
        state
            .light_usage
            .begin_checkpoint(now, checkpoint_epoch_ms)
            .map(|(generation, snapshot)| (storage, generation, snapshot))
    };
    let Some((storage, generation, snapshot)) = prepared else {
        return;
    };
    let state_for_result = state.clone();
    let spawn = std::thread::Builder::new()
        .name("light-usage-checkpoint".to_string())
        .spawn(move || {
            let result = storage.save_light_usage_ledger(&snapshot);
            if let Err(error) = &result {
                log::warn!(target: "cmd", "Failed to checkpoint light usage ledger: {error:#}");
            } else {
                log::debug!(
                    target: "cmd",
                    "light_usage stage=checkpointed checkpoint_epoch_ms={checkpoint_epoch_ms}"
                );
            }
            if let Ok(mut state) = state_for_result.lock() {
                state.light_usage.complete_checkpoint(
                    generation,
                    result.is_ok(),
                    Instant::now(),
                    checkpoint_epoch_ms,
                );
            }
        });
    if let Err(error) = spawn {
        log::warn!(target: "cmd", "Failed to start light usage checkpoint worker: {error}");
        if let Ok(mut state) = state.lock() {
            state.light_usage.complete_checkpoint(
                generation,
                false,
                Instant::now(),
                checkpoint_epoch_ms,
            );
        }
    }
}

fn age_ms(now_epoch_ms: u64, timestamp: Option<u64>) -> Option<u64> {
    timestamp.map(|timestamp| now_epoch_ms.saturating_sub(timestamp))
}

fn usage_date(epoch_ms: u64) -> String {
    Utc.timestamp_millis_opt(epoch_ms as i64)
        .single()
        .unwrap_or_else(Utc::now)
        .date_naive()
        .format("%Y-%m-%d")
        .to_string()
}

fn next_utc_midnight_epoch_ms(epoch_ms: u64) -> Option<u64> {
    let date = Utc
        .timestamp_millis_opt(epoch_ms as i64)
        .single()?
        .date_naive();
    let next = date.succ_opt()?.and_hms_opt(0, 0, 0)?.and_utc();
    u64::try_from(next.timestamp_millis()).ok()
}

fn valid_usage_date(value: &str) -> bool {
    chrono::NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok()
}

fn valid_subject_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 160
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':'))
}

fn valid_opaque_id(value: &str) -> bool {
    value.len() == 32
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
}

fn random_opaque_id() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn proportional(value: u64, numerator: u64, denominator: u64) -> u64 {
    if value == 0 || numerator == 0 || denominator == 0 {
        return 0;
    }
    (u128::from(value) * u128::from(numerator) / u128::from(denominator)).min(u128::from(u64::MAX))
        as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(
        ledger: &mut LightUsageLedger,
        subject: &str,
        kind: LightUsageSubjectKind,
        on: bool,
        source: LightUsageObservationSource,
        instant: Instant,
        epoch_ms: u64,
    ) {
        ledger.record_observation(
            subject,
            kind,
            on,
            source,
            instant,
            epoch_ms,
            Duration::from_secs(60),
        );
    }

    #[test]
    fn same_state_and_live_transition_use_monotonic_duration() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::LiveSubscription,
            start,
            1_700_000_000_000,
        );
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(10),
            1_700_000_010_000,
        );
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            false,
            LightUsageObservationSource::LiveSubscription,
            start + Duration::from_secs(20),
            1_700_000_020_000,
        );

        let segment = ledger.segments.values().next().unwrap();
        assert_eq!(segment.covered_ms, 20_000);
        assert_eq!(segment.on_ms, 20_000);
        assert_eq!(segment.transition_uncertainty_ms, 0);
        assert_eq!(segment.observation_count, 3);
        assert_eq!(segment.transition_count, 1);
    }

    #[test]
    fn poll_transition_uses_midpoint_and_records_uncertainty() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "room-1",
            LightUsageSubjectKind::RoomAggregate,
            false,
            LightUsageObservationSource::SyncPoll,
            start,
            1_700_000_000_000,
        );
        record(
            &mut ledger,
            "room-1",
            LightUsageSubjectKind::RoomAggregate,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(20),
            1_700_000_020_000,
        );
        let segment = ledger.segments.values().next().unwrap();
        assert_eq!(segment.covered_ms, 20_000);
        assert_eq!(segment.on_ms, 10_000);
        assert_eq!(segment.transition_uncertainty_ms, 10_000);
        assert_eq!(segment.subject_kind, LightUsageSubjectKind::RoomAggregate);
    }

    #[test]
    fn stale_gap_and_restart_start_new_segments_without_extrapolation() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start,
            1_700_000_000_000,
        );
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(120),
            1_700_000_120_000,
        );
        assert_eq!(ledger.segments.len(), 2);
        assert_eq!(
            ledger.segments.values().map(|s| s.covered_ms).sum::<u64>(),
            0
        );

        let persisted = ledger.persisted_snapshot(1_700_000_120_000);
        let mut restarted = persisted.normalized(1_700_000_120_000).unwrap();
        record(
            &mut restarted,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(121),
            1_700_000_121_000,
        );
        assert_eq!(restarted.segments.len(), 3);
        assert_eq!(
            restarted
                .segments
                .values()
                .map(|s| s.covered_ms)
                .sum::<u64>(),
            0
        );
    }

    #[test]
    fn backwards_wall_clock_jump_starts_a_new_baseline() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start,
            1_700_000_000_000,
        );
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(10),
            1_699_999_990_000,
        );

        assert_eq!(ledger.clock_discontinuity_count, 1);
        assert_eq!(ledger.segments.len(), 2);
        assert!(ledger
            .segments
            .values()
            .all(|segment| segment.covered_ms == 0));
    }

    #[test]
    fn utc_rollover_splits_one_monotonic_interval() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        let before_midnight = 1_704_153_595_000; // 2023-12-31T23:59:55Z
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start,
            before_midnight,
        );
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(10),
            before_midnight + 10_000,
        );
        assert_eq!(ledger.segments.len(), 2);
        assert_eq!(
            ledger.segments.values().map(|s| s.covered_ms).sum::<u64>(),
            10_000
        );
        assert_eq!(
            ledger.segments.values().map(|s| s.on_ms).sum::<u64>(),
            10_000
        );
        assert!(ledger.force_checkpoint);
        assert!(ledger.force_cloud);
    }

    #[test]
    fn acknowledgement_is_exact_batch_and_revision_scoped() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start,
            1_700_000_000_000,
        );
        ledger.force_cloud_upload(start);
        let batch = ledger.begin_cloud_upload(start, 1_700_000_000_000).unwrap();
        ledger.complete_cloud_upload(&batch, false, start, 1_700_000_000_001);
        assert!(ledger.segments.values().all(LightUsageSegment::is_dirty));
        assert_eq!(
            ledger.cloud_status.as_deref(),
            Some("schema_unacknowledged")
        );

        ledger.force_cloud_upload(start);
        let retry = ledger.begin_cloud_upload(start, 1_700_000_000_002).unwrap();
        assert_eq!(retry.batch_id, batch.batch_id);
        ledger.complete_cloud_upload(&retry, true, start, 1_700_000_000_003);
        assert!(ledger.segments.values().all(|segment| !segment.is_dirty()));
        assert_eq!(ledger.cloud_status.as_deref(), Some("ok"));
    }

    #[test]
    fn subject_scope_change_starts_a_new_segment_without_invented_coverage() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "shared-token",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::LiveSubscription,
            start,
            1_700_000_000_000,
        );
        record(
            &mut ledger,
            "shared-token",
            LightUsageSubjectKind::RoomAggregate,
            true,
            LightUsageObservationSource::Periodic,
            start + Duration::from_secs(10),
            1_700_000_010_000,
        );

        let diagnostics = ledger.diagnostics(1_700_000_010_000);
        assert_eq!(ledger.segments.len(), 2);
        assert!(ledger
            .segments
            .values()
            .all(|segment| segment.covered_ms == 0));
        assert_eq!(diagnostics.bulb_subject_count, 1);
        assert_eq!(diagnostics.room_aggregate_subject_count, 1);
        assert_eq!(diagnostics.open_subject_count, 1);
    }

    #[test]
    fn checkpoint_and_cloud_batches_stay_coalesced_and_bounded() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        for index in 0..=LIGHT_USAGE_CLOUD_BATCH_LIMIT {
            record(
                &mut ledger,
                &format!("bulb-{index}"),
                LightUsageSubjectKind::Bulb,
                true,
                LightUsageObservationSource::Periodic,
                start,
                1_700_000_000_000,
            );
        }

        assert!(ledger
            .begin_checkpoint(
                start + LIGHT_USAGE_CHECKPOINT_INTERVAL - Duration::from_millis(1),
                1_700_000_899_999,
            )
            .is_none());
        let (generation, _) = ledger
            .begin_checkpoint(start + LIGHT_USAGE_CHECKPOINT_INTERVAL, 1_700_000_900_000)
            .unwrap();
        ledger.complete_checkpoint(
            generation,
            true,
            start + LIGHT_USAGE_CHECKPOINT_INTERVAL,
            1_700_000_900_000,
        );

        ledger.force_cloud_upload(start + LIGHT_USAGE_CHECKPOINT_INTERVAL);
        let batch = ledger
            .begin_cloud_upload(start + LIGHT_USAGE_CHECKPOINT_INTERVAL, 1_700_000_900_000)
            .unwrap();
        assert_eq!(batch.segments.len(), LIGHT_USAGE_CLOUD_BATCH_LIMIT);
        assert!(batch.backlog_capped);
        ledger.complete_cloud_upload(
            &batch,
            true,
            start + LIGHT_USAGE_CHECKPOINT_INTERVAL,
            1_700_000_900_001,
        );
        assert_eq!(ledger.cloud_status.as_deref(), Some("backlog_capped"));
        assert_eq!(
            ledger
                .segments
                .values()
                .filter(|segment| segment.is_dirty())
                .count(),
            1
        );
    }

    #[test]
    fn normalization_clamps_counters_and_discards_runtime_baselines() {
        let mut ledger = LightUsageLedger::default();
        let start = Instant::now();
        record(
            &mut ledger,
            "bulb-1",
            LightUsageSubjectKind::Bulb,
            true,
            LightUsageObservationSource::Periodic,
            start,
            1_700_000_000_000,
        );
        let segment = ledger.segments.values_mut().next().unwrap();
        segment.covered_ms = 5;
        segment.on_ms = 9;
        segment.transition_uncertainty_ms = 8;
        segment.synced_revision = segment.revision + 10;
        let normalized = ledger.normalized(1_700_000_000_000).unwrap();
        let segment = normalized.segments.values().next().unwrap();
        assert_eq!(segment.on_ms, 5);
        assert_eq!(segment.transition_uncertainty_ms, 5);
        assert_eq!(segment.synced_revision, segment.revision);
        assert!(normalized.baselines.is_empty());
    }
}
