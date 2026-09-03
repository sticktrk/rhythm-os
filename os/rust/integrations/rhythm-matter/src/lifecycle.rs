//! Matter hub lifecycle — connect, disconnect, runtime creation.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use log::{info, warn};
use rhythm_devices::{DeviceQuirk, LightCapabilities};
use rhythm_os::api_types::{LightCapabilitiesDto, LightColorTemperatureCapabilitiesDto};
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::hub::{ActiveHub, HubEvent, HubType};
use rhythm_os::registry::HubDeviceRegistry;
use rhythm_os::state::SharedState;

use crate::hub_state::MatterHubData;
use crate::transport::{
    CommissionedDevice, MatterAttributeReport, MatterAttributeValue, MatterControllerEvent,
    MatterControllerEventCursor, MatterDeviceInfo, MatterEndpointCommandPlan,
    MatterSubscriptionFailureClass, MatterSubscriptionTarget, MatterTransport,
    DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS, DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
};

const MATTER_EVENT_LONG_POLL: Duration = Duration::from_secs(1);
const MATTER_EVENT_RETRY_INITIAL: Duration = Duration::from_millis(250);
const MATTER_EVENT_RETRY_MAX: Duration = Duration::from_secs(5);
const MATTER_EVENT_SHUTDOWN_POLL: Duration = Duration::from_millis(50);
const MATTER_SUBSCRIPTION_RETRY_INITIAL: Duration = Duration::from_secs(30);
const MATTER_SUBSCRIPTION_RETRY_MAX: Duration = Duration::from_secs(30 * 60);
/// How long a successful subscription is trusted before it is re-verified.
///
/// A dead subscription now announces itself: chipd reports
/// `SubscriptionTerminated` the moment an established subscription ends, so
/// this recheck is only a backstop for a termination that was never delivered
/// (a lost event, a sidecar that died mid-report). Ten minutes keeps that
/// backstop cheap — the old 60s value re-issued a SubscribeOnOff per endpoint
/// per minute for a fleet that was already healthy.
const MATTER_SUBSCRIPTION_RECHECK_INTERVAL: Duration = Duration::from_secs(10 * 60);
/// How long the worker's cached ListDevices result is trusted. Short, because
/// it is the only thing that notices a device commissioned by another path.
const MATTER_SUBSCRIPTION_TARGET_REFRESH_INTERVAL: Duration = Duration::from_secs(60);
/// Cap on an idle worker's sleep, so a newly commissioned device is observed
/// within a minute instead of after the next retry (up to 30 min away).
const MATTER_SUBSCRIPTION_MAX_IDLE_WAIT: Duration = Duration::from_secs(60);
/// Floor on the worker's sleep. Any state that would otherwise say "run again
/// immediately" must still yield, so a persistently failing pass cannot become
/// a busy loop.
const MATTER_SUBSCRIPTION_MIN_WAIT: Duration = Duration::from_millis(250);
/// Debounce for the "an unknown endpoint appeared" cache invalidation, so a
/// burst of proofs for endpoints missing from a stale list cannot issue one
/// ListDevices RPC each.
const MATTER_SUBSCRIPTION_UNKNOWN_KEY_REFRESH_DEBOUNCE: Duration = Duration::from_secs(10);

/// Endpoint -> (last observed on/off value, when it was observed).
type OnOffObservations = HashMap<(u64, u16), (bool, std::time::Instant)>;
const MATTER_ATTRIBUTE_REPORT_HISTORY_LIMIT: usize = 2_048;

fn now_unix_ms() -> u64 {
    chrono::Utc::now().timestamp_millis().max(0) as u64
}

fn record_attribute_report_history(
    report: &MatterAttributeReport,
    history: &Mutex<VecDeque<MatterAttributeReport>>,
) {
    if let Ok(mut reports) = history.lock() {
        reports.push_back(report.clone());
        while reports.len() > MATTER_ATTRIBUTE_REPORT_HISTORY_LIMIT {
            reports.pop_front();
        }
    }
}

fn next_controller_event_retry_delay(current: Duration) -> Duration {
    current.saturating_mul(2).min(MATTER_EVENT_RETRY_MAX)
}

fn wait_for_controller_event_retry(
    shutdown: &std::sync::atomic::AtomicBool,
    delay: Duration,
) -> bool {
    let deadline = std::time::Instant::now() + delay;
    while !shutdown.load(Ordering::Relaxed) {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            return false;
        }
        std::thread::sleep(remaining.min(MATTER_EVENT_SHUTDOWN_POLL));
    }
    true
}

/// List the endpoints that should carry an observed-state subscription.
///
/// Returns `Err` only when the fabric is genuinely unknown — both the
/// persisted-record listing and the live fallback failed. A commissioned-but-
/// empty fabric is `Ok(vec![])`. The distinction matters: the caller prunes
/// per-endpoint success and backoff state against this list, and treating a
/// transient RPC failure as "no devices exist" wiped every cooldown and caused
/// a re-subscribe stampede on the next pass.
fn subscription_targets(transport: &dyn MatterTransport) -> Result<Vec<MatterSubscriptionTarget>> {
    match transport.list_commissioned_devices() {
        Ok(devices) if !devices.is_empty() => Ok(devices
            .into_iter()
            .map(|device| MatterSubscriptionTarget {
                node_id: device.node_id,
                endpoint: device.light_endpoint,
            })
            .collect()),
        Ok(_) => Ok(transport
            .list_devices()
            .unwrap_or_default()
            .into_iter()
            .map(|device| MatterSubscriptionTarget {
                node_id: device.node_id,
                endpoint: 1,
            })
            .collect()),
        Err(persisted_error) => Ok(transport
            .list_devices()
            .map_err(|live_error| {
                persisted_error.context(format!("Matter device listing fallback: {live_error:#}"))
            })?
            .into_iter()
            .map(|device| MatterSubscriptionTarget {
                node_id: device.node_id,
                endpoint: 1,
            })
            .collect()),
    }
}

#[derive(Clone, Copy)]
struct MatterSubscriptionRetry {
    failures: u32,
    next_attempt: std::time::Instant,
}

enum MatterSubscriptionRefresh {
    ControllerReset,
    EndpointProof {
        target: MatterSubscriptionTarget,
        subscription_active: bool,
    },
    /// The native controller reported that an established subscription ended.
    ///
    /// Native auto-resubscribe is disabled (`MATTER RETRY OWNER: RUST`), so
    /// this is the only signal that observation stopped — without it a dead
    /// subscription was only noticed at the next 60s healthcheck.
    SubscriptionTerminated {
        target: MatterSubscriptionTarget,
        failure_class: MatterSubscriptionFailureClass,
    },
}

fn subscription_retry_delay(
    target: &MatterSubscriptionTarget,
    failures: u32,
    initial: Duration,
    maximum: Duration,
) -> Duration {
    let exponent = failures.saturating_sub(1).min(16);
    let base = initial.saturating_mul(1u32 << exponent).min(maximum);
    let base_ms = base.as_millis().try_into().unwrap_or(u64::MAX);
    let jitter_span_ms = base_ms / 4;
    if jitter_span_ms == 0 {
        return base;
    }

    // Stable endpoint-specific jitter prevents a fleet of unavailable nodes
    // from synchronizing discovery/CASE work after startup. Include the
    // failure generation so endpoints also move relative to one another on
    // later attempts without requiring a process-global random generator.
    let hash = target
        .node_id
        .wrapping_mul(0x9e37_79b9_7f4a_7c15)
        .wrapping_add(u64::from(target.endpoint).wrapping_mul(0xbf58_476d_1ce4_e5b9))
        .wrapping_add(u64::from(failures).wrapping_mul(0x94d0_49bb_1331_11eb));
    Duration::from_millis(base_ms.saturating_sub(hash % (jitter_span_ms + 1)))
}

/// Label a failed subscribe RPC for the backoff warning.
///
/// The classifier is shared with the sidecar log summarizer, and its vocabulary
/// is the same `MatterSubscriptionFailureClass` chipd uses for structured
/// terminations, so a class name in `rhythm-matter.log` and one in an `evt`
/// warning always mean the same thing.
fn subscribe_rpc_failure_class(error: &anyhow::Error) -> MatterSubscriptionFailureClass {
    crate::chip_transport::classify_sidecar_failure(&format!("{error:#}"))
        .unwrap_or(MatterSubscriptionFailureClass::Other)
}

/// Cadences that govern the observed-state subscription worker. Split apart so
/// tests can compress them independently and so each one documents a distinct
/// job rather than hiding behind a single "healthcheck" number.
#[derive(Clone, Copy)]
struct MatterSubscriptionCadence {
    /// How long a successful subscription is trusted before re-verification.
    subscription_recheck: Duration,
    /// How long the cached ListDevices result is trusted.
    target_refresh: Duration,
    /// Ceiling on an idle worker's sleep.
    max_idle_wait: Duration,
}

impl MatterSubscriptionCadence {
    fn production() -> Self {
        Self {
            subscription_recheck: MATTER_SUBSCRIPTION_RECHECK_INTERVAL,
            target_refresh: MATTER_SUBSCRIPTION_TARGET_REFRESH_INTERVAL,
            max_idle_wait: MATTER_SUBSCRIPTION_MAX_IDLE_WAIT,
        }
    }
}

/// Mutable state of the observed-state subscription worker.
///
/// Kept as a struct so the message-application rules (which are the whole
/// contract with the controller event stream) can be unit tested without a
/// thread.
struct MatterSubscriptionWorkerState {
    /// Endpoint -> instant at which the subscription should be re-verified.
    subscribed: HashMap<(u64, u16), std::time::Instant>,
    /// Endpoint -> backoff schedule after a failed attempt or a termination.
    retries: HashMap<(u64, u16), MatterSubscriptionRetry>,
    targets: Vec<MatterSubscriptionTarget>,
    known_targets: HashSet<(u64, u16)>,
    targets_refreshed_at: std::time::Instant,
    needs_target_refresh: bool,
    retry_initial: Duration,
    retry_max: Duration,
    cadence: MatterSubscriptionCadence,
}

impl MatterSubscriptionWorkerState {
    fn new(
        retry_initial: Duration,
        retry_max: Duration,
        cadence: MatterSubscriptionCadence,
    ) -> Self {
        Self {
            subscribed: HashMap::new(),
            retries: HashMap::new(),
            targets: Vec::new(),
            known_targets: HashSet::new(),
            targets_refreshed_at: std::time::Instant::now(),
            needs_target_refresh: true,
            retry_initial,
            retry_max,
            cadence,
        }
    }

    /// `subscription_targets` is a ListDevices RPC, so the target list is
    /// cached and only rebuilt when the fabric could actually have changed:
    /// after a controller reset, when a proof or termination names an endpoint
    /// the cache does not know, once per target-refresh interval, or while the
    /// cache is empty (nothing commissioned yet).
    fn should_refresh_targets(&self) -> bool {
        self.needs_target_refresh
            || self.targets.is_empty()
            || self.targets_refreshed_at.elapsed() >= self.cadence.target_refresh
    }

    /// Rebuild the target cache and prune per-endpoint state for endpoints that
    /// are gone.
    ///
    /// A failed listing leaves everything untouched: an empty list from a
    /// transient RPC error would drop every cooldown and every recorded
    /// success, turning a sidecar blip into a fleet-wide re-subscribe
    /// stampede. The refresh timestamp is only advanced on success, so the
    /// next pass retries the listing.
    fn refresh_targets(&mut self, transport: &dyn MatterTransport) {
        let targets = match subscription_targets(transport) {
            Ok(targets) => targets,
            Err(error) => {
                log::debug!(
                    target: "evt",
                    "Matter observed-state target listing unavailable, keeping cached targets: {error:#}"
                );
                return;
            }
        };
        self.targets = targets;
        self.known_targets = self
            .targets
            .iter()
            .map(|target| (target.node_id, target.endpoint))
            .collect();
        self.targets_refreshed_at = std::time::Instant::now();
        self.needs_target_refresh = false;
        let known = &self.known_targets;
        self.subscribed.retain(|key, _| known.contains(key));
        self.retries.retain(|key, _| known.contains(key));
    }

    /// An endpoint the cache has never seen means the list is stale. Debounced:
    /// a burst of proofs for endpoints missing from a list that was just
    /// rebuilt must not cost one ListDevices RPC each.
    fn note_key(&mut self, key: (u64, u16)) {
        if self.known_targets.contains(&key) {
            return;
        }
        if self.targets_refreshed_at.elapsed() < MATTER_SUBSCRIPTION_UNKNOWN_KEY_REFRESH_DEBOUNCE {
            return;
        }
        self.needs_target_refresh = true;
    }

    /// Schedule the next attempt for `key` one backoff step out.
    fn schedule_retry(&mut self, target: &MatterSubscriptionTarget) -> (u32, Duration) {
        let key = (target.node_id, target.endpoint);
        let failures = self
            .retries
            .get(&key)
            .map(|retry| retry.failures)
            .unwrap_or(0)
            .saturating_add(1);
        let delay = subscription_retry_delay(target, failures, self.retry_initial, self.retry_max);
        self.retries.insert(
            key,
            MatterSubscriptionRetry {
                failures,
                next_attempt: std::time::Instant::now() + delay,
            },
        );
        (failures, delay)
    }

    /// Apply one refresh message. Messages are applied strictly in arrival
    /// order, so a `ControllerReset` inside a drained batch clears the state
    /// built by earlier messages while proofs that arrived after it still land.
    fn apply(&mut self, message: MatterSubscriptionRefresh) {
        match message {
            MatterSubscriptionRefresh::ControllerReset => {
                // A changed controller stream means every successful
                // subscription belonged to the old sidecar. Forget success and
                // stale cooldown together so recovery is immediate and rebuilt
                // from authoritative reports.
                self.subscribed.clear();
                self.retries.clear();
                self.needs_target_refresh = true;
            }
            MatterSubscriptionRefresh::EndpointProof {
                target,
                subscription_active,
            } => {
                let key = (target.node_id, target.endpoint);
                self.note_key(key);
                // A command acknowledgement or an attribute report proves the
                // endpoint is reachable, so an obsolete cooldown is dropped.
                // It does NOT prove the subscription died: evicting
                // `subscribed` here cost one SubscribeOnOff RPC per command.
                self.retries.remove(&key);
                if subscription_active {
                    self.subscribed.insert(
                        key,
                        std::time::Instant::now() + self.cadence.subscription_recheck,
                    );
                }
            }
            MatterSubscriptionRefresh::SubscriptionTerminated {
                target,
                failure_class,
            } => {
                // NOTE: a termination carries no subscription generation, so a
                // report for a subscription that has already been replaced (the
                // recheck re-subscribed while the old one was dying) would
                // cancel the healthy replacement and cost one backoff step.
                // The window is small and self-healing, and an epoch guard has
                // to come from chipd — a per-subscription generation id echoed
                // back in MatterSubscriptionTermination — so it is deliberately
                // not faked on this side.
                let key = (target.node_id, target.endpoint);
                self.note_key(key);
                self.subscribed.remove(&key);
                let (failures, delay) = self.schedule_retry(&target);
                if failures.is_power_of_two() {
                    warn!(
                        target: "evt",
                        "Matter observed-state subscription terminated (class={}, attempts={}, retry_in_ms={})",
                        failure_class.as_str(),
                        failures,
                        delay.as_millis()
                    );
                }
            }
        }
    }

    /// Endpoints to attempt this pass, healthiest first.
    ///
    /// After a controller reset every endpoint is due at once; ordering by
    /// recorded failure count keeps the endpoints that have always worked ahead
    /// of the ones that are known to be dead, so a handful of unreachable bulbs
    /// cannot delay the whole fleet behind their address-resolution timeouts.
    fn pass_order(&self) -> Vec<MatterSubscriptionTarget> {
        let mut targets = self.targets.clone();
        targets.sort_by_key(|target| {
            self.retries
                .get(&(target.node_id, target.endpoint))
                .map(|retry| retry.failures)
                .unwrap_or(0)
        });
        targets
    }

    fn run_subscribe_pass(
        &mut self,
        transport: &dyn MatterTransport,
        shutdown: &std::sync::atomic::AtomicBool,
    ) {
        for target in self.pass_order() {
            if shutdown.load(Ordering::Relaxed) {
                break;
            }
            let key = (target.node_id, target.endpoint);
            let now = std::time::Instant::now();
            if self
                .subscribed
                .get(&key)
                .is_some_and(|next_check| *next_check > now)
                || self
                    .retries
                    .get(&key)
                    .is_some_and(|retry| retry.next_attempt > now)
            {
                continue;
            }

            match transport.subscribe_light_state(
                std::slice::from_ref(&target),
                DEFAULT_SUBSCRIPTION_MIN_INTERVAL_SECS,
                DEFAULT_SUBSCRIPTION_MAX_INTERVAL_SECS,
            ) {
                Ok(()) => {
                    if let Some(retry) = self.retries.remove(&key) {
                        info!(target: "evt", "Matter observed-state subscription recovered after {} attempts", retry.failures.saturating_add(1));
                    }
                    self.subscribed.insert(
                        key,
                        std::time::Instant::now() + self.cadence.subscription_recheck,
                    );
                }
                Err(error) => {
                    let failure_class = subscribe_rpc_failure_class(&error);
                    // The endpoint is not observed, and its due-now recheck
                    // entry must go with it: leaving a past-due `subscribed`
                    // deadline behind made next_wait() zero forever while the
                    // key itself stayed skipped by its own backoff.
                    self.subscribed.remove(&key);
                    let (failures, delay) = self.schedule_retry(&target);
                    if failures.is_power_of_two() {
                        warn!(
                            target: "evt",
                            "Matter observed-state subscription unavailable (class={}, attempts={}, retry_in_ms={})",
                            failure_class.as_str(),
                            failures,
                            delay.as_millis()
                        );
                    }
                }
            }
        }
    }

    /// Sleep until the next scheduled retry or recheck, never longer than
    /// `max_idle_wait` (an idle worker used to park for up to an hour, which
    /// left a device commissioned after startup unobserved) and never shorter
    /// than `MATTER_SUBSCRIPTION_MIN_WAIT` (so no state can spin the loop).
    fn next_wait(&self) -> Duration {
        let now = std::time::Instant::now();
        self.retries
            .values()
            .map(|retry| retry.next_attempt.saturating_duration_since(now))
            .chain(
                self.subscribed
                    .values()
                    .map(|next_check| next_check.saturating_duration_since(now)),
            )
            .min()
            .unwrap_or(self.cadence.max_idle_wait)
            .min(self.cadence.max_idle_wait)
            // The floor wins over the cap: not spinning matters more than
            // waking on time, and `clamp` would panic if a test cadence set an
            // idle cap below the floor.
            .max(MATTER_SUBSCRIPTION_MIN_WAIT)
    }
}

fn start_observed_state_subscription_worker(
    transport: Arc<dyn MatterTransport>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
) -> Sender<MatterSubscriptionRefresh> {
    start_observed_state_subscription_worker_with_backoff(
        transport,
        shutdown,
        MATTER_SUBSCRIPTION_RETRY_INITIAL,
        MATTER_SUBSCRIPTION_RETRY_MAX,
        MatterSubscriptionCadence::production(),
    )
}

fn start_observed_state_subscription_worker_with_backoff(
    transport: Arc<dyn MatterTransport>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    retry_initial: Duration,
    retry_max: Duration,
    cadence: MatterSubscriptionCadence,
) -> Sender<MatterSubscriptionRefresh> {
    // Proof, termination and reset signals are correctness events, not advisory
    // wakeups. An unbounded channel avoids silently dropping a controller
    // generation change behind a burst of endpoint reports; the try_recv drain
    // below plus the cached target list keep such a burst from costing one
    // ListDevices RPC (and one subscribe pass) per message.
    let (refresh_tx, refresh_rx) = channel();
    let spawn_result = std::thread::Builder::new()
        .name("matter-observed-subscriptions".to_string())
        .spawn(move || {
            let mut state = MatterSubscriptionWorkerState::new(retry_initial, retry_max, cadence);
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    break;
                }

                if state.should_refresh_targets() {
                    state.refresh_targets(transport.as_ref());
                }
                state.run_subscribe_pass(transport.as_ref(), shutdown.as_ref());

                if shutdown.load(Ordering::Relaxed) {
                    break;
                }
                match refresh_rx.recv_timeout(state.next_wait()) {
                    Ok(message) => {
                        state.apply(message);
                        // Drain the rest of the batch before the next pass so a
                        // burst of proofs produces one subscribe pass, not one
                        // per message.
                        loop {
                            match refresh_rx.try_recv() {
                                Ok(message) => state.apply(message),
                                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
                            }
                        }
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
            }
        });
    if let Err(error) = spawn_result {
        warn!(
            target: "evt",
            "Failed to start Matter observed-state subscription worker: {error}"
        );
    }
    refresh_tx
}

fn cache_on_off_observation(
    report: &crate::transport::MatterAttributeReport,
    observations: &Mutex<OnOffObservations>,
) {
    if report.cluster != crate::clusters::CLUSTER_ON_OFF_U32
        || report.attr_id != crate::clusters::ATTR_ON_OFF_U32
    {
        return;
    }

    let MatterAttributeValue::Bool(lights_on) = &report.value else {
        return;
    };
    if let Ok(mut observations) = observations.lock() {
        observations.insert(
            (report.node_id, report.endpoint),
            (*lights_on, std::time::Instant::now()),
        );
    }
}

/// Drop one endpoint's cached on/off value, leaving every other endpoint's
/// alone. Used when a subscription for that endpoint terminates: readers treat
/// a present value as current, so a stale entry would mask the outage.
fn forget_on_off_observation(key: (u64, u16), observations: &Mutex<OnOffObservations>) {
    if let Ok(mut observations) = observations.lock() {
        observations.remove(&key);
    }
}

fn invalidate_on_off_observations(observations: &Mutex<OnOffObservations>) {
    if let Ok(mut observations) = observations.lock() {
        observations.clear();
    }
}

fn start_controller_event_stream(
    transport: Arc<dyn MatterTransport>,
    event_tx: std::sync::mpsc::Sender<HubEvent>,
    shutdown: Arc<std::sync::atomic::AtomicBool>,
    node_proof_of_life: Arc<Mutex<HashMap<u64, std::time::Instant>>>,
    on_off_observations: Arc<Mutex<OnOffObservations>>,
    pending_turn_on_plans: Arc<Mutex<HashMap<u64, MatterEndpointCommandPlan>>>,
    needs_audition: Arc<Mutex<HashSet<(u64, u16)>>>,
    readback: Arc<crate::hub_state::MatterReadbackCoordinator>,
    attribute_report_history: Arc<Mutex<VecDeque<MatterAttributeReport>>>,
) {
    let subscription_refresh =
        start_observed_state_subscription_worker(transport.clone(), shutdown.clone());
    let spawn_result = std::thread::Builder::new()
        .name("matter-controller-events".to_string())
        .spawn(move || {
            let mut cursor: Option<MatterControllerEventCursor> = None;
            let mut connection_state: Option<bool> = None;
            let mut retry_delay = MATTER_EVENT_RETRY_INITIAL;
            let mut logged_unknown_event = false;

            while !shutdown.load(Ordering::Relaxed) {
                // Bootstrap without a long poll so the stream identity is known
                // before Connected allows callers to admit controller-owned work.
                let max_wait = if cursor.is_some() {
                    MATTER_EVENT_LONG_POLL
                } else {
                    Duration::ZERO
                };
                let batch = match transport.wait_controller_events(cursor.as_ref(), max_wait) {
                    Ok(batch) => batch,
                    Err(error) => {
                        if connection_state != Some(false) {
                            let _ = event_tx.send(HubEvent::Disconnected {
                                hub_key: None,
                                reason: format!("Matter controller event stream: {error:#}"),
                            });
                        }
                        connection_state = Some(false);
                        if wait_for_controller_event_retry(shutdown.as_ref(), retry_delay) {
                            break;
                        }
                        retry_delay = next_controller_event_retry_delay(retry_delay);
                        continue;
                    }
                };
                retry_delay = MATTER_EVENT_RETRY_INITIAL;

                let stream_changed = cursor
                    .as_ref()
                    .is_some_and(|cursor| cursor.stream_id != batch.stream_id);
                let history_gap = cursor.as_ref().is_some_and(|cursor| {
                    cursor.stream_id == batch.stream_id
                        && cursor.sequence.saturating_add(1) < batch.oldest_sequence
                });
                if stream_changed || history_gap {
                    let reason = if stream_changed {
                        "Matter controller sidecar restarted"
                    } else {
                        "Matter controller event history gap"
                    };
                    let _ = event_tx.send(HubEvent::CommandStreamReset {
                        hub_key: None,
                        stream_id: batch.stream_id.clone(),
                        history_gap,
                        reason: reason.to_string(),
                    });
                    // Values are authoritative only within one contiguous
                    // controller event stream. Re-subscription produces a new
                    // initial attribute report for every reachable endpoint.
                    invalidate_on_off_observations(on_off_observations.as_ref());
                    let _ = subscription_refresh.send(MatterSubscriptionRefresh::ControllerReset);
                }
                let mut last_sequence = cursor
                    .as_ref()
                    .filter(|cursor| cursor.stream_id == batch.stream_id)
                    .map(|cursor| cursor.sequence)
                    .unwrap_or(0);
                let event_stream_id = batch.stream_id.clone();
                for envelope in batch.events {
                    last_sequence = last_sequence.max(envelope.sequence);
                    let event = match envelope.event {
                        MatterControllerEvent::CommandOutcome(outcome) => {
                            let pending_plan = pending_turn_on_plans
                                .lock()
                                .ok()
                                .and_then(|mut pending| pending.remove(&outcome.command_id));
                            if matches!(
                                outcome.status,
                                crate::transport::MatterCommandOutcomeStatus::Succeeded
                            ) {
                                if let Some(plan) = pending_plan {
                                    crate::controller::schedule_turn_on_readback(
                                        transport.clone(),
                                        plan,
                                        needs_audition.clone(),
                                        readback.clone(),
                                    );
                                }
                                if let Ok(mut proof) = node_proof_of_life.lock() {
                                    proof.insert(outcome.node_id, std::time::Instant::now());
                                }
                                if let Some(detail) = outcome.detail.as_deref() {
                                    log::warn!(
                                        target: "cmd",
                                        "Matter: command {} for node {} endpoint {} completed with a rejected color step: {}",
                                        outcome.command_id,
                                        outcome.node_id,
                                        outcome.endpoint,
                                        detail
                                    );
                                }
                                let _ = subscription_refresh.send(
                                    MatterSubscriptionRefresh::EndpointProof {
                                        target: MatterSubscriptionTarget {
                                            node_id: outcome.node_id,
                                            endpoint: outcome.endpoint,
                                        },
                                        subscription_active: false,
                                    },
                                );
                            }
                            Some(crate::events::translate_command_outcome(
                                event_stream_id.clone(),
                                outcome,
                            ))
                        }
                        MatterControllerEvent::AttributeReport(report) => {
                            record_attribute_report_history(
                                &report,
                                attribute_report_history.as_ref(),
                            );
                            cache_on_off_observation(&report, on_off_observations.as_ref());
                            if let Ok(mut proof) = node_proof_of_life.lock() {
                                proof.insert(report.node_id, std::time::Instant::now());
                            }
                            let _ = subscription_refresh.send(
                                MatterSubscriptionRefresh::EndpointProof {
                                    target: MatterSubscriptionTarget {
                                        node_id: report.node_id,
                                        endpoint: report.endpoint,
                                    },
                                    subscription_active: true,
                                },
                            );
                            crate::events::translate_report(&report)
                        }
                        MatterControllerEvent::SubscriptionTerminated(termination) => {
                            // Native auto-resubscribe is disabled; this event is
                            // the only prompt notice that observation stopped.
                            //
                            // Drop this endpoint's cached on/off value, and only
                            // this endpoint's: `MatterHubData::observed_on_off`
                            // ignores the stored timestamp and nothing else ages
                            // an entry out, so a retained value would read as
                            // permanently fresh for a subscription that is no
                            // longer reporting. rhythm-os has no event to
                            // translate for a subscription lifecycle change, so
                            // the recovery schedule is the only other effect.
                            // TODO(#368): once DeviceReachability lands, also
                            // emit Failure(Subscription) here — its `None =>`
                            // arm depends on this eviction.
                            forget_on_off_observation(
                                (termination.node_id, termination.endpoint),
                                on_off_observations.as_ref(),
                            );
                            record_attribute_report_history(
                                &MatterAttributeReport {
                                    received_at_unix_ms: now_unix_ms(),
                                    node_id: termination.node_id,
                                    endpoint: termination.endpoint,
                                    cluster: 0,
                                    attr_id: 0,
                                    value: MatterAttributeValue::SubscriptionTerminated,
                                },
                                attribute_report_history.as_ref(),
                            );
                            let _ = subscription_refresh.send(
                                MatterSubscriptionRefresh::SubscriptionTerminated {
                                    target: MatterSubscriptionTarget {
                                        node_id: termination.node_id,
                                        endpoint: termination.endpoint,
                                    },
                                    failure_class: termination.failure_class,
                                },
                            );
                            None
                        }
                        MatterControllerEvent::Unknown => {
                            // Forward compatibility: a newer chipd published an
                            // event kind this build cannot interpret. Skipping
                            // it keeps the cursor advancing instead of stalling
                            // the whole stream.
                            if !logged_unknown_event {
                                logged_unknown_event = true;
                                warn!(
                                    target: "evt",
                                    "Ignoring unrecognized Matter controller event kind; rhythm-matter may be older than chipd"
                                );
                            }
                            None
                        }
                    };
                    if let Some(event) = event {
                        if event_tx.send(event).is_err() {
                            return;
                        }
                    }
                }
                cursor = Some(MatterControllerEventCursor {
                    stream_id: batch.stream_id,
                    sequence: last_sequence,
                });
                if connection_state != Some(true) {
                    let _ = event_tx.send(HubEvent::Connected { hub_key: None });
                    connection_state = Some(true);
                }
            }
        });
    if let Err(error) = spawn_result {
        warn!(target: "evt", "Failed to start Matter controller event stream: {error}");
    }
}

/// Connect to the local Matter fabric.
pub fn connect_matter(
    state: &SharedState,
    transport: Arc<dyn MatterTransport>,
) -> Result<(ActiveHub, Receiver<HubEvent>)> {
    let hub_key = HubKey::new(HubType::new("matter"), "local");
    let persisted_devices = transport.list_commissioned_devices().unwrap_or_default();
    let commissioned = if persisted_devices.is_empty() {
        transport.list_devices().unwrap_or_default()
    } else {
        persisted_devices
            .iter()
            .map(device_info_from_record)
            .collect()
    };
    let cloud_profiles = crate::cloud_profiles::load_or_sync_for_state(state);
    let local_overrides = crate::local_quirks::load_overrides_for_state(state);
    let initial_metadata = initial_device_metadata(
        state,
        &commissioned,
        &persisted_devices,
        &cloud_profiles,
        &local_overrides,
        &hub_key,
    );
    publish_endpoint_capabilities(
        state,
        &hub_key,
        &initial_metadata.device_caps,
        &initial_metadata.fallback_caps,
    )?;
    let next_node_id = next_node_id_seed(&commissioned);
    let fabric_id = configured_fabric_id(state, &hub_key);

    info!(target: "sys", "Matter: {} commissioned devices", commissioned.len());

    let snapshot = {
        let state = state
            .lock()
            .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
        state
            .storage
            .as_ref()
            .and_then(|storage| storage.load_hub_registry_for(&hub_key).ok().flatten())
            .and_then(|value| {
                serde_json::from_value::<rhythm_os::registry::RegistrySnapshot>(value).ok()
            })
    };

    let commissioned_for_closure = commissioned.clone();
    let transport_for_closure = transport.clone();
    let fabric_id_for_closure = fabric_id.clone();
    let cloud_profiles_for_hub_data = cloud_profiles.clone();
    let local_overrides_for_hub_data = local_overrides.clone();
    let node_proof_of_life = Arc::new(Mutex::new(HashMap::new()));
    let node_proof_of_life_for_closure = node_proof_of_life.clone();
    let node_proof_of_life_for_events = node_proof_of_life.clone();
    let on_off_observations = Arc::new(Mutex::new(HashMap::new()));
    let on_off_observations_for_closure = on_off_observations.clone();
    let on_off_observations_for_events = on_off_observations.clone();
    let pending_turn_on_plans = Arc::new(Mutex::new(HashMap::new()));
    let pending_turn_on_plans_for_closure = pending_turn_on_plans.clone();
    let pending_turn_on_plans_for_events = pending_turn_on_plans.clone();
    let needs_audition = Arc::new(Mutex::new(
        local_overrides
            .needs_audition
            .iter()
            .filter_map(|device_id| crate::lifecycle::parse_device_id(device_id))
            .collect::<HashSet<_>>(),
    ));
    let needs_audition_for_closure = needs_audition.clone();
    let needs_audition_for_events = needs_audition.clone();
    let readback = Arc::new(crate::hub_state::MatterReadbackCoordinator::new(
        crate::local_quirks::store_path(state),
    ));
    let readback_for_closure = readback.clone();
    let readback_for_events = readback.clone();
    let attribute_report_history = Arc::new(Mutex::new(VecDeque::new()));
    let attribute_report_history_for_closure = attribute_report_history.clone();
    let attribute_report_history_for_events = attribute_report_history.clone();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let hub_data_event_tx = event_tx.clone();
    let event_transport = transport.clone();

    let (hub, event_rx) = rhythm_os::lifecycle::connect_hub(
        state,
        HubType::new("matter"),
        hub_key,
        true,
        snapshot,
        move |registry: Arc<Mutex<HubDeviceRegistry>>| -> Box<dyn std::any::Any + Send + Sync> {
            let transport_cell = {
                let cell = std::sync::OnceLock::new();
                let _ = cell.set(transport_for_closure.clone());
                cell
            };

            Box::new(Arc::new(MatterHubData {
                transport: transport_cell,
                capture_dir: std::sync::OnceLock::new(),
                registry,
                fabric_id: fabric_id_for_closure.clone(),
                commissioned: std::sync::Mutex::new(commissioned_for_closure.clone()),
                next_node_id: std::sync::atomic::AtomicU64::new(next_node_id),
                device_caps: std::sync::Mutex::new(initial_metadata.device_caps.clone()),
                fallback_caps: std::sync::Mutex::new(initial_metadata.fallback_caps.clone()),
                device_quirks: std::sync::Mutex::new(initial_metadata.device_quirks.clone()),
                device_profiles: std::sync::Mutex::new(initial_metadata.device_profiles.clone()),
                pending_turn_on_plans: pending_turn_on_plans_for_closure.clone(),
                needs_audition: needs_audition_for_closure.clone(),
                readback: readback_for_closure.clone(),
                local_overrides: std::sync::Mutex::new(local_overrides_for_hub_data.clone()),
                cloud_profiles: std::sync::Mutex::new(cloud_profiles_for_hub_data.clone()),
                decommissioning: std::sync::Mutex::new(std::collections::HashSet::new()),
                recently_decommissioned: std::sync::Mutex::new(std::collections::HashMap::new()),
                node_proof_of_life: node_proof_of_life_for_closure.clone(),
                on_off_observations: on_off_observations_for_closure.clone(),
                attribute_report_history: attribute_report_history_for_closure.clone(),
                last_turn_on_dispatch: std::sync::Mutex::new(std::collections::HashMap::new()),
                event_tx: hub_data_event_tx,
            }))
        },
        move |_registry, shutdown| {
            start_controller_event_stream(
                event_transport,
                event_tx,
                shutdown,
                node_proof_of_life_for_events,
                on_off_observations_for_events,
                pending_turn_on_plans_for_events,
                needs_audition_for_events,
                readback_for_events,
                attribute_report_history_for_events,
            );
            event_rx
        },
    )?;

    Ok((hub, event_rx))
}

fn device_info_from_record(device: &CommissionedDevice) -> MatterDeviceInfo {
    MatterDeviceInfo {
        node_id: device.node_id,
        vendor_name: device.vendor_name.clone(),
        product_name: device.product_name.clone(),
        // Persisted metadata proves identity and capabilities, not current
        // liveness. Keep the neutral startup behavior; command/read outcomes
        // update reachability after connect without starting background work.
        reachable: true,
    }
}

fn configured_fabric_id(state: &SharedState, hub_key: &HubKey) -> String {
    state
        .lock()
        .ok()
        .and_then(|state| state.hub_credentials.get(hub_key).cloned())
        .and_then(|creds| crate::provider::matter_fabric_id(&creds).map(ToOwned::to_owned))
        .unwrap_or_else(|| "default".to_string())
}

fn next_node_id_seed(commissioned: &[MatterDeviceInfo]) -> u64 {
    commissioned
        .iter()
        .map(|device| device.node_id)
        .max()
        .unwrap_or(99)
        .saturating_add(1)
}

#[derive(Clone)]
struct InitialDeviceMetadata {
    device_caps: HashMap<String, LightCapabilities>,
    fallback_caps: HashSet<String>,
    device_quirks: HashMap<String, Vec<DeviceQuirk>>,
    device_profiles: HashMap<String, crate::control_profile::MatterControlProfile>,
}

pub(crate) fn normalized_endpoint_capabilities(
    capabilities: &LightCapabilities,
    is_fallback: bool,
) -> Option<serde_json::Value> {
    let color_temperature = if capabilities.supports_color_temp() {
        let min_kelvin = capabilities.min_kelvin?;
        let max_kelvin = capabilities.max_kelvin?;
        if min_kelvin == 0 || min_kelvin > max_kelvin {
            return None;
        }
        Some(LightColorTemperatureCapabilitiesDto {
            min_kelvin,
            max_kelvin,
        })
    } else {
        None
    };

    let mut normalized = serde_json::json!({
        "light_capabilities": LightCapabilitiesDto {
            color_temperature,
            individual_profile_overrides: None,
        },
    });
    if !is_fallback {
        normalized["automatic_naming"] = serde_json::json!({
            "color_kind": if capabilities.light_type == rhythm_devices::LightType::ExtendedColor {
                "color"
            } else {
                "white"
            }
        });
    }
    Some(normalized)
}

fn publish_endpoint_capabilities(
    state: &SharedState,
    hub_key: &HubKey,
    device_capabilities: &HashMap<String, LightCapabilities>,
    fallback_caps: &HashSet<String>,
) -> Result<()> {
    let mut state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("Failed to lock state"))?;
    let mut changed = false;

    let canonical_ids = state
        .canonical_registry
        .devices()
        .map(|device| device.id.clone())
        .collect::<Vec<_>>();
    for canonical_id in canonical_ids {
        let Some(device) = state.canonical_registry.get_mut(&canonical_id) else {
            continue;
        };
        for endpoint in &mut device.endpoints {
            if &endpoint.hub_key != hub_key || !endpoint.active {
                continue;
            }
            let Some(capabilities) = device_capabilities.get(&endpoint.native_id) else {
                continue;
            };
            let Some(normalized) = normalized_endpoint_capabilities(
                capabilities,
                fallback_caps.contains(&endpoint.native_id),
            ) else {
                continue;
            };
            if endpoint.capabilities.as_ref() != Some(&normalized) {
                endpoint.capabilities = Some(normalized);
                changed = true;
            }
        }
    }

    if changed {
        rhythm_os::commands::save_authority_state(&state)?;
    }
    Ok(())
}

/// Build the best metadata available without talking to the device.
///
/// `list_devices` is backed by chipd's persisted device store, so its vendor
/// and product names survive a temporary connectivity failure. Preserve any
/// matching built-in profile here instead of replacing it with an anonymous
/// colour-temperature fallback from `fallback_device_capabilities`. In
/// particular, command quirks such as
/// `NeedsExplicitOn` remain necessary while the device is unreachable to
/// probes but reachable again by the time a light command is dispatched.
fn fallback_device_metadata(info: &MatterDeviceInfo) -> (LightCapabilities, Vec<DeviceQuirk>) {
    let Some(entry) =
        rhythm_devices::builtin_db().lookup(info.vendor_name.as_str(), info.product_name.as_str())
    else {
        return (
            crate::commissioning::fallback_device_capabilities(),
            Vec::new(),
        );
    };

    let quirks = entry
        .matter
        .as_ref()
        .map(|matter| matter.quirks.clone())
        .unwrap_or_default();
    (entry.capabilities(), quirks)
}

fn fallback_initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut device_caps = HashMap::new();
    let mut fallback_caps = HashSet::new();
    let mut device_quirks = HashMap::new();
    let mut device_profiles = HashMap::new();
    let commissioned_nodes = commissioned
        .iter()
        .map(|info| info.node_id)
        .collect::<HashSet<_>>();

    let mut insert_fallback = |device_id: String, info: Option<&MatterDeviceInfo>| {
        let (caps, quirks) = info.map(fallback_device_metadata).unwrap_or_else(|| {
            (
                crate::commissioning::fallback_device_capabilities(),
                Vec::new(),
            )
        });
        let profile = crate::control_profile::profile_from_legacy(
            &caps,
            &quirks,
            crate::control_profile::MatterProfileSource::Builtin,
        );
        device_caps.entry(device_id.clone()).or_insert(caps);
        fallback_caps.insert(device_id.clone());
        device_quirks.entry(device_id.clone()).or_insert(quirks);
        device_profiles.entry(device_id).or_insert(profile);
    };

    for info in commissioned {
        insert_fallback(format_device_id(info.node_id, 1), Some(info));
    }

    if let Ok(state) = state.lock() {
        for device in state.canonical_registry.devices() {
            for endpoint in device.active_endpoints() {
                if &endpoint.hub_key != hub_key {
                    continue;
                }
                let Some((node_id, _)) = parse_device_id(&endpoint.native_id) else {
                    continue;
                };
                if commissioned_nodes.contains(&node_id) {
                    let info = commissioned.iter().find(|info| info.node_id == node_id);
                    insert_fallback(endpoint.native_id.clone(), info);
                }
            }
        }
    }

    InitialDeviceMetadata {
        device_caps,
        fallback_caps,
        device_quirks,
        device_profiles,
    }
}

fn initial_device_metadata(
    state: &SharedState,
    commissioned: &[MatterDeviceInfo],
    persisted_devices: &[CommissionedDevice],
    cloud_profiles: &crate::cloud_profiles::CloudMatterProfileCatalog,
    local_overrides: &crate::local_quirks::LocalMatterOverrides,
    hub_key: &HubKey,
) -> InitialDeviceMetadata {
    let mut metadata = fallback_initial_device_metadata(state, commissioned, hub_key);
    let aliases_by_node = state
        .lock()
        .ok()
        .map(|state| {
            state
                .canonical_registry
                .devices()
                .flat_map(|device| device.active_endpoints())
                .filter(|endpoint| &endpoint.hub_key == hub_key)
                .filter_map(|endpoint| {
                    parse_device_id(&endpoint.native_id)
                        .map(|(node_id, _)| (node_id, endpoint.native_id.clone()))
                })
                .fold(
                    HashMap::<u64, Vec<String>>::new(),
                    |mut aliases, (node_id, id)| {
                        aliases.entry(node_id).or_default().push(id);
                        aliases
                    },
                )
        })
        .unwrap_or_default();
    for device in persisted_devices {
        let device_id = format_device_id(device.node_id, device.light_endpoint);
        let resolved = crate::commissioning::resolve_device_metadata(
            device,
            &device_id,
            cloud_profiles,
            local_overrides,
        );
        let caps = resolved.capabilities;
        let quirks = resolved.quirks;
        let profile = resolved.control_profile;

        metadata.device_caps.insert(device_id.clone(), caps.clone());
        metadata.fallback_caps.remove(&device_id);
        metadata
            .device_quirks
            .insert(device_id.clone(), quirks.clone());
        metadata
            .device_profiles
            .insert(device_id.clone(), profile.clone());
        if let Some(aliases) = aliases_by_node.get(&device.node_id) {
            for alias in aliases {
                metadata.device_caps.insert(alias.clone(), caps.clone());
                metadata.fallback_caps.remove(alias);
                metadata.device_quirks.insert(alias.clone(), quirks.clone());
                metadata
                    .device_profiles
                    .insert(alias.clone(), profile.clone());
            }
        }
    }
    metadata
}

/// Format a Matter node ID as a device ID string.
pub fn format_device_id(node_id: u64, endpoint: u16) -> String {
    if endpoint == 1 {
        format!("matter-{}", node_id)
    } else {
        format!("matter-{}-{}", node_id, endpoint)
    }
}

/// Parse a device ID string back into `(node_id, endpoint)`.
pub fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
    let rest = device_id.strip_prefix("matter-")?;
    match rest.split_once('-') {
        Some((node_str, endpoint_str)) => {
            let node_id = node_str.parse::<u64>().ok()?;
            let endpoint = endpoint_str.parse::<u16>().ok()?;
            Some((node_id, endpoint))
        }
        None => {
            let node_id = rest.parse::<u64>().ok()?;
            Some((node_id, 1))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use rhythm_core::runtime::hub_registry::DeviceType;
    use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
    use rhythm_os::state::AppState;

    use crate::provider::matter_credentials;
    use crate::transport::{
        CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterGroup,
        MatterGroupMember, MatterSubscriptionTarget,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct FakeMatterTransport {
        devices: Mutex<Vec<MatterDeviceInfo>>,
        persisted_devices: Mutex<Vec<CommissionedDevice>>,
        probe_calls: AtomicUsize,
        subscribe_calls: AtomicUsize,
        subscription_attempts: Mutex<Vec<MatterSubscriptionTarget>>,
        subscription_failures_remaining: Mutex<HashMap<(u64, u16), usize>>,
        list_calls: AtomicUsize,
        fail_listing: AtomicBool,
        queued_events: Mutex<Vec<MatterControllerEvent>>,
        event_wait_calls: AtomicUsize,
        block_initial_event_wait: AtomicBool,
        release_initial_event_wait: AtomicBool,
    }

    impl FakeMatterTransport {
        fn new(devices: Vec<MatterDeviceInfo>, persisted_devices: Vec<CommissionedDevice>) -> Self {
            Self {
                devices: Mutex::new(devices),
                persisted_devices: Mutex::new(persisted_devices),
                probe_calls: AtomicUsize::new(0),
                subscribe_calls: AtomicUsize::new(0),
                subscription_attempts: Mutex::new(Vec::new()),
                subscription_failures_remaining: Mutex::new(HashMap::new()),
                list_calls: AtomicUsize::new(0),
                fail_listing: AtomicBool::new(false),
                queued_events: Mutex::new(Vec::new()),
                event_wait_calls: AtomicUsize::new(0),
                block_initial_event_wait: AtomicBool::new(false),
                release_initial_event_wait: AtomicBool::new(false),
            }
        }

        fn fail_subscription_attempts(&self, node_id: u64, endpoint: u16, attempt_count: usize) {
            self.subscription_failures_remaining
                .lock()
                .unwrap()
                .insert((node_id, endpoint), attempt_count);
        }

        fn remove_commissioned_device(&self, node_id: u64) {
            self.persisted_devices
                .lock()
                .unwrap()
                .retain(|device| device.node_id != node_id);
        }

        fn add_commissioned_device(&self, device: CommissionedDevice) {
            self.persisted_devices.lock().unwrap().push(device);
        }

        fn queue_controller_event(&self, event: MatterControllerEvent) {
            self.queued_events.lock().unwrap().push(event);
        }
    }

    impl MatterTransport for FakeMatterTransport {
        fn commission_light(
            &self,
            _request: &MatterCommissionRequest,
        ) -> Result<CommissionedDevice> {
            anyhow::bail!("not used")
        }

        fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
            Ok(())
        }

        fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_listing.load(Ordering::SeqCst) {
                anyhow::bail!("simulated CHIP RPC failure listing devices");
            }
            Ok(self.devices.lock().unwrap().clone())
        }

        fn list_commissioned_devices(&self) -> Result<Vec<CommissionedDevice>> {
            self.list_calls.fetch_add(1, Ordering::SeqCst);
            if self.fail_listing.load(Ordering::SeqCst) {
                anyhow::bail!("simulated CHIP RPC failure listing commissioned devices");
            }
            Ok(self.persisted_devices.lock().unwrap().clone())
        }

        fn probe_light(&self, _node_id: u64) -> Result<CommissionedDevice> {
            self.probe_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("startup must not probe persisted Matter devices")
        }

        fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
            Ok(())
        }

        fn configure_group(&self, _group: &MatterGroup) -> Result<()> {
            Ok(())
        }

        fn remove_group(&self, _group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
            Ok(())
        }

        fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
            Ok(())
        }

        fn set_brightness(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _level: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_color_temperature(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _kelvin: u16,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_xy(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _x: f32,
            _y: f32,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn set_hue_saturation(
            &self,
            _node_id: u64,
            _endpoint: u16,
            _hue: u8,
            _saturation: u8,
            _transition_ms: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }

        fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
            Ok(false)
        }

        fn subscribe_on_off(
            &self,
            targets: &[MatterSubscriptionTarget],
            _min_interval_secs: u16,
            _max_interval_secs: u16,
        ) -> Result<()> {
            assert_eq!(
                targets.len(),
                1,
                "observed-state subscriptions must be isolated per endpoint"
            );
            let target = targets[0].clone();
            self.subscription_attempts
                .lock()
                .unwrap()
                .push(target.clone());
            let mut failures = self.subscription_failures_remaining.lock().unwrap();
            let should_fail =
                if let Some(remaining) = failures.get_mut(&(target.node_id, target.endpoint)) {
                    if *remaining > 0 {
                        *remaining -= 1;
                        true
                    } else {
                        false
                    }
                } else {
                    false
                };
            self.subscribe_calls.fetch_add(1, Ordering::SeqCst);
            if should_fail {
                anyhow::bail!("simulated operational discovery timeout")
            }
            Ok(())
        }

        fn wait_controller_events(
            &self,
            cursor: Option<&MatterControllerEventCursor>,
            max_wait: Duration,
        ) -> Result<crate::transport::MatterControllerEventBatch> {
            let call_index = self.event_wait_calls.fetch_add(1, Ordering::SeqCst);
            if call_index == 0 && self.block_initial_event_wait.load(Ordering::SeqCst) {
                while !self.release_initial_event_wait.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
            }
            if !max_wait.is_zero() {
                std::thread::sleep(max_wait);
            }
            let mut next_sequence = cursor.map(|cursor| cursor.sequence).unwrap_or(0);
            let events = std::mem::take(&mut *self.queued_events.lock().unwrap())
                .into_iter()
                .map(|event| {
                    next_sequence += 1;
                    crate::transport::MatterControllerEventEnvelope {
                        sequence: next_sequence,
                        event,
                    }
                })
                .collect();
            Ok(crate::transport::MatterControllerEventBatch {
                stream_id: cursor
                    .map(|cursor| cursor.stream_id.clone())
                    .unwrap_or_else(|| "fake-controller".to_string()),
                oldest_sequence: cursor.map(|cursor| cursor.sequence + 1).unwrap_or(1),
                events,
            })
        }
    }

    fn shared_state(prefix: &str) -> SharedState {
        static NEXT_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let id = NEXT_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "rhythm-matter-lifecycle-{}-{}-{}",
            prefix,
            std::process::id(),
            id
        ));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).unwrap();
        }
        std::fs::create_dir_all(&dir).unwrap();
        let state = Arc::new(Mutex::new(AppState::default()));
        state.lock().unwrap().data_dir = dir.to_string_lossy().to_string();
        state
    }

    fn wait_for_atomic_at_least(value: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while value.load(Ordering::SeqCst) < expected && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert!(
            value.load(Ordering::SeqCst) >= expected,
            "timed out waiting for {expected} calls"
        );
    }

    fn device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            reachable: true,
        }
    }

    fn h6004_device_info(node_id: u64) -> MatterDeviceInfo {
        MatterDeviceInfo {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            reachable: true,
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: format!("Vendor {node_id}"),
            product_name: format!("Lamp {node_id}"),
            vendor_id: 100,
            product_id: 200,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn h6004_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H6004".to_string(),
            vendor_id: 4999,
            product_id: 24580,
            serial_number: Some(format!("h6004-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2000),
            max_kelvin: Some(6500),
        }
    }

    fn h7056_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Shenzhen Qianyan Technology".to_string(),
            product_name: "H7056".to_string(),
            vendor_id: 4999,
            product_id: 28758,
            serial_number: Some(format!("h7056-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    fn moes_matter_light(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "MOES".to_string(),
            product_name: "MOES Matter Light".to_string(),
            vendor_id: 5245,
            product_id: 1412,
            serial_number: Some(format!("moes-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::Xy,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2702),
            max_kelvin: Some(6535),
        }
    }

    fn sengled_w41_device(node_id: u64) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            vendor_id: 4448,
            product_id: 36866,
            serial_number: Some(format!("sengled-{node_id}")),
            light_endpoint: 1,
            color_modes: vec![
                MatterColorMode::HueSaturation,
                MatterColorMode::ColorTemperature,
            ],
            min_kelvin: None,
            max_kelvin: None,
        }
    }

    #[test]
    fn parse_simple_device_id() {
        assert_eq!(parse_device_id("matter-100"), Some((100, 1)));
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(parse_device_id("matter-42-2"), Some((42, 2)));
    }

    #[test]
    fn parse_invalid_prefix() {
        assert_eq!(parse_device_id("hue-abc123"), None);
    }

    #[test]
    fn parse_invalid_node_id() {
        assert_eq!(parse_device_id("matter-abc"), None);
    }

    #[test]
    fn roundtrip() {
        assert_eq!(parse_device_id(&format_device_id(55, 1)), Some((55, 1)));
        assert_eq!(parse_device_id(&format_device_id(55, 3)), Some((55, 3)));
    }

    #[test]
    fn next_node_id_seed_uses_max_commissioned_node() {
        let commissioned = vec![
            MatterDeviceInfo {
                node_id: 100,
                vendor_name: "A".to_string(),
                product_name: "Light".to_string(),
                reachable: true,
            },
            MatterDeviceInfo {
                node_id: 105,
                vendor_name: "B".to_string(),
                product_name: "Lamp".to_string(),
                reachable: true,
            },
        ];

        assert_eq!(next_node_id_seed(&commissioned), 106);
    }

    #[test]
    fn fallback_initial_metadata_includes_persisted_endpoint_ids() {
        let state = shared_state("fallback-endpoints");
        let key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-12-2".to_string(),
            room_id: None,
            room_name: None,
            name: "Endpoint two bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("12")],
            manufacturer: None,
            model: None,
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 100);

        let metadata = fallback_initial_device_metadata(&state, &[device_info(12)], &key);

        assert!(metadata.device_caps.contains_key("matter-12"));
        assert!(metadata.device_caps.contains_key("matter-12-2"));
        assert!(metadata.fallback_caps.contains("matter-12"));
        assert!(metadata.fallback_caps.contains("matter-12-2"));
    }

    #[test]
    fn normalized_endpoint_capabilities_omits_naming_hint_for_fallback_caps() {
        let capabilities =
            LightCapabilities::defaults_for(rhythm_devices::LightType::ColorTemperature);

        let fallback = normalized_endpoint_capabilities(&capabilities, true).unwrap();
        assert!(fallback.get("light_capabilities").is_some());
        assert!(fallback.get("automatic_naming").is_none());

        let probed = normalized_endpoint_capabilities(&capabilities, false).unwrap();
        assert_eq!(
            probed.pointer("/automatic_naming/color_kind"),
            Some(&serde_json::json!("white"))
        );
    }

    #[test]
    fn startup_publishes_persisted_matter_capabilities_to_canonical_endpoint() {
        let state = shared_state("publish-endpoint-capabilities");
        let key = HubKey::new(HubType::new("matter"), "local");
        let identity = DiscoveredIdentity {
            native_id: "matter-101".to_string(),
            room_id: None,
            room_name: None,
            name: "AiDot Smart RGBTW Bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("101")],
            manufacturer: Some("AiDot".to_string()),
            model: Some("Smart RGBTW Bulb".to_string()),
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&identity, &key, 100);
        let fallback_identity = DiscoveredIdentity {
            native_id: "matter-102".to_string(),
            room_id: None,
            room_name: None,
            name: "Unprobed bulb".to_string(),
            device_type: DeviceType::Light,
            hardware_ids: vec![HardwareId::matter("102")],
            manufacturer: None,
            model: None,
        };
        state
            .lock()
            .unwrap()
            .canonical_registry
            .resolve(&fallback_identity, &key, 100);
        let capabilities = HashMap::from([
            (
                "matter-101".to_string(),
                LightCapabilities {
                    color_modes: vec![
                        rhythm_devices::ColorMode::HueSaturation,
                        rhythm_devices::ColorMode::ColorTemperature,
                    ],
                    min_kelvin: Some(2702),
                    max_kelvin: Some(6535),
                    ..LightCapabilities::defaults_for(rhythm_devices::LightType::ExtendedColor)
                },
            ),
            (
                "matter-102".to_string(),
                LightCapabilities::defaults_for(rhythm_devices::LightType::ColorTemperature),
            ),
        ]);
        let fallback_caps = HashSet::from(["matter-102".to_string()]);

        publish_endpoint_capabilities(&state, &key, &capabilities, &fallback_caps).unwrap();

        let state = state.lock().unwrap();
        let endpoint = state
            .canonical_registry
            .find_by_native_id(&key, "matter-101")
            .and_then(|device| device.endpoint_by_native_id("matter-101"))
            .unwrap();
        assert_eq!(
            endpoint
                .capabilities
                .as_ref()
                .and_then(|value| value.pointer("/light_capabilities/color_temperature")),
            Some(&serde_json::json!({
                "min_kelvin": 2702,
                "max_kelvin": 6535,
            }))
        );
        let fallback_endpoint = state
            .canonical_registry
            .find_by_native_id(&key, "matter-102")
            .and_then(|device| device.endpoint_by_native_id("matter-102"))
            .unwrap();
        assert!(fallback_endpoint
            .capabilities
            .as_ref()
            .is_some_and(|value| value.get("light_capabilities").is_some()));
        assert!(fallback_endpoint
            .capabilities
            .as_ref()
            .is_some_and(|value| value.get("automatic_naming").is_none()));
    }

    #[test]
    fn fallback_initial_metadata_preserves_known_device_quirks() {
        let state = shared_state("fallback-known-quirks");
        let key = HubKey::new(HubType::new("matter"), "local");

        let metadata = fallback_initial_device_metadata(&state, &[h6004_device_info(107)], &key);

        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert!(metadata
            .device_caps
            .get("matter-107")
            .is_some_and(LightCapabilities::supports_hue_saturation));
        assert!(!metadata
            .device_caps
            .get("matter-107")
            .is_some_and(LightCapabilities::supports_xy_color));
    }

    #[test]
    fn fallback_initial_metadata_preserves_sengled_hue_saturation_profile() {
        let state = shared_state("fallback-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let info = MatterDeviceInfo {
            node_id: 104,
            vendor_name: "Sengled".to_string(),
            product_name: "W41-N15A".to_string(),
            reachable: false,
        };

        let metadata = fallback_initial_device_metadata(&state, &[info], &key);
        let caps = metadata.device_caps.get("matter-104").unwrap();

        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn persisted_metadata_applies_local_overrides_without_curated_color_preference() {
        let state = shared_state("persisted-overrides");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = commissioned_device(107, 1);
        crate::local_quirks::save_device_profile_override(
            &state,
            "matter-107",
            Some(vec![DeviceQuirk::NeedsXyNotCt]),
            Some(crate::local_quirks::LocalCapabilityOverride {
                min_brightness: Some(17),
                supports_transition: Some(false),
            }),
            Some("test-report".to_string()),
        )
        .unwrap();

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &crate::local_quirks::load_overrides_for_state(&state),
            &key,
        );

        let caps = metadata.device_caps.get("matter-107").unwrap();
        assert_eq!(caps.min_brightness, Some(17));
        assert!(!caps.supports_transition);
        assert_eq!(
            metadata.device_quirks.get("matter-107"),
            Some(&vec![DeviceQuirk::NeedsXyNotCt])
        );
    }

    #[test]
    fn typed_profile_keeps_legacy_local_transition_evidence() {
        let state = shared_state("persisted-legacy-and-typed-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = commissioned_device(108, 1);
        crate::local_quirks::save_device_profile_override(
            &state,
            "matter-108",
            None,
            Some(crate::local_quirks::LocalCapabilityOverride {
                min_brightness: None,
                supports_transition: Some(false),
            }),
            Some("legacy-report".to_string()),
        )
        .unwrap();
        crate::local_quirks::save_device_control_profile(
            &state,
            "matter-108",
            crate::control_profile::MatterControlProfile {
                supports_transition: true,
                source: crate::control_profile::MatterControlProfileSources {
                    supports_transition: crate::control_profile::MatterProfileSource::Builtin,
                    ..crate::control_profile::MatterControlProfileSources::default()
                },
                ..crate::control_profile::MatterControlProfile::default()
            },
            Some("typed-report".to_string()),
        )
        .unwrap();

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &crate::local_quirks::load_overrides_for_state(&state),
            &key,
        );

        let profile = metadata.device_profiles.get("matter-108").unwrap();
        assert!(!profile.supports_transition);
        assert_eq!(
            profile.source.supports_transition,
            crate::control_profile::MatterProfileSource::Audition
        );
    }

    #[test]
    fn legacy_projection_keeps_curated_ct_while_runtime_honours_local_audition() {
        let state = shared_state("persisted-moes-color-preference");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = moes_matter_light(112);
        crate::local_quirks::save_device_profile_override(
            &state,
            "matter-112",
            Some(vec![
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ]),
            None,
            Some("stale-hs-report".to_string()),
        )
        .unwrap();

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &crate::local_quirks::load_overrides_for_state(&state),
            &key,
        );

        assert_eq!(
            metadata.device_quirks.get("matter-112"),
            Some(&vec![
                DeviceQuirk::CommandThrottleMs(250),
                DeviceQuirk::Other(
                    rhythm_devices::quirks::PREFER_COLOR_TEMPERATURE_QUIRK.to_string(),
                ),
            ])
        );
        let profile = metadata.device_profiles.get("matter-112").unwrap();
        assert_eq!(
            profile.color_route,
            crate::control_profile::MatterColorRoute::HueSaturation
        );
        assert_eq!(
            profile.source.color_route,
            crate::control_profile::MatterProfileSource::Audition
        );
        assert_eq!(profile.command_spacing_ms.value_ms, 250);
    }

    #[test]
    fn persisted_sengled_metadata_keeps_profile_hue_saturation_and_explicit_on() {
        let state = shared_state("persisted-sengled-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = sengled_w41_device(104);

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &crate::local_quirks::load_overrides_for_state(&state),
            &key,
        );

        let caps = metadata.device_caps.get("matter-104").unwrap();
        assert!(caps.supports_hue_saturation());
        assert!(!caps.supports_xy_color());
        assert_eq!(
            metadata.device_quirks.get("matter-104"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
            ])
        );
    }

    #[test]
    fn persisted_h7056_metadata_publishes_profiled_ct_range() {
        let state = shared_state("persisted-h7056-profile");
        let key = HubKey::new(HubType::new("matter"), "local");
        let device = h7056_device(116);

        let metadata = initial_device_metadata(
            &state,
            &[device_info_from_record(&device)],
            &[device],
            &crate::cloud_profiles::CloudMatterProfileCatalog::default(),
            &crate::local_quirks::load_overrides_for_state(&state),
            &key,
        );

        let caps = metadata.device_caps.get("matter-116").unwrap();
        assert_eq!(caps.min_kelvin, Some(3080));
        assert_eq!(caps.max_kelvin, Some(6120));
        assert_eq!(
            normalized_endpoint_capabilities(caps, false).and_then(|value| {
                value
                    .pointer("/light_capabilities/color_temperature")
                    .cloned()
            }),
            Some(serde_json::json!({
                "min_kelvin": 3080,
                "max_kelvin": 6120,
            }))
        );
    }

    #[test]
    fn connect_matter_uses_persisted_records_without_probing_and_starts_subscription_stream() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect");
        let key = HubKey::new(HubType::new("matter"), "local");
        state
            .lock()
            .unwrap()
            .hub_credentials
            .insert(key.clone(), matter_credentials("local", "fabric-test"));

        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(10, 2), h6004_device(107)],
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();

        assert_eq!(hub.hub_key, key);
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(data.fabric_id, "fabric-test");
        assert_eq!(data.commissioned.lock().unwrap().len(), 2);
        assert!(data.device_caps.lock().unwrap().contains_key("matter-10-2"));
        assert!(data.device_caps.lock().unwrap().contains_key("matter-107"));
        assert_eq!(
            data.next_node_id.load(std::sync::atomic::Ordering::SeqCst),
            108
        );
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&key));
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 2);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }

    #[test]
    fn observed_state_subscriptions_isolate_offline_endpoint_and_retry_recovery() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(103, 1), commissioned_device(110, 1)],
        ));
        transport.fail_subscription_attempts(103, 1, 1);
        let shutdown = Arc::new(AtomicBool::new(false));

        let refresh = start_observed_state_subscription_worker(transport.clone(), shutdown.clone());
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap().as_slice(),
            &[
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
            ],
            "the healthy endpoint must still be attempted after its peer fails"
        );

        refresh
            .send(MatterSubscriptionRefresh::ControllerReset)
            .unwrap();
        wait_for_atomic_at_least(&transport.subscribe_calls, 4);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap().as_slice(),
            &[
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 103,
                    endpoint: 1,
                },
                MatterSubscriptionTarget {
                    node_id: 110,
                    endpoint: 1,
                },
            ],
            "the next refresh must retry the failed endpoint without losing healthy subscriptions"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn observed_state_subscription_backoff_is_per_endpoint_jittered_and_resets_on_recovery() {
        let devices: Vec<_> = (1..=22)
            .map(|node_id| commissioned_device(node_id, 1))
            .collect();
        let transport = Arc::new(FakeMatterTransport::new(Vec::new(), devices));
        // Model one assigned endpoint that recovers on its third attempt and
        // one roomless commissioned endpoint that remains unavailable.
        transport.fail_subscription_attempts(7, 1, 2);
        transport.fail_subscription_attempts(22, 1, 100);
        let shutdown = Arc::new(AtomicBool::new(false));

        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_millis(20),
            Duration::from_millis(80),
            MatterSubscriptionCadence::production(),
        );
        wait_for_atomic_at_least(&transport.subscribe_calls, 22);

        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        loop {
            let attempts = transport.subscription_attempts.lock().unwrap();
            let recovered_attempts = attempts.iter().filter(|target| target.node_id == 7).count();
            if recovered_attempts >= 3 {
                break;
            }
            drop(attempts);
            assert!(
                std::time::Instant::now() < deadline,
                "timed out waiting for the assigned endpoint to recover"
            );
            std::thread::yield_now();
        }

        let attempts_after_recovery = transport.subscription_attempts.lock().unwrap().clone();
        for healthy_node in (1..=21).filter(|node_id| *node_id != 7) {
            assert_eq!(
                attempts_after_recovery
                    .iter()
                    .filter(|target| target.node_id == healthy_node)
                    .count(),
                1,
                "healthy endpoint {healthy_node} must not be re-subscribed on a peer's retry tick"
            );
        }
        assert_eq!(
            attempts_after_recovery
                .iter()
                .filter(|target| target.node_id == 7)
                .count(),
            3,
            "successful proof of life must clear the endpoint retry schedule"
        );
        assert!(
            attempts_after_recovery
                .iter()
                .filter(|target| target.node_id == 22)
                .count()
                <= 3,
            "an unavailable roomless endpoint must remain bounded while another endpoint recovers"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn endpoint_proof_of_life_clears_subscription_cooldown_immediately() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(44, 1)],
        ));
        transport.fail_subscription_attempts(44, 1, 1);
        let shutdown = Arc::new(AtomicBool::new(false));
        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_secs(10),
            Duration::from_secs(10),
            MatterSubscriptionCadence::production(),
        );
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);

        refresh
            .send(MatterSubscriptionRefresh::EndpointProof {
                target: MatterSubscriptionTarget {
                    node_id: 44,
                    endpoint: 1,
                },
                subscription_active: false,
            })
            .unwrap();
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(
            transport.subscribe_calls.load(Ordering::SeqCst),
            2,
            "a command acknowledgement must bypass the stale ten-second subscription cooldown"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn controller_reset_is_lossless_after_a_saturated_proof_burst() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(51, 1)],
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_secs(10),
            Duration::from_secs(10),
            MatterSubscriptionCadence::production(),
        );
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);

        for _ in 0..256 {
            refresh
                .send(MatterSubscriptionRefresh::EndpointProof {
                    target: MatterSubscriptionTarget {
                        node_id: 51,
                        endpoint: 1,
                    },
                    subscription_active: true,
                })
                .unwrap();
        }
        refresh
            .send(MatterSubscriptionRefresh::ControllerReset)
            .unwrap();

        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 2);

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn removed_endpoint_is_not_rebuilt_after_controller_reset() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(61, 1), commissioned_device(62, 1)],
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_secs(10),
            Duration::from_secs(10),
            MatterSubscriptionCadence::production(),
        );
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);

        transport.remove_commissioned_device(62);
        refresh
            .send(MatterSubscriptionRefresh::ControllerReset)
            .unwrap();
        wait_for_atomic_at_least(&transport.subscribe_calls, 3);

        let attempts = transport.subscription_attempts.lock().unwrap();
        assert_eq!(attempts.len(), 3);
        assert_eq!(attempts[2].node_id, 61);

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    fn worker_state() -> MatterSubscriptionWorkerState {
        MatterSubscriptionWorkerState::new(
            MATTER_SUBSCRIPTION_RETRY_INITIAL,
            MATTER_SUBSCRIPTION_RETRY_MAX,
            MatterSubscriptionCadence::production(),
        )
    }

    #[test]
    fn command_proof_keeps_a_healthy_subscription_and_does_not_relist_devices() {
        let transport = FakeMatterTransport::new(Vec::new(), vec![commissioned_device(81, 1)]);
        let shutdown = AtomicBool::new(false);
        let mut state = worker_state();

        state.refresh_targets(&transport);
        state.run_subscribe_pass(&transport, &shutdown);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        assert_eq!(transport.list_calls.load(Ordering::SeqCst), 1);

        for _ in 0..8 {
            state.apply(MatterSubscriptionRefresh::EndpointProof {
                target: MatterSubscriptionTarget {
                    node_id: 81,
                    endpoint: 1,
                },
                subscription_active: false,
            });
        }
        assert!(
            !state.should_refresh_targets(),
            "a proof for a known endpoint must not cost a ListDevices RPC"
        );
        state.run_subscribe_pass(&transport, &shutdown);

        assert_eq!(
            transport.subscribe_calls.load(Ordering::SeqCst),
            1,
            "a command outcome must not evict a healthy subscription"
        );
        assert_eq!(transport.list_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn proof_for_an_unknown_endpoint_refreshes_the_target_cache() {
        let transport = FakeMatterTransport::new(Vec::new(), vec![commissioned_device(82, 1)]);
        let mut state = worker_state();
        state.refresh_targets(&transport);
        assert!(!state.should_refresh_targets());
        let unknown = MatterSubscriptionRefresh::EndpointProof {
            target: MatterSubscriptionTarget {
                node_id: 83,
                endpoint: 1,
            },
            subscription_active: true,
        };

        state.apply(unknown);
        assert!(
            !state.should_refresh_targets(),
            "a burst of proofs right after a refresh must not re-list once each"
        );

        // Age the cache past the debounce and the same unknown endpoint now
        // does invalidate it.
        state.targets_refreshed_at = std::time::Instant::now()
            .checked_sub(MATTER_SUBSCRIPTION_UNKNOWN_KEY_REFRESH_DEBOUNCE + Duration::from_secs(1))
            .unwrap();
        state.apply(MatterSubscriptionRefresh::EndpointProof {
            target: MatterSubscriptionTarget {
                node_id: 83,
                endpoint: 1,
            },
            subscription_active: true,
        });

        assert!(
            state.should_refresh_targets(),
            "an endpoint the cache has never seen must trigger one refresh"
        );
    }

    #[test]
    fn subscription_termination_evicts_success_and_schedules_a_backoff() {
        let transport = FakeMatterTransport::new(Vec::new(), vec![commissioned_device(84, 1)]);
        let mut state = worker_state();
        state.refresh_targets(&transport);
        let key = (84, 1);
        state
            .subscribed
            .insert(key, std::time::Instant::now() + Duration::from_secs(60));

        state.apply(MatterSubscriptionRefresh::SubscriptionTerminated {
            target: MatterSubscriptionTarget {
                node_id: 84,
                endpoint: 1,
            },
            failure_class: MatterSubscriptionFailureClass::PeerClosed,
        });

        assert!(
            !state.subscribed.contains_key(&key),
            "a terminated subscription is no longer observing"
        );
        let retry = state.retries.get(&key).copied().expect("backoff scheduled");
        assert_eq!(retry.failures, 1);
        assert!(
            retry.next_attempt > std::time::Instant::now(),
            "recovery costs one backoff step, not an immediate retry storm"
        );
        assert!(
            !state.should_refresh_targets(),
            "a termination for a known endpoint must not cost a ListDevices RPC"
        );
        assert_eq!(transport.list_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn worker_wait_is_bounded_at_both_ends() {
        let mut state = worker_state();
        assert_eq!(
            state.next_wait(),
            MATTER_SUBSCRIPTION_MAX_IDLE_WAIT,
            "an idle worker must not park for an hour"
        );

        state.known_targets.insert((85, 1));
        state.apply(MatterSubscriptionRefresh::SubscriptionTerminated {
            target: MatterSubscriptionTarget {
                node_id: 85,
                endpoint: 1,
            },
            failure_class: MatterSubscriptionFailureClass::AddressResolution,
        });
        for _ in 0..20 {
            state.apply(MatterSubscriptionRefresh::SubscriptionTerminated {
                target: MatterSubscriptionTarget {
                    node_id: 85,
                    endpoint: 1,
                },
                failure_class: MatterSubscriptionFailureClass::AddressResolution,
            });
        }

        assert!(
            state.retries[&(85, 1)]
                .next_attempt
                .saturating_duration_since(std::time::Instant::now())
                > MATTER_SUBSCRIPTION_MAX_IDLE_WAIT,
            "the backoff itself must have reached its 30 minute ceiling"
        );
        assert!(state.next_wait() <= MATTER_SUBSCRIPTION_MAX_IDLE_WAIT);

        // A due-now entry still yields; the floor wins over any smaller cap.
        state.subscribed.insert(
            (85, 2),
            std::time::Instant::now()
                .checked_sub(Duration::from_secs(5))
                .unwrap(),
        );
        assert_eq!(state.next_wait(), MATTER_SUBSCRIPTION_MIN_WAIT);
        state.cadence.max_idle_wait = Duration::from_millis(1);
        assert_eq!(state.next_wait(), MATTER_SUBSCRIPTION_MIN_WAIT);
    }

    #[test]
    fn terminated_subscription_is_rebuilt_only_after_its_backoff() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(71, 1)],
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_millis(300),
            Duration::from_millis(300),
            MatterSubscriptionCadence::production(),
        );
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);

        let terminated_at = std::time::Instant::now();
        refresh
            .send(MatterSubscriptionRefresh::SubscriptionTerminated {
                target: MatterSubscriptionTarget {
                    node_id: 71,
                    endpoint: 1,
                },
                failure_class: MatterSubscriptionFailureClass::PeerClosed,
            })
            .unwrap();
        wait_for_atomic_at_least(&transport.subscribe_calls, 2);
        let recovered_after = terminated_at.elapsed();

        assert!(
            recovered_after >= Duration::from_millis(150),
            "a termination must schedule a backoff, not resubscribe immediately: {recovered_after:?}"
        );
        assert_eq!(
            transport.subscribe_calls.load(Ordering::SeqCst),
            2,
            "one termination costs exactly one recovery attempt"
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn device_commissioned_after_startup_is_subscribed_within_the_idle_wait_cap() {
        let transport = Arc::new(FakeMatterTransport::new(Vec::new(), Vec::new()));
        let shutdown = Arc::new(AtomicBool::new(false));
        // A long backoff must not delay a brand-new endpoint, and the idle wait
        // is capped rather than running to the next retry (up to 30 min away).
        let refresh = start_observed_state_subscription_worker_with_backoff(
            transport.clone(),
            shutdown.clone(),
            Duration::from_secs(30 * 60),
            Duration::from_secs(30 * 60),
            MatterSubscriptionCadence {
                subscription_recheck: Duration::from_secs(600),
                target_refresh: Duration::from_millis(50),
                max_idle_wait: Duration::from_millis(250),
            },
        );

        transport.add_commissioned_device(commissioned_device(91, 1));
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap()[0],
            MatterSubscriptionTarget {
                node_id: 91,
                endpoint: 1,
            }
        );

        shutdown.store(true, Ordering::SeqCst);
        let _ = refresh.send(MatterSubscriptionRefresh::ControllerReset);
    }

    #[test]
    fn a_failed_pass_never_leaves_the_worker_spinning() {
        // Regression: the failure path used to leave the endpoint's past-due
        // recheck deadline in `subscribed`, so next_wait() stayed ZERO while
        // the endpoint itself was skipped by its own backoff — a busy loop.
        let transport = FakeMatterTransport::new(Vec::new(), vec![commissioned_device(86, 1)]);
        transport.fail_subscription_attempts(86, 1, 1);
        let shutdown = AtomicBool::new(false);
        let mut state = worker_state();
        state.refresh_targets(&transport);
        // Model a subscription whose recheck already came due.
        state.subscribed.insert(
            (86, 1),
            std::time::Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap(),
        );

        state.run_subscribe_pass(&transport, &shutdown);

        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        assert!(
            !state.subscribed.contains_key(&(86, 1)),
            "a failed attempt must drop the stale recheck deadline"
        );
        assert!(
            state.next_wait() >= MATTER_SUBSCRIPTION_MIN_WAIT,
            "a failing endpoint must never drive the worker loop to a zero wait"
        );
    }

    #[test]
    fn a_failed_device_listing_leaves_subscription_state_untouched() {
        // Regression: a transient list failure used to surface as an empty
        // fabric, pruning every recorded success and cooldown and causing a
        // re-subscribe stampede on the next pass.
        let transport = FakeMatterTransport::new(Vec::new(), vec![commissioned_device(87, 1)]);
        let shutdown = AtomicBool::new(false);
        let mut state = worker_state();
        state.refresh_targets(&transport);
        state.run_subscribe_pass(&transport, &shutdown);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        let refreshed_at = state.targets_refreshed_at;

        transport.fail_listing.store(true, Ordering::SeqCst);
        state.needs_target_refresh = true;
        state.refresh_targets(&transport);

        assert_eq!(
            state.targets.len(),
            1,
            "cached targets survive a list failure"
        );
        assert!(state.subscribed.contains_key(&(87, 1)));
        assert!(
            state.needs_target_refresh,
            "a failed listing must stay pending instead of counting as done"
        );
        assert_eq!(
            state.targets_refreshed_at, refreshed_at,
            "only a successful listing advances the refresh timestamp"
        );

        state.run_subscribe_pass(&transport, &shutdown);
        assert_eq!(
            transport.subscribe_calls.load(Ordering::SeqCst),
            1,
            "a sidecar blip must not restampede every healthy endpoint"
        );
    }

    #[test]
    fn a_genuinely_empty_fabric_is_not_a_listing_error() {
        let transport = FakeMatterTransport::new(Vec::new(), Vec::new());
        assert!(subscription_targets(&transport).unwrap().is_empty());

        transport.fail_listing.store(true, Ordering::SeqCst);
        assert!(
            subscription_targets(&transport).is_err(),
            "both the persisted listing and the live fallback failed"
        );
    }

    #[test]
    fn subscribe_pass_puts_healthy_endpoints_ahead_of_known_dead_ones() {
        let transport = FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(88, 1), commissioned_device(89, 1)],
        );
        // Node 88 is listed first but is the dead one.
        transport.fail_subscription_attempts(88, 1, 100);
        let shutdown = AtomicBool::new(false);
        let mut state = worker_state();
        state.refresh_targets(&transport);
        state.run_subscribe_pass(&transport, &shutdown);
        assert_eq!(
            transport.subscription_attempts.lock().unwrap().len(),
            2,
            "the first pass attempts both, in list order"
        );

        // A controller reset makes every endpoint due again; the endpoint with
        // no recorded failures must not queue behind the dead one's timeouts.
        state.apply(MatterSubscriptionRefresh::ControllerReset);
        state.refresh_targets(&transport);
        state.retries.insert(
            (88, 1),
            MatterSubscriptionRetry {
                failures: 3,
                next_attempt: std::time::Instant::now()
                    .checked_sub(Duration::from_secs(1))
                    .unwrap(),
            },
        );

        assert_eq!(
            state
                .pass_order()
                .iter()
                .map(|target| target.node_id)
                .collect::<Vec<_>>(),
            vec![89, 88]
        );
    }

    #[test]
    fn subscription_retry_delay_uses_bounded_endpoint_jitter() {
        let initial = Duration::from_secs(30);
        let maximum = Duration::from_secs(30 * 60);
        let first = subscription_retry_delay(
            &MatterSubscriptionTarget {
                node_id: 1,
                endpoint: 1,
            },
            1,
            initial,
            maximum,
        );
        let peer = subscription_retry_delay(
            &MatterSubscriptionTarget {
                node_id: 2,
                endpoint: 1,
            },
            1,
            initial,
            maximum,
        );
        let capped = subscription_retry_delay(
            &MatterSubscriptionTarget {
                node_id: 1,
                endpoint: 1,
            },
            100,
            initial,
            maximum,
        );

        assert!((Duration::from_millis(22_500)..=initial).contains(&first));
        assert_ne!(first, peer, "peers should not synchronize retry work");
        assert!((Duration::from_secs(22 * 60 + 30)..=maximum).contains(&capped));
    }

    #[test]
    fn subscribe_rpc_failure_class_reuses_the_shared_sidecar_classifier() {
        assert_eq!(
            subscribe_rpc_failure_class(&anyhow::anyhow!(
                "operational discovery timed out for node 0x1234"
            )),
            MatterSubscriptionFailureClass::AddressResolution
        );
        assert_eq!(
            subscribe_rpc_failure_class(&anyhow::anyhow!("resource is busy for fabric 7")),
            MatterSubscriptionFailureClass::ResourceBusy
        );
        assert_eq!(
            subscribe_rpc_failure_class(&anyhow::anyhow!("something the SDK never says")),
            MatterSubscriptionFailureClass::Other
        );
    }

    #[test]
    fn terminated_subscription_is_scheduled_for_recovery_and_not_forwarded_to_the_hub() {
        let transport = Arc::new(FakeMatterTransport::new(
            Vec::new(),
            vec![commissioned_device(93, 1)],
        ));
        transport.queue_controller_event(MatterControllerEvent::SubscriptionTerminated(
            crate::transport::MatterSubscriptionTermination {
                node_id: 93,
                endpoint: 1,
                failure_class: MatterSubscriptionFailureClass::Timeout,
                chip_error: 0x0000_0032,
                detail: None,
            },
        ));
        let shutdown = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = std::sync::mpsc::channel();
        // Two cached observations; only the terminated endpoint's may be lost.
        let observations = Arc::new(Mutex::new(HashMap::from([
            ((93, 1), (true, std::time::Instant::now())),
            ((94, 1), (true, std::time::Instant::now())),
        ])));
        let report_history = Arc::new(Mutex::new(VecDeque::new()));

        start_controller_event_stream(
            transport.clone(),
            event_tx,
            shutdown.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            observations.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashSet::new())),
            Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            report_history.clone(),
        );

        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            HubEvent::Connected { .. }
        ));
        {
            let observations = observations.lock().unwrap();
            assert!(
                !observations.contains_key(&(93, 1)),
                "a terminated subscription must not keep reading as freshly observed"
            );
            assert!(
                observations.contains_key(&(94, 1)),
                "other endpoints keep their cached observation"
            );
        }
        assert!(
            event_rx.recv_timeout(Duration::from_millis(50)).is_err(),
            "a subscription lifecycle change is not a hub event"
        );
        // The default worker backoff is 30s, so recovery is scheduled rather
        // than attempted inline: the termination must not start a retry storm.
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        assert!(report_history.lock().unwrap().iter().any(|report| {
            report.node_id == 93
                && report.endpoint == 1
                && matches!(&report.value, MatterAttributeValue::SubscriptionTerminated)
        }));

        shutdown.store(true, Ordering::SeqCst);
    }

    #[test]
    fn controller_stream_identity_is_known_before_connected_is_emitted() {
        let transport = Arc::new(FakeMatterTransport::new(Vec::new(), Vec::new()));
        transport
            .block_initial_event_wait
            .store(true, Ordering::SeqCst);
        let shutdown = Arc::new(AtomicBool::new(false));
        let (event_tx, event_rx) = std::sync::mpsc::channel();

        start_controller_event_stream(
            transport.clone(),
            event_tx,
            shutdown.clone(),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(Mutex::new(HashSet::new())),
            Arc::new(crate::hub_state::MatterReadbackCoordinator::default()),
            Arc::new(Mutex::new(VecDeque::new())),
        );

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while transport.event_wait_calls.load(Ordering::SeqCst) == 0
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(transport.event_wait_calls.load(Ordering::SeqCst), 1);
        assert!(event_rx.recv_timeout(Duration::from_millis(50)).is_err());

        transport
            .release_initial_event_wait
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            event_rx.recv_timeout(Duration::from_secs(1)).unwrap(),
            HubEvent::Connected { .. }
        ));
        shutdown.store(true, Ordering::SeqCst);
    }

    #[test]
    fn on_off_subscription_reports_update_periodic_observation_cache() {
        let observations = Mutex::new(HashMap::new());
        let report = crate::transport::MatterAttributeReport {
            received_at_unix_ms: 0,
            node_id: 42,
            endpoint: 2,
            cluster: crate::clusters::CLUSTER_ON_OFF_U32,
            attr_id: crate::clusters::ATTR_ON_OFF_U32,
            value: MatterAttributeValue::Bool(true),
        };

        cache_on_off_observation(&report, &observations);

        assert_eq!(
            observations
                .lock()
                .unwrap()
                .get(&(42, 2))
                .map(|(lights_on, _)| *lights_on),
            Some(true)
        );
    }

    #[test]
    fn controller_report_history_preserves_audition_evidence_without_native_redrain() {
        let history = Mutex::new(VecDeque::new());
        let report = crate::transport::MatterAttributeReport {
            received_at_unix_ms: 1234,
            node_id: 42,
            endpoint: 2,
            cluster: crate::clusters::CLUSTER_ON_OFF_U32,
            attr_id: crate::clusters::ATTR_ON_OFF_U32,
            value: MatterAttributeValue::Bool(true),
        };

        record_attribute_report_history(&report, &history);

        assert_eq!(history.lock().unwrap().front(), Some(&report));
    }

    #[test]
    fn controller_stream_reset_invalidates_subscription_observations() {
        let observations = Mutex::new(HashMap::from([(
            (42, 2),
            (true, std::time::Instant::now()),
        )]));

        invalidate_on_off_observations(&observations);

        assert!(observations.lock().unwrap().is_empty());
    }

    #[test]
    fn controller_event_retry_is_bounded_and_shutdown_aware() {
        let mut delay = MATTER_EVENT_RETRY_INITIAL;
        assert_eq!(delay, Duration::from_millis(250));
        delay = next_controller_event_retry_delay(delay);
        assert_eq!(delay, Duration::from_millis(500));
        delay = next_controller_event_retry_delay(delay);
        assert_eq!(delay, Duration::from_secs(1));
        for _ in 0..8 {
            delay = next_controller_event_retry_delay(delay);
        }
        assert_eq!(delay, MATTER_EVENT_RETRY_MAX);

        let shutdown = Arc::new(AtomicBool::new(false));
        let shutdown_for_thread = shutdown.clone();
        let setter = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            shutdown_for_thread.store(true, Ordering::SeqCst);
        });
        assert!(wait_for_controller_event_retry(
            shutdown.as_ref(),
            MATTER_EVENT_RETRY_INITIAL
        ));
        setter.join().unwrap();
    }

    #[test]
    fn connect_matter_falls_back_to_basic_list_without_probing_old_transports() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_MATTER_PROFILE_SYNC", "disabled");
        let state = shared_state("connect-old-transport");
        let transport = Arc::new(FakeMatterTransport::new(
            vec![h6004_device_info(107)],
            Vec::new(),
        ));

        let (hub, event_rx) = connect_matter(&state, transport.clone()).unwrap();
        let data = hub.data::<Arc<MatterHubData>>().unwrap();
        assert_eq!(
            data.device_quirks.lock().unwrap().get("matter-107"),
            Some(&vec![
                DeviceQuirk::NeedsExplicitOn,
                DeviceQuirk::NeedsHueSaturationNotCt,
                DeviceQuirk::CommandThrottleMs(250),
            ])
        );
        assert_eq!(transport.probe_calls.load(Ordering::SeqCst), 0);

        let event = event_rx
            .recv_timeout(std::time::Duration::from_secs(1))
            .unwrap();
        assert_eq!(event.hub_key(), Some(&hub.hub_key));
        wait_for_atomic_at_least(&transport.subscribe_calls, 1);
        assert_eq!(transport.subscribe_calls.load(Ordering::SeqCst), 1);
        std::env::remove_var("RHYTHM_MATTER_PROFILE_SYNC");
    }
}
