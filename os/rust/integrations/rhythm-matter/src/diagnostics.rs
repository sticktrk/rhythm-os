//! Bounded, passive evidence for private support bundles. Never performs I/O
//! against a light, and never changes dispatch or recovery decisions.

use std::collections::{BTreeMap, VecDeque};
use std::sync::Mutex;
use std::time::Instant;

use serde::Serialize;

use crate::transport::{
    MatterAttributeReport, MatterAttributeValue, MatterCommandOutcome, MatterCommandOutcomeStatus,
    MatterCommandSubmission, MatterEndpointCommandPlan, MatterSubscriptionFailureClass,
};

const COMMAND_LIMIT: usize = 256;
const FAILURE_LIMIT: usize = 64;
const EVENT_LIMIT: usize = 256;
const ENDPOINT_LIMIT: usize = 256;
const PLAN_STEP_LIMIT: usize = 16;

fn now_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

#[derive(Clone, Debug, Serialize)]
pub struct MatterCommandDiagnostic {
    pub command_id: u64,
    pub node_id: u64,
    pub endpoint: u16,
    pub plan: Option<MatterEndpointCommandPlan>,
    pub plan_steps_truncated: bool,
    pub submission_started_at_unix_ms: Option<u64>,
    pub accepted_at_unix_ms: Option<u64>,
    pub submission_error: bool,
    pub completed_inline: bool,
    pub controller_stream_id: Option<String>,
    pub outcome: Option<MatterCommandOutcome>,
    pub outcome_detail_present: bool,
    pub outcome_received_at_unix_ms: Option<u64>,
    /// Local monotonic elapsed time, including RPC admission and event delivery.
    /// This is not a physical confirmation latency.
    pub elapsed_ms: Option<u64>,
    pub outcome_may_be_lost: bool,
    #[serde(skip)]
    started: Option<Instant>,
}

impl MatterCommandDiagnostic {
    fn new(command_id: u64, node_id: u64, endpoint: u16) -> Self {
        Self {
            command_id,
            node_id,
            endpoint,
            plan: None,
            plan_steps_truncated: false,
            submission_started_at_unix_ms: None,
            accepted_at_unix_ms: None,
            submission_error: false,
            completed_inline: false,
            controller_stream_id: None,
            outcome: None,
            outcome_detail_present: false,
            outcome_received_at_unix_ms: None,
            elapsed_ms: None,
            outcome_may_be_lost: false,
            started: None,
        }
    }

    fn pending(&self) -> bool {
        self.outcome.is_none()
            && !self.completed_inline
            && !self.submission_error
            && !self.outcome_may_be_lost
    }

    fn finish_elapsed(&mut self) {
        self.elapsed_ms = self.started.map(|at| at.elapsed().as_millis() as u64);
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(crate) enum MatterDiagnosticEvent {
    StreamUnavailable,
    StreamConnected {
        stream_id: String,
    },
    StreamReset {
        stream_id: String,
        history_gap: bool,
    },
    SubscriptionAttempt {
        node_id: u64,
        endpoint: u16,
    },
    SubscriptionReady {
        node_id: u64,
        endpoint: u16,
    },
    SubscriptionRetry {
        node_id: u64,
        endpoint: u16,
        failure_class: MatterSubscriptionFailureClass,
        attempts: u32,
        retry_in_ms: u64,
    },
    SubscriptionTerminated {
        node_id: u64,
        endpoint: u16,
        failure_class: MatterSubscriptionFailureClass,
        chip_error: u32,
    },
}

#[derive(Clone, Debug, Serialize)]
struct TimedEvent {
    recorded_at_unix_ms: u64,
    #[serde(flatten)]
    event: MatterDiagnosticEvent,
}

#[derive(Clone, Debug, Serialize)]
struct EndpointObservation {
    node_id: u64,
    endpoint: u16,
    last_activity_received_at_unix_ms: u64,
    last_activity_age_ms: u64,
    last_activity_native_age_ms: Option<u64>,
    last_on_off: Option<bool>,
    last_on_off_received_at_unix_ms: Option<u64>,
    last_on_off_age_ms: Option<u64>,
    last_on_off_native_age_ms: Option<u64>,
    /// False after termination or a controller stream gap/reset. Age remains
    /// separate: liveness-only reports never refresh a physical attribute.
    on_off_valid_in_current_stream: bool,
    #[serde(skip)]
    activity_at: Instant,
    #[serde(skip)]
    on_off_at: Option<Instant>,
}

#[derive(Default)]
struct DiagnosticState {
    commands: BTreeMap<u64, MatterCommandDiagnostic>,
    failures: VecDeque<MatterCommandDiagnostic>,
    events: VecDeque<TimedEvent>,
    endpoints: BTreeMap<(u64, u16), EndpointObservation>,
    commands_evicted: u64,
    failures_evicted: u64,
    events_evicted: u64,
    endpoints_evicted: u64,
    last_reset: Option<(String, bool, Instant)>,
}

#[derive(Serialize)]
pub struct MatterDiagnosticSnapshot {
    schema_version: u32,
    captured_at_unix_ms: u64,
    command_limit: usize,
    failure_limit: usize,
    event_limit: usize,
    endpoint_limit: usize,
    commands_evicted: u64,
    failures_evicted: u64,
    events_evicted: u64,
    endpoints_evicted: u64,
    pending_command_count: usize,
    indeterminate_command_count: usize,
    commands: Vec<MatterCommandDiagnostic>,
    recent_failures: Vec<MatterCommandDiagnostic>,
    events: Vec<TimedEvent>,
    observations: Vec<EndpointObservation>,
}

#[derive(Default)]
pub struct MatterDiagnostics {
    state: Mutex<DiagnosticState>,
}

impl MatterDiagnostics {
    pub(crate) fn submitted(&self, plans: &[MatterEndpointCommandPlan]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for plan in plans {
            let mut record =
                MatterCommandDiagnostic::new(plan.command_id, plan.node_id, plan.endpoint);
            record.started = Some(Instant::now());
            record.submission_started_at_unix_ms = Some(now_ms());
            let mut bounded_plan = plan.clone();
            record.plan_steps_truncated = bounded_plan.steps.len() > PLAN_STEP_LIMIT;
            bounded_plan.steps.truncate(PLAN_STEP_LIMIT);
            record.plan = Some(bounded_plan);
            state.commands.insert(plan.command_id, record);
        }
        Self::trim_commands(&mut state);
    }

    pub(crate) fn accepted(&self, submission: &MatterCommandSubmission) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let last_reset = state.last_reset.clone();
        if let Some(record) = state.commands.get_mut(&submission.command_id) {
            record.accepted_at_unix_ms = Some(now_ms());
            if record.outcome.is_none() {
                record.controller_stream_id = submission
                    .controller_stream_id
                    .as_ref()
                    .map(|id| bounded_id(id));
                if let Some((stream, gap, reset_at)) = last_reset {
                    record.outcome_may_be_lost = !submission.completed
                        && (record
                            .controller_stream_id
                            .as_ref()
                            .is_some_and(|owner| owner != &stream)
                            || (gap && record.started.is_some_and(|started| started <= reset_at)));
                    if record.outcome_may_be_lost {
                        record.finish_elapsed();
                    }
                }
            }
            record.completed_inline = submission.completed;
            // An event may arrive before the submit RPC returns. Never replace
            // its terminal result with an admission-only success.
            if submission.completed && record.outcome.is_none() {
                record.finish_elapsed();
            }
        }
    }

    pub(crate) fn submission_failed(&self, ids: &[u64]) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        for id in ids {
            if let Some(record) = state.commands.get_mut(id) {
                record.submission_error = true;
                if record.outcome.is_none() {
                    record.finish_elapsed();
                }
                let failed = record.clone();
                Self::retain_failure(&mut state, failed);
            }
        }
    }

    pub(crate) fn outcome(&self, stream_id: &str, outcome: &MatterCommandOutcome) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let record = state.commands.entry(outcome.command_id).or_insert_with(|| {
            MatterCommandDiagnostic::new(outcome.command_id, outcome.node_id, outcome.endpoint)
        });
        record.controller_stream_id = Some(bounded_id(stream_id));
        record.outcome_received_at_unix_ms = Some(now_ms());
        record.outcome_detail_present = outcome.detail.is_some();
        let mut sanitized = outcome.clone();
        // Free-form native errors can contain addresses or unbounded text.
        // Keep typed status/class and presence; the private raw logs retain detail.
        sanitized.detail = None;
        record.outcome = Some(sanitized);
        record.outcome_may_be_lost = false;
        record.finish_elapsed();
        if outcome.status == MatterCommandOutcomeStatus::Failed {
            let failed = record.clone();
            Self::retain_failure(&mut state, failed);
        }
        Self::trim_commands(&mut state);
    }

    pub(crate) fn event(&self, mut event: MatterDiagnosticEvent) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        match &mut event {
            MatterDiagnosticEvent::StreamConnected { stream_id } => {
                *stream_id = bounded_id(stream_id)
            }
            MatterDiagnosticEvent::StreamReset {
                stream_id,
                history_gap,
            } => {
                *stream_id = bounded_id(stream_id);
                state.last_reset = Some((stream_id.clone(), *history_gap, Instant::now()));
                for record in state
                    .commands
                    .values_mut()
                    .filter(|record| record.pending())
                {
                    if record
                        .controller_stream_id
                        .as_ref()
                        .is_some_and(|owner| *history_gap || owner != stream_id)
                    {
                        record.outcome_may_be_lost = true;
                        record.finish_elapsed();
                    }
                }
                for observation in state.endpoints.values_mut() {
                    observation.on_off_valid_in_current_stream = false;
                }
            }
            MatterDiagnosticEvent::SubscriptionTerminated {
                node_id, endpoint, ..
            } => {
                if let Some(observation) = state.endpoints.get_mut(&(*node_id, *endpoint)) {
                    observation.on_off_valid_in_current_stream = false;
                }
            }
            _ => {}
        }
        state.events.push_back(TimedEvent {
            recorded_at_unix_ms: now_ms(),
            event,
        });
        while state.events.len() > EVENT_LIMIT {
            state.events.pop_front();
            state.events_evicted += 1;
        }
    }

    pub(crate) fn attribute_report(&self, report: &MatterAttributeReport) {
        let Ok(mut state) = self.state.lock() else {
            return;
        };
        let observation = state
            .endpoints
            .entry((report.node_id, report.endpoint))
            .or_insert_with(|| EndpointObservation {
                node_id: report.node_id,
                endpoint: report.endpoint,
                last_activity_received_at_unix_ms: report.received_at_unix_ms,
                last_activity_age_ms: 0,
                last_activity_native_age_ms: None,
                last_on_off: None,
                last_on_off_received_at_unix_ms: None,
                last_on_off_age_ms: None,
                last_on_off_native_age_ms: None,
                on_off_valid_in_current_stream: false,
                activity_at: Instant::now(),
                on_off_at: None,
            });
        observation.last_activity_received_at_unix_ms = report.received_at_unix_ms;
        observation.activity_at = Instant::now();
        if report.cluster == 6 && report.attr_id == 0 {
            if let MatterAttributeValue::Bool(on) = report.value {
                observation.last_on_off = Some(on);
                observation.last_on_off_received_at_unix_ms = Some(report.received_at_unix_ms);
                observation.on_off_at = Some(Instant::now());
                observation.on_off_valid_in_current_stream = true;
            }
        }
        while state.endpoints.len() > ENDPOINT_LIMIT {
            let oldest = state
                .endpoints
                .iter()
                .min_by_key(|(_, value)| value.activity_at)
                .map(|(key, _)| *key)
                .unwrap();
            state.endpoints.remove(&oldest);
            state.endpoints_evicted += 1;
        }
    }

    /// A busy recorder is explicitly unavailable, never an empty healthy
    /// snapshot. Clone under a short lock; serialization happens after release.
    pub fn snapshot(&self) -> Option<MatterDiagnosticSnapshot> {
        let state = self.state.try_lock().ok()?;
        let captured_at_unix_ms = now_ms();
        let mut commands: Vec<_> = state.commands.values().cloned().collect();
        for record in &mut commands {
            if record.pending() {
                record.finish_elapsed();
            }
        }
        let mut observations: Vec<_> = state.endpoints.values().cloned().collect();
        for observation in &mut observations {
            observation.last_activity_age_ms = observation.activity_at.elapsed().as_millis() as u64;
            observation.last_activity_native_age_ms = native_age_ms(
                captured_at_unix_ms,
                observation.last_activity_received_at_unix_ms,
            );
            observation.last_on_off_native_age_ms = observation
                .last_on_off_received_at_unix_ms
                .and_then(|received| native_age_ms(captured_at_unix_ms, received));
            observation.last_on_off_age_ms = observation
                .on_off_at
                .map(|at| at.elapsed().as_millis() as u64);
        }
        Some(MatterDiagnosticSnapshot {
            schema_version: 1,
            captured_at_unix_ms,
            command_limit: COMMAND_LIMIT,
            failure_limit: FAILURE_LIMIT,
            event_limit: EVENT_LIMIT,
            endpoint_limit: ENDPOINT_LIMIT,
            commands_evicted: state.commands_evicted,
            failures_evicted: state.failures_evicted,
            events_evicted: state.events_evicted,
            endpoints_evicted: state.endpoints_evicted,
            pending_command_count: commands.iter().filter(|record| record.pending()).count(),
            indeterminate_command_count: commands
                .iter()
                .filter(|record| record.outcome_may_be_lost)
                .count(),
            commands,
            recent_failures: state.failures.iter().cloned().collect(),
            events: state.events.iter().cloned().collect(),
            observations,
        })
    }

    fn trim_commands(state: &mut DiagnosticState) {
        while state.commands.len() > COMMAND_LIMIT {
            state.commands.pop_first();
            state.commands_evicted += 1;
        }
    }

    fn retain_failure(state: &mut DiagnosticState, failed: MatterCommandDiagnostic) {
        // A submission error followed by a late terminal outcome, or a replayed
        // outcome after a stream resume, describes the same command.
        if let Some(existing) = state
            .failures
            .iter_mut()
            .find(|existing| existing.command_id == failed.command_id)
        {
            *existing = failed;
            return;
        }
        state.failures.push_back(failed);
        while state.failures.len() > FAILURE_LIMIT {
            state.failures.pop_front();
            state.failures_evicted += 1;
        }
    }
}

fn bounded_id(id: &str) -> String {
    id.chars().take(128).collect()
}

// Older sidecars omit native timestamps. Future times indicate clock skew;
// neither case is evidence of a freshly observed state.
fn native_age_ms(now: u64, received: u64) -> Option<u64> {
    (received != 0).then(|| now.checked_sub(received)).flatten()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::MatterCommandStep;

    fn plan(id: u64) -> MatterEndpointCommandPlan {
        MatterEndpointCommandPlan {
            command_id: id,
            node_id: 112,
            endpoint: 1,
            steps: vec![MatterCommandStep::SetOnOff { on: true }],
            inter_step_delay_ms: Some(100),
        }
    }

    fn accepted(id: u64) -> MatterCommandSubmission {
        MatterCommandSubmission {
            command_id: id,
            completed: false,
            controller_stream_id: Some("stream-a".into()),
        }
    }

    fn outcome(id: u64, status: MatterCommandOutcomeStatus) -> MatterCommandOutcome {
        MatterCommandOutcome {
            command_id: id,
            node_id: 112,
            endpoint: 1,
            status,
            // Older sidecars omit time and failure class: preserve unknown.
            completed_at_unix_ms: None,
            detail: Some("private native error text".into()),
            failure_class: None,
        }
    }

    #[test]
    fn admission_and_acknowledgement_never_invent_physical_confirmation() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(1)]);
        recorder.accepted(&accepted(1));
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.pending_command_count, 1);
        assert!(snapshot.commands[0].outcome.is_none());
        assert!(snapshot.observations.is_empty());
        recorder.outcome(
            "stream-a",
            &outcome(1, MatterCommandOutcomeStatus::Succeeded),
        );
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.pending_command_count, 0);
        assert!(snapshot.commands[0].elapsed_ms.is_some());
        assert!(snapshot.observations.is_empty());
    }

    #[test]
    fn terminal_event_before_admission_response_stays_terminal_and_redacts_detail() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(2)]);
        recorder.outcome("stream-a", &outcome(2, MatterCommandOutcomeStatus::Failed));
        recorder.accepted(&accepted(2));
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.pending_command_count, 0);
        assert_eq!(
            snapshot.commands[0].outcome.as_ref().unwrap().status,
            MatterCommandOutcomeStatus::Failed
        );
        assert!(snapshot.commands[0]
            .outcome
            .as_ref()
            .unwrap()
            .failure_class
            .is_none());
        assert!(snapshot.commands[0].outcome_detail_present);
        assert!(snapshot.commands[0].accepted_at_unix_ms.is_some());
        assert!(!serde_json::to_string(&snapshot)
            .unwrap()
            .contains("private native error text"));
    }

    #[test]
    fn liveness_and_subscription_recovery_do_not_refresh_an_old_on_off_value() {
        let recorder = MatterDiagnostics::default();
        let mut report = MatterAttributeReport {
            node_id: 112,
            endpoint: 1,
            received_at_unix_ms: 100,
            cluster: 6,
            attr_id: 0,
            value: MatterAttributeValue::Bool(true),
        };
        recorder.attribute_report(&report);
        recorder.event(MatterDiagnosticEvent::SubscriptionTerminated {
            node_id: 112,
            endpoint: 1,
            failure_class: MatterSubscriptionFailureClass::Timeout,
            chip_error: 50,
        });
        report.received_at_unix_ms = 200;
        report.value = MatterAttributeValue::SubscriptionAlive;
        recorder.attribute_report(&report);
        recorder.event(MatterDiagnosticEvent::SubscriptionReady {
            node_id: 112,
            endpoint: 1,
        });
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(
            snapshot.observations[0].last_activity_received_at_unix_ms,
            200
        );
        assert_eq!(
            snapshot.observations[0].last_on_off_received_at_unix_ms,
            Some(100)
        );
        assert_eq!(snapshot.observations[0].last_on_off, Some(true));
        assert!(!snapshot.observations[0].on_off_valid_in_current_stream);
        report.received_at_unix_ms = 300;
        report.value = MatterAttributeValue::Bool(false);
        recorder.attribute_report(&report);
        assert!(recorder.snapshot().unwrap().observations[0].on_off_valid_in_current_stream);
        assert_eq!(
            recorder.snapshot().unwrap().observations[0].last_on_off,
            Some(false)
        );
    }

    #[test]
    fn stream_gap_marks_outstanding_results_unknown_without_claiming_failure() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(3)]);
        recorder.accepted(&accepted(3));
        recorder.event(MatterDiagnosticEvent::StreamReset {
            stream_id: "stream-a".into(),
            history_gap: true,
        });
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.pending_command_count, 0);
        assert!(snapshot.commands[0].outcome_may_be_lost);
        assert!(snapshot.commands[0].outcome.is_none());
        assert!(snapshot.commands[0].elapsed_ms.is_some());
        assert!(snapshot.recent_failures.is_empty());
    }

    #[test]
    fn late_admission_after_restart_preserves_unknown_old_work_and_accepts_new_work() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(1)]);
        recorder.event(MatterDiagnosticEvent::StreamReset {
            stream_id: "stream-b".into(),
            history_gap: false,
        });
        recorder.accepted(&accepted(1));
        recorder.submitted(&[plan(2)]);
        let mut new_admission = accepted(2);
        new_admission.controller_stream_id = Some("stream-b".into());
        recorder.accepted(&new_admission);
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.indeterminate_command_count, 1);
        assert_eq!(snapshot.pending_command_count, 1);
        assert!(snapshot.commands[0].outcome_may_be_lost);
        assert!(snapshot.commands[0].elapsed_ms.is_some());
        assert!(!snapshot.commands[1].outcome_may_be_lost);
    }

    #[test]
    fn submission_error_and_late_failed_outcome_share_one_failure_entry() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(1)]);
        recorder.submission_failed(&[1]);
        recorder.outcome("stream-a", &outcome(1, MatterCommandOutcomeStatus::Failed));
        // A replayed outcome after a stream resume describes the same command.
        recorder.outcome("stream-a", &outcome(1, MatterCommandOutcomeStatus::Failed));
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.recent_failures.len(), 1);
        assert_eq!(snapshot.failures_evicted, 0);
        let failure = &snapshot.recent_failures[0];
        assert_eq!(failure.command_id, 1);
        assert!(failure.submission_error);
        assert_eq!(
            failure.outcome.as_ref().unwrap().status,
            MatterCommandOutcomeStatus::Failed
        );
    }

    #[test]
    fn stream_reset_invalidates_every_on_off_value_but_keeps_its_age() {
        let recorder = MatterDiagnostics::default();
        for endpoint in [1u16, 2u16] {
            recorder.attribute_report(&MatterAttributeReport {
                node_id: 112,
                endpoint,
                received_at_unix_ms: 100,
                cluster: 6,
                attr_id: 0,
                value: MatterAttributeValue::Bool(true),
            });
        }
        assert!(recorder
            .snapshot()
            .unwrap()
            .observations
            .iter()
            .all(|observation| observation.on_off_valid_in_current_stream));
        recorder.event(MatterDiagnosticEvent::StreamReset {
            stream_id: "stream-b".into(),
            history_gap: false,
        });
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.observations.len(), 2);
        for observation in &snapshot.observations {
            assert!(!observation.on_off_valid_in_current_stream);
            assert_eq!(observation.last_on_off, Some(true));
            assert_eq!(observation.last_on_off_received_at_unix_ms, Some(100));
            assert!(observation.last_on_off_age_ms.is_some());
        }
    }

    #[test]
    fn missing_native_timestamp_remains_unknown_instead_of_fresh() {
        let recorder = MatterDiagnostics::default();
        recorder.attribute_report(&MatterAttributeReport {
            node_id: 112,
            endpoint: 1,
            received_at_unix_ms: 0,
            cluster: 6,
            attr_id: 0,
            value: MatterAttributeValue::Bool(true),
        });
        let snapshot = recorder.snapshot().unwrap();
        assert!(snapshot.observations[0].last_on_off_age_ms.is_some());
        assert!(snapshot.observations[0].last_on_off_native_age_ms.is_none());
        assert!(snapshot.observations[0]
            .last_activity_native_age_ms
            .is_none());
    }

    #[test]
    fn failure_evidence_survives_healthy_traffic_and_all_histories_are_bounded() {
        let recorder = MatterDiagnostics::default();
        recorder.submitted(&[plan(1)]);
        recorder.submission_failed(&[1]);
        for id in 2..=1_000 {
            recorder.submitted(&[plan(id)]);
            recorder.outcome(
                "stream-a",
                &outcome(id, MatterCommandOutcomeStatus::Succeeded),
            );
            recorder.event(MatterDiagnosticEvent::SubscriptionReady {
                node_id: id,
                endpoint: 1,
            });
            recorder.attribute_report(&MatterAttributeReport {
                node_id: id,
                endpoint: 1,
                received_at_unix_ms: id,
                cluster: 0,
                attr_id: 0,
                value: MatterAttributeValue::SubscriptionAlive,
            });
        }
        let snapshot = recorder.snapshot().unwrap();
        assert_eq!(snapshot.commands.len(), COMMAND_LIMIT);
        assert_eq!(snapshot.events.len(), EVENT_LIMIT);
        assert_eq!(snapshot.observations.len(), ENDPOINT_LIMIT);
        assert_eq!(snapshot.commands_evicted, 1_000 - COMMAND_LIMIT as u64);
        assert!(snapshot.events_evicted > 0 && snapshot.endpoints_evicted > 0);
        assert_eq!(snapshot.recent_failures.len(), 1);
        assert_eq!(snapshot.recent_failures[0].command_id, 1);
        for id in 1_001..1_100 {
            recorder.outcome("stream-a", &outcome(id, MatterCommandOutcomeStatus::Failed));
        }
        assert_eq!(
            recorder.snapshot().unwrap().recent_failures.len(),
            FAILURE_LIMIT
        );
        assert!(recorder.snapshot().unwrap().failures_evicted > 0);
    }

    #[test]
    fn busy_recorder_is_unavailable_and_plan_payload_is_capped() {
        let recorder = MatterDiagnostics::default();
        let guard = recorder.state.lock().unwrap();
        assert!(recorder.snapshot().is_none());
        drop(guard);
        let mut request = plan(1);
        request.steps = vec![MatterCommandStep::SetOnOff { on: true }; 100];
        recorder.submitted(&[request]);
        let snapshot = recorder.snapshot().unwrap();
        assert!(snapshot.commands[0].plan_steps_truncated);
        assert_eq!(
            snapshot.commands[0].plan.as_ref().unwrap().steps.len(),
            PLAN_STEP_LIMIT
        );
    }
}
