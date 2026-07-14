//! Composite light controller for multi-hub fan-out.
//!
//! Wraps multiple per-hub [`HubLightController`] implementations and routes
//! commands to the correct hub(s) based on a room routing table. Supports
//! dynamic registration — controllers can be added/removed while the
//! `RhythmEngine` is running.
//!
//! ## Non-blocking dispatch
//!
//! Device commands NEVER block the caller. `turn_on`/`turn_off` place the
//! command in a per-hub mailbox and return immediately; a per-hub async
//! worker on a dedicated dispatch runtime delivers commands to the hub
//! transport with:
//!
//! - **Latest-wins coalescing** per target: commands carry absolute light
//!   state, so while a target is busy or paced, newer commands replace older
//!   queued ones instead of piling up. The mailbox can never overflow.
//! - **Per-target ordering**: a target never has two dispatches in flight;
//!   independent targets dispatch concurrently up to `max_in_flight`.
//! - **Supervised timeouts**: each physical dispatch runs on the blocking
//!   pool under a `tokio::time::timeout`. A hung transport call can only
//!   linger until its own transport timeout — it never occupies the hub
//!   worker or leaks an unbounded thread.
//! - **Hub pacing**: an optional token bucket (used for Hue) keeps command
//!   rates within what the bridge tolerates (~10 light commands/s, ~1 group
//!   command/s). Commands wait in the mailbox — and keep coalescing — until
//!   tokens are available.
//! - **Outcome events**: every dispatch reports a [`HubDispatchOutcome`]
//!   (success, failure, timeout, cooldown-skip, drop) through the composite's
//!   outcome listener so failures surface as events instead of vanishing
//!   into logs.
//!
//! ## Usage
//!
//! ```ignore
//! let composite = CompositeController::new();
//! composite.register_controller("hue@192.168.1.5", hue_controller);
//! composite.update_routing(hashmap! {
//!     "living-room" => vec![(
//!         "hue@192.168.1.5".to_string(),
//!         HubDispatchTarget::Group {
//!             room_id: "living-room".to_string(),
//!             control_id: "gl-living-room".to_string(),
//!         },
//!     )],
//! });
//! composite.set_outcome_listener(Arc::new(|outcome| {
//!     // forward failures to the event bus
//! }));
//! ```

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::warn;
use tokio::sync::Notify;

use crate::controller::{
    HubCommandDelivery, HubCommandReceipt, HubDispatchTarget, HubLightController,
    LightControlError, LightControlResult, LightController,
};
use crate::lighting::LightingCommand;
use crate::room::Room;

const DISPATCH_WARN_MS: u128 = 1000;
const DEFAULT_HUB_DISPATCH_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_HUB_TIMEOUT_COOLDOWN: Duration = Duration::from_secs(30);
const DEFAULT_MATTER_TIMEOUT_COOLDOWN: Duration = Duration::from_secs(120);
const DEFAULT_QUERY_TIMEOUT: Duration = Duration::from_secs(6);
const MATTER_QUERY_TIMEOUT: Duration = Duration::from_secs(9);
const GET_ROOMS_TIMEOUT: Duration = Duration::from_secs(20);
const IS_CONNECTED_TIMEOUT: Duration = Duration::from_secs(5);
const FLASH_TIMEOUT: Duration = Duration::from_secs(20);
const DEFAULT_MAX_IN_FLIGHT: usize = 2;
const MATTER_MAX_IN_FLIGHT: usize = 4;

/// Hue bridges tolerate roughly 10 per-light commands and 1 group command
/// per second. One bucket approximates both: light commands cost 1, group
/// commands cost the full refill second.
const HUE_RATE_LIMIT: HubRateLimit = HubRateLimit {
    capacity: 8.0,
    refill_per_sec: 8.0,
    group_cost: 8.0,
    device_cost: 1.0,
};

/// The dedicated runtime that hub dispatch workers, supervised transport
/// calls, and query timeouts run on.
///
/// Dispatch must work from any thread — engine worker threads, HTTP handler
/// threads, and axum tasks all enqueue commands — so the composite owns its
/// own small runtime instead of borrowing whichever context the caller has.
fn dispatch_runtime() -> &'static tokio::runtime::Runtime {
    static DISPATCH_RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    DISPATCH_RUNTIME.get_or_init(|| {
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(32)
            .thread_name("rhythm-dispatch")
            .enable_all()
            .build()
            .expect("failed to build hub dispatch runtime")
    })
}

/// Run a (potentially blocking) controller call on the dispatch runtime's
/// blocking pool with a supervision timeout.
///
/// The returned join handle can be awaited from any executor. On timeout the
/// blocking call keeps running detached — bounded by the transport's own
/// timeouts — but the caller is released immediately.
fn spawn_supervised<T, F>(
    timeout: Duration,
    what: String,
    f: F,
) -> tokio::task::JoinHandle<LightControlResult<T>>
where
    T: Send + 'static,
    F: FnOnce() -> LightControlResult<T> + Send + 'static,
{
    dispatch_runtime().spawn(async move {
        match tokio::time::timeout(timeout, tokio::task::spawn_blocking(f)).await {
            Ok(Ok(result)) => result,
            Ok(Err(join_error)) => Err(LightControlError::Internal(format!(
                "{} task failed: {}",
                what, join_error
            ))),
            Err(_) => Err(LightControlError::Timeout(format!(
                "{} exceeded {}ms",
                what,
                timeout.as_millis()
            ))),
        }
    })
}

async fn await_supervised<T>(
    handle: tokio::task::JoinHandle<LightControlResult<T>>,
) -> LightControlResult<T> {
    handle.await.unwrap_or_else(|e| {
        Err(LightControlError::Internal(format!(
            "query task failed: {e}"
        )))
    })
}

/// Scope of the cooldown applied after a physical dispatch timeout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubDispatchTimeoutScope {
    /// Only reject retries for the same target.
    Target,
    /// Reject all commands for this hub while the timed-out job may still be alive.
    Hub,
}

/// Token bucket parameters pacing a hub's outbound command rate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HubRateLimit {
    /// Maximum burst size in tokens.
    pub capacity: f64,
    /// Tokens restored per second.
    pub refill_per_sec: f64,
    /// Cost of a group/room command.
    pub group_cost: f64,
    /// Cost per device of a device-list command.
    pub device_cost: f64,
}

/// Per-hub command dispatch policy.
#[derive(Clone, Debug, PartialEq)]
pub struct HubDispatchPolicy {
    /// Maximum concurrent physical dispatches for this hub.
    pub max_in_flight: usize,
    /// Optional outbound command pacing.
    pub rate_limit: Option<HubRateLimit>,
    /// Fan `HubDispatchTarget::Devices` out into one dispatch per device so
    /// a single lagging device cannot delay its siblings (used for Matter).
    pub split_device_targets: bool,
    /// Maximum time a single physical dispatch may run before it is reported
    /// as timed out and its target cooled down.
    pub dispatch_timeout: Duration,
    /// How long to reject commands after a dispatch timeout.
    pub timeout_cooldown: Duration,
    /// Whether timeout cooldown applies to one target or the whole hub.
    pub timeout_scope: HubDispatchTimeoutScope,
    /// Maximum time a state query (`any_lights_on`, …) may take.
    pub query_timeout: Duration,
}

impl HubDispatchPolicy {
    /// Build the default policy for a registered hub key.
    pub fn for_hub_key(hub_key: &str) -> Self {
        match hub_type_from_key(hub_key) {
            "matter" => Self {
                max_in_flight: MATTER_MAX_IN_FLIGHT,
                rate_limit: None,
                split_device_targets: true,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_MATTER_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Target,
                query_timeout: MATTER_QUERY_TIMEOUT,
            },
            "hue" => Self {
                max_in_flight: DEFAULT_MAX_IN_FLIGHT,
                rate_limit: Some(HUE_RATE_LIMIT),
                split_device_targets: false,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_HUB_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Target,
                query_timeout: DEFAULT_QUERY_TIMEOUT,
            },
            _ => Self {
                max_in_flight: DEFAULT_MAX_IN_FLIGHT,
                rate_limit: None,
                split_device_targets: false,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_HUB_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Target,
                query_timeout: DEFAULT_QUERY_TIMEOUT,
            },
        }
    }
}

/// Public metadata describing a registered hub command queue.
#[derive(Clone, Debug, PartialEq)]
pub struct HubDispatchMetadata {
    pub hub_key: String,
    pub hub_type: String,
    pub policy: HubDispatchPolicy,
}

/// What kind of light command a dispatch outcome refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubDispatchKind {
    TurnOn,
    TurnOff,
}

impl HubDispatchKind {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::TurnOn => "turn_on",
            Self::TurnOff => "turn_off",
        }
    }
}

/// Terminal status of a dispatched (or rejected) hub command.
#[derive(Clone, Debug, PartialEq)]
pub enum HubDispatchStatus {
    /// The hub transport accepted the command.
    Succeeded,
    /// A controller service accepted the command and will publish its
    /// terminal physical outcome separately.
    Accepted {
        command_ids: Vec<u64>,
        controller_stream_id: String,
    },
    /// The hub transport reported an error.
    Failed { error: String },
    /// The dispatch exceeded the policy timeout; a cooldown was applied.
    TimedOut { timeout_ms: u64 },
    /// The command was rejected/dropped because its target (or hub) is
    /// cooling down after a timeout.
    SkippedCooldown { remaining_ms: u64 },
    /// The command was dropped without dispatch (hub removed/replaced).
    Dropped { reason: String },
}

impl HubDispatchStatus {
    pub fn is_success(&self) -> bool {
        matches!(self, Self::Succeeded | Self::Accepted { .. })
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Succeeded => "succeeded",
            Self::Accepted { .. } => "accepted",
            Self::Failed { .. } => "failed",
            Self::TimedOut { .. } => "timed_out",
            Self::SkippedCooldown { .. } => "skipped_cooldown",
            Self::Dropped { .. } => "dropped",
        }
    }

    pub fn detail(&self) -> Option<String> {
        match self {
            Self::Succeeded => None,
            Self::Accepted { command_ids, .. } => Some(format!(
                "controller command ids {}",
                command_ids
                    .iter()
                    .map(u64::to_string)
                    .collect::<Vec<_>>()
                    .join(",")
            )),
            Self::Failed { error } => Some(error.clone()),
            Self::TimedOut { timeout_ms } => Some(format!("exceeded {}ms", timeout_ms)),
            Self::SkippedCooldown { remaining_ms } => {
                Some(format!("cooling down for {}ms", remaining_ms))
            }
            Self::Dropped { reason } => Some(reason.clone()),
        }
    }
}

/// The result of one hub command dispatch, reported to the outcome listener.
#[derive(Clone, Debug)]
pub struct HubDispatchOutcome {
    pub hub_key: String,
    pub hub_type: String,
    /// Topology node the engine addressed.
    pub node_id: String,
    /// Hub-native target label.
    pub target_label: String,
    pub kind: HubDispatchKind,
    pub status: HubDispatchStatus,
    /// Time the command spent queued before dispatch started.
    pub queued_ms: u64,
    /// Time the physical dispatch took (0 for enqueue-time rejections).
    pub dispatch_ms: u64,
    /// How many earlier queued commands this one superseded.
    pub coalesced: u32,
}

/// Callback invoked for every dispatch outcome (success and failure).
pub type HubDispatchOutcomeListener = Arc<dyn Fn(HubDispatchOutcome) + Send + Sync>;

type SharedOutcomeListener = Arc<RwLock<Option<HubDispatchOutcomeListener>>>;

/// A command accepted into a hub mailbox.
///
/// Every queued event is matched by exactly one [`HubDispatchOutcome`] later
/// (dispatch result, cooldown drop, or close drop) — enqueue-time rejections
/// emit a synthetic queued event immediately before their outcome to keep the
/// pairing exact. A coalesce that changes the node a mailbox slot addresses
/// carries the superseded node so listeners can rebalance per-node counters.
#[derive(Clone, Debug)]
pub struct HubDispatchQueued {
    pub hub_key: String,
    pub hub_type: String,
    /// Topology node the engine addressed.
    pub node_id: String,
    /// Node a coalesce displaced from this mailbox slot, if any.
    pub superseded_node_id: Option<String>,
}

/// Callback invoked when a command is accepted into a hub mailbox.
pub type HubDispatchQueuedListener = Arc<dyn Fn(HubDispatchQueued) + Send + Sync>;

type SharedQueuedListener = Arc<RwLock<Option<HubDispatchQueuedListener>>>;

pub(crate) fn format_node_log_label(node_id: &str, node_name: Option<&str>) -> String {
    match node_name
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != node_id)
    {
        Some(name) => format!("{} ({})", name, node_id),
        None => node_id.to_string(),
    }
}

fn hub_type_from_key(hub_key: &str) -> &str {
    hub_key
        .split_once('@')
        .map_or(hub_key, |(hub_type, _)| hub_type)
}

// ── Dispatch actions ─────────────────────────────────────────────────────

#[derive(Clone)]
enum HubDispatchAction {
    TurnOn {
        target: HubDispatchTarget,
        command: LightingCommand,
    },
    TurnOff {
        target: HubDispatchTarget,
        transition_ms: Option<u32>,
    },
}

impl HubDispatchAction {
    fn kind(&self) -> HubDispatchKind {
        match self {
            Self::TurnOn { .. } => HubDispatchKind::TurnOn,
            Self::TurnOff { .. } => HubDispatchKind::TurnOff,
        }
    }

    fn target(&self) -> &HubDispatchTarget {
        match self {
            Self::TurnOn { target, .. } => target,
            Self::TurnOff { target, .. } => target,
        }
    }

    /// Mailbox key: one slot per hub-native target.
    fn target_key(&self) -> String {
        self.target().label()
    }

    /// Element ids used for in-flight conflict detection. Two dispatch units
    /// may run concurrently only if their element sets are disjoint.
    fn elements(&self) -> Vec<String> {
        match self.target() {
            HubDispatchTarget::Group { control_id, .. } => vec![format!("group:{control_id}")],
            HubDispatchTarget::Devices { native_ids } => {
                native_ids.iter().map(|id| format!("dev:{id}")).collect()
            }
        }
    }

    fn cost(&self, limit: &HubRateLimit) -> f64 {
        match self.target() {
            HubDispatchTarget::Group { .. } => limit.group_cost,
            HubDispatchTarget::Devices { native_ids } => {
                limit.device_cost * native_ids.len().max(1) as f64
            }
        }
    }

    async fn dispatch(
        self,
        controller: Arc<dyn HubLightController>,
    ) -> LightControlResult<HubCommandReceipt> {
        match self {
            Self::TurnOn { target, command } => {
                controller
                    .turn_on_target_with_receipt(&target, command)
                    .await
            }
            Self::TurnOff {
                target,
                transition_ms,
            } => {
                controller
                    .turn_off_target_with_receipt(&target, transition_ms)
                    .await
            }
        }
    }
}

struct PendingJob {
    node_id: String,
    action: HubDispatchAction,
    enqueued_at: Instant,
    coalesced: u32,
}

struct ReadyJob {
    key: String,
    job: PendingJob,
}

// ── Dispatch queue state ─────────────────────────────────────────────────

struct DispatchQueue {
    /// FIFO of mailbox keys awaiting dispatch.
    order: VecDeque<String>,
    /// Latest command per mailbox key.
    pending: HashMap<String, PendingJob>,
    /// Element ids currently being dispatched.
    in_flight: HashSet<String>,
    in_flight_count: usize,
    target_cooldowns: HashMap<String, Instant>,
    hub_cooldown_until: Option<Instant>,
    tokens: f64,
    last_refill: Instant,
    closed: bool,
}

impl DispatchQueue {
    fn new(rate_limit: Option<&HubRateLimit>) -> Self {
        Self {
            order: VecDeque::new(),
            pending: HashMap::new(),
            in_flight: HashSet::new(),
            in_flight_count: 0,
            target_cooldowns: HashMap::new(),
            hub_cooldown_until: None,
            tokens: rate_limit.map(|limit| limit.capacity).unwrap_or(0.0),
            last_refill: Instant::now(),
            closed: false,
        }
    }

    fn refill_tokens(&mut self, limit: &HubRateLimit, now: Instant) {
        let elapsed = now.saturating_duration_since(self.last_refill);
        self.tokens =
            (self.tokens + elapsed.as_secs_f64() * limit.refill_per_sec).min(limit.capacity);
        self.last_refill = now;
    }

    fn expire_cooldowns(&mut self, now: Instant) {
        if self
            .hub_cooldown_until
            .is_some_and(|cooldown_until| cooldown_until <= now)
        {
            self.hub_cooldown_until = None;
        }
        self.target_cooldowns
            .retain(|_, cooldown_until| *cooldown_until > now);
    }

    fn active_cooldown(&self, key: &str, now: Instant) -> Option<Duration> {
        let cooldown_until = match self.hub_cooldown_until {
            Some(until) if until > now => Some(until),
            _ => self.target_cooldowns.get(key).copied().filter(|u| *u > now),
        };
        cooldown_until.map(|until| until.saturating_duration_since(now))
    }
}

enum WorkerStep {
    /// Nothing dispatchable; wait for a notification.
    Wait,
    /// Pacing/token shortage; wait up to the duration (or a notification).
    Sleep(Duration),
    /// Dispatch this job now.
    Dispatch(ReadyJob),
    /// The dispatcher is closed; exit the worker.
    Exit,
}

// ── Hub dispatcher ───────────────────────────────────────────────────────

struct DispatcherShared {
    key: String,
    hub_type: String,
    controller: Arc<dyn HubLightController>,
    policy: HubDispatchPolicy,
    queue: Mutex<DispatchQueue>,
    notify: Notify,
    listener: SharedOutcomeListener,
    queued_listener: SharedQueuedListener,
}

impl DispatcherShared {
    fn emit(&self, outcome: HubDispatchOutcome) {
        match &outcome.status {
            HubDispatchStatus::Succeeded | HubDispatchStatus::Accepted { .. } => {
                if outcome.dispatch_ms as u128 >= DISPATCH_WARN_MS {
                    tracing::warn!(
                        target: "cmd",
                        event = "hub_dispatch_slow",
                        hub = %outcome.hub_key,
                        node_id = %outcome.node_id,
                        dispatch_target = %outcome.target_label,
                        kind = outcome.kind.as_str(),
                        queued_ms = outcome.queued_ms,
                        dispatch_ms = outcome.dispatch_ms,
                        coalesced = outcome.coalesced,
                        "Hub dispatch slow"
                    );
                } else {
                    tracing::debug!(
                        target: "cmd",
                        event = "hub_dispatch_ok",
                        hub = %outcome.hub_key,
                        node_id = %outcome.node_id,
                        dispatch_target = %outcome.target_label,
                        kind = outcome.kind.as_str(),
                        queued_ms = outcome.queued_ms,
                        dispatch_ms = outcome.dispatch_ms,
                        coalesced = outcome.coalesced,
                        "Hub dispatch ok"
                    );
                }
            }
            status => {
                tracing::warn!(
                    target: "cmd",
                    event = "hub_dispatch_failed",
                    hub = %outcome.hub_key,
                    node_id = %outcome.node_id,
                    dispatch_target = %outcome.target_label,
                    kind = outcome.kind.as_str(),
                    status = status.as_str(),
                    detail = %status.detail().unwrap_or_default(),
                    queued_ms = outcome.queued_ms,
                    dispatch_ms = outcome.dispatch_ms,
                    coalesced = outcome.coalesced,
                    "Hub dispatch did not complete"
                );
            }
        }

        let listener = self.listener.read().ok().and_then(|l| l.clone());
        if let Some(listener) = listener {
            listener(outcome);
        }
    }

    fn emit_queued(&self, node_id: &str, superseded_node_id: Option<String>) {
        let listener = self.queued_listener.read().ok().and_then(|l| l.clone());
        if let Some(listener) = listener {
            listener(HubDispatchQueued {
                hub_key: self.key.clone(),
                hub_type: self.hub_type.clone(),
                node_id: node_id.to_string(),
                superseded_node_id,
            });
        }
    }

    fn outcome_for(
        &self,
        node_id: &str,
        action: &HubDispatchAction,
        status: HubDispatchStatus,
        queued_ms: u64,
        dispatch_ms: u64,
        coalesced: u32,
    ) -> HubDispatchOutcome {
        HubDispatchOutcome {
            hub_key: self.key.clone(),
            hub_type: self.hub_type.clone(),
            node_id: node_id.to_string(),
            target_label: action.target_key(),
            kind: action.kind(),
            status,
            queued_ms,
            dispatch_ms,
            coalesced,
        }
    }

    /// Accept a command into the mailbox. Never blocks.
    fn enqueue(&self, node_id: &str, action: HubDispatchAction) -> LightControlResult<()> {
        let key = action.target_key();
        let now = Instant::now();
        let mut queue = match self.queue.lock() {
            Ok(queue) => queue,
            Err(_) => {
                return Err(LightControlError::Internal(format!(
                    "Hub {} dispatch queue poisoned",
                    self.key
                )))
            }
        };
        if queue.closed {
            return Err(LightControlError::ConnectionError(format!(
                "Hub {} dispatch worker is not running",
                self.key
            )));
        }
        queue.expire_cooldowns(now);

        if let Some(remaining) = queue.active_cooldown(&key, now) {
            drop(queue);
            let remaining_ms = remaining.as_millis() as u64;
            // Synthetic queued event so this enqueue-time rejection still
            // pairs 1:1 with the outcome below.
            self.emit_queued(node_id, None);
            self.emit(self.outcome_for(
                node_id,
                &action,
                HubDispatchStatus::SkippedCooldown { remaining_ms },
                0,
                0,
                0,
            ));
            return Err(LightControlError::Timeout(format!(
                "Hub {} dispatch to '{}' is cooling down for {}ms after a timeout",
                self.key, key, remaining_ms
            )));
        }

        // A coalesce reuses the slot's eventual single outcome, so it emits a
        // queued event only when it changes which node the slot addresses.
        let queued_change = match queue.pending.get_mut(&key) {
            Some(job) => {
                let superseded = (job.node_id != node_id).then(|| job.node_id.clone());
                job.action = action;
                job.node_id = node_id.to_string();
                job.coalesced += 1;
                superseded.map(|old| (node_id.to_string(), Some(old)))
            }
            None => {
                queue.pending.insert(
                    key.clone(),
                    PendingJob {
                        node_id: node_id.to_string(),
                        action,
                        enqueued_at: now,
                        coalesced: 0,
                    },
                );
                queue.order.push_back(key);
                Some((node_id.to_string(), None))
            }
        };
        drop(queue);
        if let Some((queued_node, superseded)) = queued_change {
            self.emit_queued(&queued_node, superseded);
        }
        self.notify.notify_one();
        Ok(())
    }

    /// Pick the next worker action under the queue lock. Returns the step and
    /// any cooldown-drop outcomes to emit after the lock is released.
    fn select_next(&self) -> (WorkerStep, Vec<HubDispatchOutcome>) {
        let mut outcomes = Vec::new();
        let mut queue = match self.queue.lock() {
            Ok(queue) => queue,
            Err(_) => return (WorkerStep::Exit, outcomes),
        };
        if queue.closed {
            return (WorkerStep::Exit, outcomes);
        }

        let now = Instant::now();
        queue.expire_cooldowns(now);

        if queue.in_flight_count >= self.policy.max_in_flight {
            return (WorkerStep::Wait, outcomes);
        }

        if let Some(limit) = &self.policy.rate_limit {
            queue.refill_tokens(limit, now);
        }

        // Scan the FIFO for the first dispatchable key, dropping any whose
        // target started cooling down after they were queued.
        let mut index = 0;
        while index < queue.order.len() {
            let key = queue.order[index].clone();
            if let Some(remaining) = queue.active_cooldown(&key, now) {
                queue.order.remove(index);
                if let Some(job) = queue.pending.remove(&key) {
                    let queued_ms = now.saturating_duration_since(job.enqueued_at).as_millis();
                    outcomes.push(self.outcome_for(
                        &job.node_id,
                        &job.action,
                        HubDispatchStatus::SkippedCooldown {
                            remaining_ms: remaining.as_millis() as u64,
                        },
                        queued_ms as u64,
                        0,
                        job.coalesced,
                    ));
                }
                continue;
            }

            let conflicts = queue
                .pending
                .get(&key)
                .map(|job| {
                    job.action
                        .elements()
                        .iter()
                        .any(|element| queue.in_flight.contains(element))
                })
                .unwrap_or(false);
            if conflicts {
                index += 1;
                continue;
            }

            // Candidate found — check pacing before popping so newer
            // commands keep coalescing onto it while we wait for tokens.
            if let Some(limit) = &self.policy.rate_limit {
                let cost = queue
                    .pending
                    .get(&key)
                    .map(|job| job.action.cost(limit))
                    .unwrap_or(0.0);
                if queue.tokens < cost {
                    let deficit = cost - queue.tokens;
                    let wait_secs = deficit / limit.refill_per_sec.max(f64::EPSILON);
                    return (
                        WorkerStep::Sleep(Duration::from_secs_f64(wait_secs.max(0.001))),
                        outcomes,
                    );
                }
                queue.tokens -= cost;
            }

            queue.order.remove(index);
            let job = queue
                .pending
                .remove(&key)
                .expect("pending job for ordered key");
            for element in job.action.elements() {
                queue.in_flight.insert(element);
            }
            queue.in_flight_count += 1;
            return (WorkerStep::Dispatch(ReadyJob { key, job }), outcomes);
        }

        (WorkerStep::Wait, outcomes)
    }

    fn finish_dispatch(&self, action: &HubDispatchAction) {
        if let Ok(mut queue) = self.queue.lock() {
            for element in action.elements() {
                queue.in_flight.remove(&element);
            }
            queue.in_flight_count = queue.in_flight_count.saturating_sub(1);
        }
        self.notify.notify_one();
    }

    fn apply_timeout_cooldown(&self, key: &str) -> Option<Instant> {
        if let Ok(mut queue) = self.queue.lock() {
            let cooldown_until = Instant::now() + self.policy.timeout_cooldown;
            match self.policy.timeout_scope {
                HubDispatchTimeoutScope::Target => {
                    queue
                        .target_cooldowns
                        .insert(key.to_string(), cooldown_until);
                }
                HubDispatchTimeoutScope::Hub => {
                    queue.hub_cooldown_until = Some(cooldown_until);
                }
            }
            return Some(cooldown_until);
        }
        None
    }

    fn clear_timeout_cooldown(&self, key: &str, applied_until: Instant) {
        if let Ok(mut queue) = self.queue.lock() {
            match self.policy.timeout_scope {
                HubDispatchTimeoutScope::Target => {
                    if queue.target_cooldowns.get(key) == Some(&applied_until) {
                        queue.target_cooldowns.remove(key);
                    }
                }
                HubDispatchTimeoutScope::Hub => {
                    if queue.hub_cooldown_until == Some(applied_until) {
                        queue.hub_cooldown_until = None;
                    }
                }
            }
        }
        self.notify.notify_one();
    }

    /// Close the mailbox and drop pending commands, reporting each drop.
    fn close(&self, reason: &str) {
        let dropped = {
            let mut queue = match self.queue.lock() {
                Ok(queue) => queue,
                Err(_) => return,
            };
            if queue.closed {
                Vec::new()
            } else {
                queue.closed = true;
                queue.order.clear();
                queue.pending.drain().collect::<Vec<_>>()
            }
        };
        let now = Instant::now();
        for (_, job) in dropped {
            let queued_ms = now.saturating_duration_since(job.enqueued_at).as_millis() as u64;
            self.emit(self.outcome_for(
                &job.node_id,
                &job.action,
                HubDispatchStatus::Dropped {
                    reason: reason.to_string(),
                },
                queued_ms,
                0,
                job.coalesced,
            ));
        }
        self.notify.notify_one();
    }
}

async fn run_dispatch_worker(shared: Arc<DispatcherShared>) {
    loop {
        // Create the notified future before inspecting state so a
        // notification arriving between the check and the await is not lost.
        let notified = shared.notify.notified();
        let (step, outcomes) = shared.select_next();
        for outcome in outcomes {
            shared.emit(outcome);
        }
        match step {
            WorkerStep::Exit => break,
            WorkerStep::Wait => notified.await,
            WorkerStep::Sleep(duration) => {
                // Wake early if new work arrives (it may be cheaper).
                let _ = tokio::time::timeout(duration, notified).await;
            }
            WorkerStep::Dispatch(ready) => {
                let shared = shared.clone();
                tokio::spawn(supervise_dispatch(shared, ready));
            }
        }
    }
}

async fn supervise_dispatch(shared: Arc<DispatcherShared>, ready: ReadyJob) {
    let ReadyJob { key, job } = ready;
    let PendingJob {
        node_id,
        action,
        enqueued_at,
        coalesced,
    } = job;

    let started = Instant::now();
    let queued_ms = started.saturating_duration_since(enqueued_at).as_millis() as u64;

    let dispatch_controller = shared.controller.clone();
    let dispatch_action = action.clone();
    // Hub transports are blocking (blocking reqwest, unix sockets), so run
    // the controller call on the blocking pool exactly as the old per-hub
    // worker threads did — but supervised by an async timeout.
    let mut transport_call = tokio::task::spawn_blocking(move || {
        futures::executor::block_on(dispatch_action.dispatch(dispatch_controller))
    });

    let timeout = shared.policy.dispatch_timeout;
    match tokio::time::timeout(timeout, &mut transport_call).await {
        Ok(join_result) => {
            let dispatch_ms = started.elapsed().as_millis() as u64;
            let status = match join_result {
                Ok(Ok(receipt)) => match receipt.delivery {
                    HubCommandDelivery::Delivered => HubDispatchStatus::Succeeded,
                    HubCommandDelivery::Accepted {
                        command_ids,
                        controller_stream_id,
                    } => HubDispatchStatus::Accepted {
                        command_ids,
                        controller_stream_id,
                    },
                },
                Ok(Err(error)) => HubDispatchStatus::Failed {
                    error: error.to_string(),
                },
                Err(join_error) => HubDispatchStatus::Failed {
                    error: format!("dispatch task failed: {join_error}"),
                },
            };
            shared.emit(shared.outcome_for(
                &node_id,
                &action,
                status,
                queued_ms,
                dispatch_ms,
                coalesced,
            ));
            shared.finish_dispatch(&action);
        }
        Err(_) => {
            let cooldown_until = shared.apply_timeout_cooldown(&key);
            shared.emit(shared.outcome_for(
                &node_id,
                &action,
                HubDispatchStatus::TimedOut {
                    timeout_ms: timeout.as_millis() as u64,
                },
                queued_ms,
                started.elapsed().as_millis() as u64,
                coalesced,
            ));
            // Wait for the zombie transport call so per-target ordering and
            // the in-flight budget stay honest. The wait is bounded by the
            // transport's own timeouts.
            let late = transport_call.await;
            let late_succeeded = matches!(&late, Ok(Ok(_)));
            if late_succeeded {
                if let Some(cooldown_until) = cooldown_until {
                    shared.clear_timeout_cooldown(&key, cooldown_until);
                }
            }
            tracing::warn!(
                target: "cmd",
                event = "hub_dispatch_late_completion",
                hub = %shared.key,
                node_id = %node_id,
                dispatch_target = %key,
                total_ms = started.elapsed().as_millis() as u64,
                result_ok = late_succeeded,
                "Timed-out hub dispatch eventually completed"
            );
            shared.finish_dispatch(&action);
        }
    }
}

struct HubDispatcher {
    shared: Arc<DispatcherShared>,
}

impl HubDispatcher {
    fn new(
        key: &str,
        controller: Arc<dyn HubLightController>,
        policy: HubDispatchPolicy,
        listener: SharedOutcomeListener,
        queued_listener: SharedQueuedListener,
    ) -> Self {
        let shared = Arc::new(DispatcherShared {
            key: key.to_string(),
            hub_type: hub_type_from_key(key).to_string(),
            controller,
            policy: policy.clone(),
            queue: Mutex::new(DispatchQueue::new(policy.rate_limit.as_ref())),
            notify: Notify::new(),
            listener,
            queued_listener,
        });
        dispatch_runtime().spawn(run_dispatch_worker(shared.clone()));
        Self { shared }
    }

    fn metadata(&self) -> HubDispatchMetadata {
        HubDispatchMetadata {
            hub_key: self.shared.key.clone(),
            hub_type: self.shared.hub_type.clone(),
            policy: self.shared.policy.clone(),
        }
    }

    fn controller(&self) -> Arc<dyn HubLightController> {
        self.shared.controller.clone()
    }

    fn policy(&self) -> &HubDispatchPolicy {
        &self.shared.policy
    }

    /// Enqueue a command, splitting device-list targets when the policy
    /// isolates devices from each other (Matter).
    fn enqueue_action(&self, node_id: &str, action: HubDispatchAction) -> LightControlResult<()> {
        let split_ids = match (&self.shared.policy.split_device_targets, action.target()) {
            (true, HubDispatchTarget::Devices { native_ids }) if native_ids.len() > 1 => {
                Some(native_ids.clone())
            }
            _ => None,
        };

        match split_ids {
            None => self.shared.enqueue(node_id, action),
            Some(native_ids) => {
                let mut last_error = None;
                let mut any_ok = false;
                for native_id in native_ids {
                    let single_target = HubDispatchTarget::Devices {
                        native_ids: vec![native_id],
                    };
                    let single_action = match &action {
                        HubDispatchAction::TurnOn { command, .. } => HubDispatchAction::TurnOn {
                            target: single_target,
                            command: command.clone(),
                        },
                        HubDispatchAction::TurnOff { transition_ms, .. } => {
                            HubDispatchAction::TurnOff {
                                target: single_target,
                                transition_ms: *transition_ms,
                            }
                        }
                    };
                    match self.shared.enqueue(node_id, single_action) {
                        Ok(()) => any_ok = true,
                        Err(error) => last_error = Some(error),
                    }
                }
                if any_ok {
                    Ok(())
                } else {
                    Err(last_error.unwrap_or_else(|| {
                        LightControlError::CommandFailed(format!(
                            "Hub {} rejected all device dispatches",
                            self.shared.key
                        ))
                    }))
                }
            }
        }
    }

    fn close(&self, reason: &str) {
        self.shared.close(reason);
    }
}

impl Drop for HubDispatcher {
    fn drop(&mut self) {
        self.shared.close("hub dispatcher dropped");
    }
}

// ── Composite controller ─────────────────────────────────────────────────

/// A composite light controller that fans out commands to per-hub controllers.
///
/// Interior mutability via [`RwLock`] allows adding/removing controllers
/// and updating the routing table while the engine is running.
pub struct CompositeController {
    /// Per-hub dispatchers keyed by hub identifier (e.g., "hue@192.168.1.5").
    controllers: RwLock<HashMap<String, Arc<HubDispatcher>>>,
    /// Room routing: topology_room_id → list of (hub_key, dispatch target) pairs.
    routing: RwLock<HashMap<String, Vec<(String, HubDispatchTarget)>>>,
    /// Human-readable node labels for logs keyed by public/synthetic node ID.
    node_labels: RwLock<HashMap<String, String>>,
    /// Listener receiving every dispatch outcome.
    outcome_listener: SharedOutcomeListener,
    /// Listener receiving every mailbox-queued event.
    queued_listener: SharedQueuedListener,
}

impl CompositeController {
    /// Create an empty composite controller (no hubs registered).
    pub fn new() -> Self {
        Self {
            controllers: RwLock::new(HashMap::new()),
            routing: RwLock::new(HashMap::new()),
            node_labels: RwLock::new(HashMap::new()),
            outcome_listener: Arc::new(RwLock::new(None)),
            queued_listener: Arc::new(RwLock::new(None)),
        }
    }

    /// Install the listener that receives every [`HubDispatchOutcome`].
    ///
    /// Failures never propagate to command callers (dispatch is
    /// fire-and-forget), so this is the channel through which they surface.
    pub fn set_outcome_listener(&self, listener: HubDispatchOutcomeListener) {
        if let Ok(mut slot) = self.outcome_listener.write() {
            *slot = Some(listener);
        }
    }

    /// Install the listener that receives every [`HubDispatchQueued`] event.
    ///
    /// Paired 1:1 with outcomes, this lets callers track how many commands a
    /// node still has physically unresolved (e.g. a user-visible pending flag
    /// that holds until the lights actually changed).
    pub fn set_queued_listener(&self, listener: HubDispatchQueuedListener) {
        if let Ok(mut slot) = self.queued_listener.write() {
            *slot = Some(listener);
        }
    }

    /// Register a per-hub controller. Replaces any existing controller for this key.
    pub fn register_controller(&self, key: &str, controller: Arc<dyn HubLightController>) {
        self.register_controller_with_policy(key, controller, HubDispatchPolicy::for_hub_key(key));
    }

    /// Register a per-hub controller with an explicit dispatch policy.
    pub fn register_controller_with_policy(
        &self,
        key: &str,
        controller: Arc<dyn HubLightController>,
        policy: HubDispatchPolicy,
    ) {
        let dispatcher = Arc::new(HubDispatcher::new(
            key,
            controller,
            policy,
            self.outcome_listener.clone(),
            self.queued_listener.clone(),
        ));
        let replaced = self
            .controllers
            .write()
            .ok()
            .and_then(|mut controllers| controllers.insert(key.to_string(), dispatcher));
        if let Some(replaced) = replaced {
            replaced.close("hub controller replaced");
        }
    }

    /// Remove a per-hub controller.
    pub fn remove_controller(&self, key: &str) {
        let removed = self
            .controllers
            .write()
            .ok()
            .and_then(|mut controllers| controllers.remove(key));
        if let Some(removed) = removed {
            removed.close("hub controller removed");
        }
    }

    /// Replace the full routing table.
    ///
    /// Each entry maps a topology room ID to a list of (hub_key, target)
    /// pairs. For single-hub rooms, the Vec has one entry. For cross-hub rooms,
    /// it has multiple. The composite calls each hub's controller with its
    /// typed hub-native dispatch target.
    pub fn update_routing(&self, table: HashMap<String, Vec<(String, HubDispatchTarget)>>) {
        if let Ok(mut routing) = self.routing.write() {
            *routing = table;
        }
    }

    /// Replace the node-label lookup table used for logs.
    pub fn update_node_labels(&self, labels: HashMap<String, String>) {
        if let Ok(mut node_labels) = self.node_labels.write() {
            *node_labels = labels;
        }
    }

    /// How many hub controllers are registered.
    pub fn controller_count(&self) -> usize {
        self.controllers.read().map(|c| c.len()).unwrap_or(0)
    }

    /// Metadata for registered hub command queues.
    pub fn hub_dispatch_metadata(&self) -> Vec<HubDispatchMetadata> {
        let mut metadata: Vec<_> = self
            .controllers
            .read()
            .map(|controllers| {
                controllers
                    .values()
                    .map(|dispatcher| dispatcher.metadata())
                    .collect()
            })
            .unwrap_or_default();
        metadata.sort_by(|a, b| a.hub_key.cmp(&b.hub_key));
        metadata
    }

    /// Get controller targets for a room from the routing table.
    ///
    /// Returns (hub_key, dispatcher, target) triples.
    fn controllers_for_room(
        &self,
        room_id: &str,
    ) -> Vec<(String, Arc<HubDispatcher>, HubDispatchTarget)> {
        let targets = self
            .routing
            .read()
            .ok()
            .and_then(|r| r.get(room_id).cloned());

        let controllers = match self.controllers.read() {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };

        if let Some(targets) = targets {
            targets
                .iter()
                .filter_map(|(hub_key, target)| {
                    controllers
                        .get(hub_key)
                        .map(|dispatcher| (hub_key.clone(), dispatcher.clone(), target.clone()))
                })
                .collect()
        } else {
            let node_label = self.node_log_label(room_id);
            warn!(
                target: "composite",
                "No routing entry for node {}",
                node_label
            );
            Vec::new()
        }
    }

    fn node_log_label(&self, node_id: &str) -> String {
        let node_name = self
            .node_labels
            .read()
            .ok()
            .and_then(|labels| labels.get(node_id).cloned());
        format_node_log_label(node_id, node_name.as_deref())
    }

    fn enqueue_for_room(
        &self,
        room_id: &str,
        make_action: impl Fn(HubDispatchTarget) -> HubDispatchAction,
        kind: HubDispatchKind,
    ) -> LightControlResult<()> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            let node_label = self.node_log_label(room_id);
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for node {}",
                node_label
            )));
        }

        let node_label = self.node_log_label(room_id);
        let target_count = targets.len();
        let mut any_ok = false;
        let mut last_error = None;
        for (key, dispatcher, target) in targets {
            match dispatcher.enqueue_action(room_id, make_action(target)) {
                Ok(()) => any_ok = true,
                Err(e) => {
                    warn!(target: "composite", "hub {} rejected {}: {}", key, kind.as_str(), e);
                    last_error = Some(e);
                }
            }
        }

        if any_ok {
            tracing::debug!(
                target: "cmd",
                event = "dispatch_enqueued",
                node_id = %room_id,
                node = %node_label,
                kind = kind.as_str(),
                target_count,
                "Composite dispatch enqueued"
            );
            Ok(())
        } else {
            tracing::warn!(
                target: "cmd",
                event = "dispatch_rejected",
                node_id = %room_id,
                node = %node_label,
                kind = kind.as_str(),
                target_count,
                "Composite dispatch rejected by all hubs"
            );
            Err(last_error.unwrap_or_else(|| {
                LightControlError::CommandFailed(format!(
                    "All controllers rejected {} for node {}",
                    kind.as_str(),
                    node_label
                ))
            }))
        }
    }

    /// Flash a concrete hub target for physical identification.
    ///
    /// This bypasses the runtime state machine intentionally: identification
    /// flashes should not change Rhythm's persisted room/node state.
    pub async fn flash_target(
        &self,
        hub_key: &str,
        target: HubDispatchTarget,
    ) -> LightControlResult<()> {
        let controller = self
            .controllers
            .read()
            .ok()
            .and_then(|controllers| controllers.get(hub_key).cloned())
            .map(|dispatcher| dispatcher.controller())
            .ok_or_else(|| {
                LightControlError::RoomNotFound(format!(
                    "No controller registered for hub {}",
                    hub_key
                ))
            })?;

        let what = format!("Hub {} flash '{}'", hub_key, target.label());
        await_supervised(spawn_supervised(FLASH_TIMEOUT, what, move || {
            futures::executor::block_on(controller.flash_target(&target))
        }))
        .await
    }
}

impl Default for CompositeController {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl LightController for CompositeController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        self.enqueue_for_room(
            room_id,
            |target| HubDispatchAction::TurnOn {
                target,
                command: command.clone(),
            },
            HubDispatchKind::TurnOn,
        )
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        self.enqueue_for_room(
            room_id,
            |target| HubDispatchAction::TurnOff {
                target,
                transition_ms,
            },
            HubDispatchKind::TurnOff,
        )
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        let all_controllers: Vec<(String, Arc<dyn HubLightController>)> = self
            .controllers
            .read()
            .map(|c| {
                c.iter()
                    .map(|(key, dispatcher)| (key.clone(), dispatcher.controller()))
                    .collect()
            })
            .unwrap_or_default();

        let handles: Vec<_> = all_controllers
            .into_iter()
            .map(|(key, controller)| {
                let what = format!("Hub {} get_rooms", key);
                spawn_supervised(GET_ROOMS_TIMEOUT, what, move || {
                    futures::executor::block_on(controller.get_rooms())
                })
            })
            .collect();

        let mut rooms = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for handle in handles {
            if let Ok(hub_rooms) = await_supervised(handle).await {
                for room in hub_rooms {
                    if seen.insert(room.id.clone()) {
                        rooms.push(room);
                    }
                }
            }
        }
        Ok(rooms)
    }

    async fn is_connected(&self) -> bool {
        let all_controllers: Vec<(String, Arc<dyn HubLightController>)> = self
            .controllers
            .read()
            .map(|c| {
                c.iter()
                    .map(|(key, dispatcher)| (key.clone(), dispatcher.controller()))
                    .collect()
            })
            .unwrap_or_default();

        let handles: Vec<_> = all_controllers
            .into_iter()
            .map(|(key, controller)| {
                let what = format!("Hub {} is_connected", key);
                spawn_supervised(IS_CONNECTED_TIMEOUT, what, move || {
                    Ok(futures::executor::block_on(controller.is_connected()))
                })
            })
            .collect();

        for handle in handles {
            if let Ok(true) = await_supervised(handle).await {
                return true;
            }
        }
        false
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            let node_label = self.node_log_label(room_id);
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for node {}",
                node_label
            )));
        }

        let handles: Vec<_> = targets
            .into_iter()
            .map(|(key, dispatcher, target)| {
                let controller = dispatcher.controller();
                let query_timeout = dispatcher.policy().query_timeout;
                let what = format!("Hub {} any_lights_on '{}'", key, target.label());
                let label = target.label();
                let handle = spawn_supervised(query_timeout, what, move || {
                    futures::executor::block_on(controller.any_lights_on_target(&target))
                });
                (key, label, handle)
            })
            .collect();

        for (key, label, handle) in handles {
            match await_supervised(handle).await {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        target: "composite",
                        "any_lights_on '{}' via {}: {}",
                        label,
                        key,
                        e
                    );
                }
            }
        }
        Ok(false)
    }

    fn name(&self) -> &str {
        "Composite"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::mpsc;
    use std::sync::{Condvar, Mutex};

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        futures::executor::block_on(f)
    }

    fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if predicate() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        predicate()
    }

    fn group_target(id: &str) -> HubDispatchTarget {
        HubDispatchTarget::Group {
            room_id: id.to_string(),
            control_id: id.to_string(),
        }
    }

    fn route(
        entries: &[(&str, &str, HubDispatchTarget)],
    ) -> HashMap<String, Vec<(String, HubDispatchTarget)>> {
        let mut table: HashMap<String, Vec<(String, HubDispatchTarget)>> = HashMap::new();
        for (room, hub, target) in entries {
            table
                .entry(room.to_string())
                .or_default()
                .push((hub.to_string(), target.clone()));
        }
        table
    }

    /// Collects dispatch outcomes for assertions.
    struct OutcomeCollector {
        tx: Mutex<mpsc::Sender<HubDispatchOutcome>>,
        rx: Mutex<mpsc::Receiver<HubDispatchOutcome>>,
    }

    impl OutcomeCollector {
        fn install(composite: &CompositeController) -> Arc<Self> {
            let (tx, rx) = mpsc::channel();
            let collector = Arc::new(Self {
                tx: Mutex::new(tx),
                rx: Mutex::new(rx),
            });
            let sink = collector.clone();
            composite.set_outcome_listener(Arc::new(move |outcome| {
                let _ = sink.tx.lock().unwrap().send(outcome);
            }));
            collector
        }

        fn wait_for(
            &self,
            timeout: Duration,
            predicate: impl Fn(&HubDispatchOutcome) -> bool,
        ) -> Option<HubDispatchOutcome> {
            let deadline = Instant::now() + timeout;
            loop {
                let remaining = deadline.saturating_duration_since(Instant::now());
                if remaining.is_zero() {
                    return None;
                }
                match self.rx.lock().unwrap().recv_timeout(remaining) {
                    Ok(outcome) if predicate(&outcome) => return Some(outcome),
                    Ok(_) => continue,
                    Err(_) => return None,
                }
            }
        }
    }

    /// Mock controller that records calls and can be configured to fail.
    struct MockController {
        name: String,
        should_fail: AtomicBool,
        turn_on_calls: Mutex<Vec<(String, LightingCommand)>>,
        turn_off_calls: Mutex<Vec<(String, Option<u32>)>>,
        lights_on: AtomicBool,
        rooms: Mutex<Vec<Room>>,
        accepted_receipt: Mutex<Option<HubCommandReceipt>>,
    }

    impl MockController {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: AtomicBool::new(false),
                turn_on_calls: Mutex::new(Vec::new()),
                turn_off_calls: Mutex::new(Vec::new()),
                lights_on: AtomicBool::new(false),
                rooms: Mutex::new(Vec::new()),
                accepted_receipt: Mutex::new(None),
            }
        }

        fn with_rooms(name: &str, rooms: Vec<Room>) -> Self {
            let c = Self::new(name);
            *c.rooms.lock().unwrap() = rooms;
            c
        }

        fn set_fail(&self, fail: bool) {
            self.should_fail.store(fail, Ordering::Relaxed);
        }

        fn set_lights_on(&self, on: bool) {
            self.lights_on.store(on, Ordering::Relaxed);
        }

        fn set_accepted_receipt(&self, command_ids: Vec<u64>, stream_id: &str) {
            *self.accepted_receipt.lock().unwrap() = Some(HubCommandReceipt::accepted(
                command_ids,
                stream_id.to_string(),
            ));
        }

        fn turn_on_count(&self) -> usize {
            self.turn_on_calls.lock().unwrap().len()
        }

        fn turn_off_count(&self) -> usize {
            self.turn_off_calls.lock().unwrap().len()
        }

        fn last_turn_on(&self) -> Option<(String, LightingCommand)> {
            self.turn_on_calls.lock().unwrap().last().cloned()
        }
    }

    #[async_trait]
    impl HubLightController for MockController {
        async fn turn_on_target(
            &self,
            target: &HubDispatchTarget,
            command: LightingCommand,
        ) -> LightControlResult<()> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            self.turn_on_calls
                .lock()
                .unwrap()
                .push((target.label(), command));
            Ok(())
        }

        async fn turn_off_target(
            &self,
            target: &HubDispatchTarget,
            transition_ms: Option<u32>,
        ) -> LightControlResult<()> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            self.turn_off_calls
                .lock()
                .unwrap()
                .push((target.label(), transition_ms));
            Ok(())
        }

        async fn turn_on_target_with_receipt(
            &self,
            target: &HubDispatchTarget,
            command: LightingCommand,
        ) -> LightControlResult<HubCommandReceipt> {
            self.turn_on_target(target, command).await?;
            Ok(self
                .accepted_receipt
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(HubCommandReceipt::delivered))
        }

        async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
            Ok(self.rooms.lock().unwrap().clone())
        }

        async fn is_connected(&self) -> bool {
            !self.should_fail.load(Ordering::Relaxed)
        }

        async fn any_lights_on_target(
            &self,
            _target: &HubDispatchTarget,
        ) -> LightControlResult<bool> {
            if self.should_fail.load(Ordering::Relaxed) {
                return Err(LightControlError::CommandFailed("mock failure".into()));
            }
            Ok(self.lights_on.load(Ordering::Relaxed))
        }

        fn name(&self) -> &str {
            &self.name
        }
    }

    /// Controller whose turn_on blocks until released.
    struct BlockingController {
        turn_on_calls: AtomicUsize,
        started: (Mutex<bool>, Condvar),
        release: (Mutex<bool>, Condvar),
        block_queries: bool,
    }

    impl BlockingController {
        fn new() -> Self {
            Self {
                turn_on_calls: AtomicUsize::new(0),
                started: (Mutex::new(false), Condvar::new()),
                release: (Mutex::new(false), Condvar::new()),
                block_queries: false,
            }
        }

        fn blocking_queries() -> Self {
            Self {
                block_queries: true,
                ..Self::new()
            }
        }

        fn wait_started_timeout(&self, timeout: Duration) -> bool {
            let (lock, cvar) = &self.started;
            let started = lock.lock().unwrap();
            let (started, _) = cvar
                .wait_timeout_while(started, timeout, |started| !*started)
                .unwrap();
            *started
        }

        fn clear_started(&self) {
            let (lock, _) = &self.started;
            *lock.lock().unwrap() = false;
        }

        fn release(&self) {
            let (lock, cvar) = &self.release;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }

        fn hold(&self) {
            {
                let (lock, cvar) = &self.started;
                *lock.lock().unwrap() = true;
                cvar.notify_all();
            }
            let (lock, cvar) = &self.release;
            let mut release = lock.lock().unwrap();
            while !*release {
                release = cvar.wait(release).unwrap();
            }
        }

        fn turn_on_count(&self) -> usize {
            self.turn_on_calls.load(Ordering::SeqCst)
        }
    }

    #[async_trait]
    impl HubLightController for BlockingController {
        async fn turn_on_target(
            &self,
            _target: &HubDispatchTarget,
            _command: LightingCommand,
        ) -> LightControlResult<()> {
            self.turn_on_calls.fetch_add(1, Ordering::SeqCst);
            self.hold();
            Ok(())
        }

        async fn turn_off_target(
            &self,
            _target: &HubDispatchTarget,
            _transition_ms: Option<u32>,
        ) -> LightControlResult<()> {
            Ok(())
        }

        async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
            Ok(Vec::new())
        }

        async fn is_connected(&self) -> bool {
            true
        }

        async fn any_lights_on_target(
            &self,
            _target: &HubDispatchTarget,
        ) -> LightControlResult<bool> {
            if self.block_queries {
                self.hold();
            }
            Ok(false)
        }

        fn name(&self) -> &str {
            "blocking"
        }
    }

    /// Controller that blocks only for one target label.
    struct SelectiveBlockingController {
        blocked_label: String,
        turn_on_calls: Mutex<Vec<String>>,
        blocked_started: (Mutex<bool>, Condvar),
        release: (Mutex<bool>, Condvar),
    }

    impl SelectiveBlockingController {
        fn new(blocked_label: &str) -> Self {
            Self {
                blocked_label: blocked_label.to_string(),
                turn_on_calls: Mutex::new(Vec::new()),
                blocked_started: (Mutex::new(false), Condvar::new()),
                release: (Mutex::new(false), Condvar::new()),
            }
        }

        fn wait_blocked_started_timeout(&self, timeout: Duration) -> bool {
            let (lock, cvar) = &self.blocked_started;
            let started = lock.lock().unwrap();
            let (started, _) = cvar
                .wait_timeout_while(started, timeout, |started| !*started)
                .unwrap();
            *started
        }

        fn release(&self) {
            let (lock, cvar) = &self.release;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }

        fn turn_on_count_for(&self, label: &str) -> usize {
            self.turn_on_calls
                .lock()
                .unwrap()
                .iter()
                .filter(|seen| seen.as_str() == label)
                .count()
        }
    }

    #[async_trait]
    impl HubLightController for SelectiveBlockingController {
        async fn turn_on_target(
            &self,
            target: &HubDispatchTarget,
            _command: LightingCommand,
        ) -> LightControlResult<()> {
            let target_label = target.label();
            self.turn_on_calls
                .lock()
                .unwrap()
                .push(target_label.clone());

            if target_label == self.blocked_label {
                {
                    let (lock, cvar) = &self.blocked_started;
                    *lock.lock().unwrap() = true;
                    cvar.notify_all();
                }
                let (lock, cvar) = &self.release;
                let mut release = lock.lock().unwrap();
                while !*release {
                    release = cvar.wait(release).unwrap();
                }
            }

            Ok(())
        }

        async fn turn_off_target(
            &self,
            _target: &HubDispatchTarget,
            _transition_ms: Option<u32>,
        ) -> LightControlResult<()> {
            Ok(())
        }

        async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
            Ok(Vec::new())
        }

        async fn is_connected(&self) -> bool {
            true
        }

        async fn any_lights_on_target(
            &self,
            _target: &HubDispatchTarget,
        ) -> LightControlResult<bool> {
            Ok(false)
        }

        fn name(&self) -> &str {
            "selective-blocking"
        }
    }

    fn fast_timeout_policy() -> HubDispatchPolicy {
        HubDispatchPolicy {
            max_in_flight: 2,
            rate_limit: None,
            split_device_targets: false,
            dispatch_timeout: Duration::from_millis(75),
            timeout_cooldown: Duration::from_millis(500),
            timeout_scope: HubDispatchTimeoutScope::Target,
            query_timeout: Duration::from_millis(200),
        }
    }

    // ── Registration ─────────────────────────────────────────────────

    #[test]
    fn empty_composite_has_no_controllers() {
        let composite = CompositeController::new();
        assert_eq!(composite.controller_count(), 0);
    }

    #[test]
    fn register_and_count() {
        let composite = CompositeController::new();
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        assert_eq!(composite.controller_count(), 1);
        composite.register_controller("hub_b", Arc::new(MockController::new("b")));
        assert_eq!(composite.controller_count(), 2);
    }

    #[test]
    fn hub_dispatch_metadata_exposes_policy_by_hub_type() {
        let composite = CompositeController::new();
        composite.register_controller("hue@192.168.1.5", Arc::new(MockController::new("hue")));
        composite.register_controller("matter@local", Arc::new(MockController::new("matter")));
        composite.register_controller(
            "homeassistant@ha.local",
            Arc::new(MockController::new("ha")),
        );

        let metadata = composite.hub_dispatch_metadata();
        let hue = metadata
            .iter()
            .find(|entry| entry.hub_key == "hue@192.168.1.5")
            .unwrap();
        let matter = metadata
            .iter()
            .find(|entry| entry.hub_key == "matter@local")
            .unwrap();
        let home_assistant = metadata
            .iter()
            .find(|entry| entry.hub_key == "homeassistant@ha.local")
            .unwrap();

        assert!(hue.policy.rate_limit.is_some());
        assert!(!hue.policy.split_device_targets);
        assert_eq!(hue.policy.timeout_scope, HubDispatchTimeoutScope::Target);
        assert!(matter.policy.rate_limit.is_none());
        assert!(matter.policy.split_device_targets);
        assert_eq!(matter.policy.max_in_flight, MATTER_MAX_IN_FLIGHT);
        assert_eq!(
            matter.policy.timeout_cooldown,
            DEFAULT_MATTER_TIMEOUT_COOLDOWN
        );
        assert!(home_assistant.policy.rate_limit.is_none());
        assert!(!home_assistant.policy.split_device_targets);
    }

    #[test]
    fn register_controller_with_policy_uses_explicit_policy() {
        let composite = CompositeController::new();
        let policy = HubDispatchPolicy {
            max_in_flight: 7,
            rate_limit: Some(HubRateLimit {
                capacity: 3.0,
                refill_per_sec: 5.0,
                group_cost: 3.0,
                device_cost: 1.0,
            }),
            split_device_targets: true,
            dispatch_timeout: Duration::from_secs(2),
            timeout_cooldown: Duration::from_secs(5),
            timeout_scope: HubDispatchTimeoutScope::Hub,
            query_timeout: Duration::from_secs(1),
        };
        composite.register_controller_with_policy(
            "custom@hub",
            Arc::new(MockController::new("custom")),
            policy.clone(),
        );

        assert_eq!(composite.hub_dispatch_metadata()[0].policy, policy);
    }

    #[test]
    fn remove_controller() {
        let composite = CompositeController::new();
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        composite.remove_controller("hub_a");
        assert_eq!(composite.controller_count(), 0);
    }

    // ── Basic dispatch ───────────────────────────────────────────────

    #[test]
    fn single_controller_turn_on_dispatches_async() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        let cmd = LightingCommand::new(80, 4000);
        block_on(composite.turn_on("room1", cmd)).unwrap();
        assert!(wait_until(Duration::from_secs(5), || mock.turn_on_count() == 1));
    }

    #[test]
    fn single_controller_turn_off_dispatches_async() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        block_on(composite.turn_off("room1", Some(400))).unwrap();
        assert!(wait_until(Duration::from_secs(5), || mock.turn_off_count() == 1));
    }

    #[test]
    fn cross_hub_turn_on_fans_out() {
        let hub_a = Arc::new(MockController::new("a"));
        let hub_b = Arc::new(MockController::new("b"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", hub_a.clone());
        composite.register_controller("hub_b", hub_b.clone());
        composite.update_routing(route(&[
            ("hall", "hub_a", group_target("hall-a")),
            ("hall", "hub_b", group_target("hall-b")),
        ]));

        block_on(composite.turn_on("hall", LightingCommand::new(70, 3500))).unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            hub_a.turn_on_count() == 1 && hub_b.turn_on_count() == 1
        }));
    }

    #[test]
    fn unknown_room_returns_room_not_found() {
        let composite = CompositeController::new();
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        composite.update_routing(HashMap::new());

        let result = block_on(composite.turn_on("unknown", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::RoomNotFound(_))));
    }

    // ── Non-blocking guarantees ──────────────────────────────────────

    #[test]
    fn turn_on_returns_immediately_while_hub_is_stuck() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "stuck@hub",
            blocking.clone(),
            fast_timeout_policy(),
        );
        composite.update_routing(route(&[("room1", "stuck@hub", group_target("room1"))]));

        let started = Instant::now();
        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_millis(250),
            "enqueue took {:?}, expected immediate return",
            elapsed
        );

        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));
        blocking.release();
    }

    #[test]
    fn stuck_hub_does_not_block_other_hub() {
        let blocking = Arc::new(BlockingController::new());
        let fast = Arc::new(MockController::new("fast"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "stuck@hub",
            blocking.clone(),
            fast_timeout_policy(),
        );
        composite.register_controller("fast@hub", fast.clone());
        composite.update_routing(route(&[
            ("blocked", "stuck@hub", group_target("blocked")),
            ("other", "fast@hub", group_target("other")),
        ]));

        block_on(composite.turn_on("blocked", LightingCommand::new(80, 4000))).unwrap();
        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));

        block_on(composite.turn_on("other", LightingCommand::new(70, 3500))).unwrap();
        assert!(wait_until(Duration::from_secs(5), || fast.turn_on_count() == 1));
        blocking.release();
    }

    #[test]
    fn stuck_target_does_not_block_other_targets_on_same_hub() {
        let selective = Arc::new(SelectiveBlockingController::new("dev-1"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "matter@local",
            selective.clone(),
            HubDispatchPolicy {
                split_device_targets: true,
                max_in_flight: 4,
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[(
            "room1",
            "matter@local",
            HubDispatchTarget::Devices {
                native_ids: vec!["dev-1".to_string(), "dev-2".to_string()],
            },
        )]));

        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();

        assert!(selective.wait_blocked_started_timeout(Duration::from_secs(5)));
        assert!(wait_until(Duration::from_secs(5), || {
            selective.turn_on_count_for("dev-2") == 1
        }));
        selective.release();
    }

    // ── Coalescing & ordering ────────────────────────────────────────

    #[test]
    fn commands_coalesce_while_target_is_busy() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                dispatch_timeout: Duration::from_secs(10),
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        // First command occupies the target.
        block_on(composite.turn_on("room1", LightingCommand::new(10, 2000))).unwrap();
        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));
        blocking.clear_started();

        // These should collapse into one trailing dispatch.
        for brightness in [20, 30, 40, 50] {
            block_on(composite.turn_on("room1", LightingCommand::new(brightness, 2000))).unwrap();
        }

        blocking.release();
        assert!(wait_until(Duration::from_secs(5), || {
            blocking.turn_on_count() == 2
        }));
        // No further dispatches arrive.
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(blocking.turn_on_count(), 2);

        let coalesced = outcomes
            .wait_for(Duration::from_secs(5), |o| o.coalesced == 3)
            .expect("trailing dispatch outcome should record 3 superseded commands");
        assert!(coalesced.status.is_success());
    }

    #[test]
    fn queued_events_pair_one_to_one_with_outcomes() {
        let composite = CompositeController::new();
        let outcomes = OutcomeCollector::install(&composite);
        let queued_events = Arc::new(Mutex::new(Vec::<HubDispatchQueued>::new()));
        {
            let sink = queued_events.clone();
            composite.set_queued_listener(Arc::new(move |event| {
                sink.lock().unwrap().push(event);
            }));
        }
        composite.register_controller("hub_a", Arc::new(MockController::new("a")));
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        block_on(composite.turn_on("room1", LightingCommand::new(50, 3000))).unwrap();

        let outcome = outcomes
            .wait_for(Duration::from_secs(5), |o| o.node_id == "room1")
            .expect("dispatch outcome");
        assert!(outcome.status.is_success());

        let queued = queued_events.lock().unwrap().clone();
        assert_eq!(queued.len(), 1, "one enqueue must emit one queued event");
        assert_eq!(queued[0].node_id, "room1");
        assert_eq!(queued[0].superseded_node_id, None);
        assert_eq!(queued[0].hub_key, "hub_a");
    }

    #[test]
    fn coalesce_reports_superseded_node_and_keeps_pairing_balanced() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        let queued_events = Arc::new(Mutex::new(Vec::<HubDispatchQueued>::new()));
        {
            let sink = queued_events.clone();
            composite.set_queued_listener(Arc::new(move |event| {
                sink.lock().unwrap().push(event);
            }));
        }
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                dispatch_timeout: Duration::from_secs(10),
                ..fast_timeout_policy()
            },
        );
        // Two nodes share one hub-native target, so their commands coalesce
        // into the same mailbox slot.
        composite.update_routing(route(&[
            ("room1", "hub_a", group_target("shared")),
            ("room2", "hub_a", group_target("shared")),
        ]));

        // Occupy the target, then queue room1 and coalesce room2 over it.
        block_on(composite.turn_on("room1", LightingCommand::new(10, 2000))).unwrap();
        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));
        block_on(composite.turn_on("room1", LightingCommand::new(20, 2000))).unwrap();
        block_on(composite.turn_on("room2", LightingCommand::new(30, 2000))).unwrap();
        blocking.release();

        // Consume in arrival order — wait_for drops non-matching messages.
        let first = outcomes
            .wait_for(Duration::from_secs(5), |o| o.node_id == "room1")
            .expect("in-flight dispatch outcome");
        let trailing = outcomes
            .wait_for(Duration::from_secs(5), |o| o.node_id == "room2")
            .expect("coalesced dispatch outcome");
        assert!(trailing.status.is_success());

        let queued = queued_events.lock().unwrap().clone();
        assert_eq!(queued.len(), 3);
        assert_eq!(queued[0].superseded_node_id, None);
        assert_eq!(queued[1].superseded_node_id, None);
        assert_eq!(
            queued[2].superseded_node_id.as_deref(),
            Some("room1"),
            "a coalesce that changes the addressed node must report the displaced one"
        );

        // Per-node accounting balances: marks from queued events equal clears
        // from outcomes plus superseded rebalances.
        let mut counts = std::collections::HashMap::<String, i64>::new();
        for event in &queued {
            *counts.entry(event.node_id.clone()).or_default() += 1;
            if let Some(old) = &event.superseded_node_id {
                *counts.entry(old.clone()).or_default() -= 1;
            }
        }
        // Two outcomes total: the in-flight room1 dispatch and the trailing
        // coalesced room2 dispatch.
        for outcome in [&first, &trailing] {
            *counts.entry(outcome.node_id.clone()).or_default() -= 1;
        }
        assert!(
            counts.values().all(|count| *count == 0),
            "queued/outcome pairing must balance per node: {counts:?}"
        );
    }

    #[test]
    fn same_target_never_has_two_dispatches_in_flight() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                dispatch_timeout: Duration::from_secs(10),
                max_in_flight: 4,
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        block_on(composite.turn_on("room1", LightingCommand::new(10, 2000))).unwrap();
        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));

        block_on(composite.turn_on("room1", LightingCommand::new(90, 2000))).unwrap();
        std::thread::sleep(Duration::from_millis(150));
        assert_eq!(
            blocking.turn_on_count(),
            1,
            "second dispatch must wait for the first to finish"
        );

        blocking.release();
        assert!(wait_until(Duration::from_secs(5), || {
            blocking.turn_on_count() == 2
        }));
    }

    // ── Timeouts, cooldowns, outcomes ────────────────────────────────

    #[test]
    fn timeout_emits_outcome_and_applies_cooldown() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller_with_policy("hub_a", blocking.clone(), fast_timeout_policy());
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();

        let timed_out = outcomes
            .wait_for(Duration::from_secs(5), |o| {
                matches!(o.status, HubDispatchStatus::TimedOut { .. })
            })
            .expect("timeout outcome");
        assert_eq!(timed_out.hub_key, "hub_a");
        assert_eq!(timed_out.node_id, "room1");
        assert_eq!(timed_out.kind, HubDispatchKind::TurnOn);

        // Target now cooling down: enqueue rejected with an outcome.
        let result = block_on(composite.turn_on("room1", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::Timeout(_))));
        outcomes
            .wait_for(Duration::from_secs(5), |o| {
                matches!(o.status, HubDispatchStatus::SkippedCooldown { .. })
            })
            .expect("cooldown outcome");

        blocking.release();

        // Once the timed-out transport call succeeds, commands flow again.
        assert!(wait_until(Duration::from_secs(3), || {
            block_on(composite.turn_on("room1", LightingCommand::new(60, 3200))).is_ok()
        }));
        assert!(wait_until(Duration::from_secs(5), || {
            blocking.turn_on_count() >= 2
        }));
        blocking.release();
    }

    #[test]
    fn late_success_clears_timeout_cooldown_before_its_deadline() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                timeout_cooldown: Duration::from_secs(30),
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();
        outcomes
            .wait_for(Duration::from_secs(5), |outcome| {
                matches!(outcome.status, HubDispatchStatus::TimedOut { .. })
            })
            .expect("timeout outcome");

        let during_timeout = block_on(composite.turn_on("room1", LightingCommand::new(50, 3000)));
        assert!(matches!(during_timeout, Err(LightControlError::Timeout(_))));

        blocking.release();
        let released_at = Instant::now();
        assert!(wait_until(Duration::from_secs(3), || {
            block_on(composite.turn_on("room1", LightingCommand::new(60, 3200))).is_ok()
        }));
        assert!(
            released_at.elapsed() < Duration::from_secs(5),
            "retry waited for the 30-second cooldown instead of late success"
        );
        assert!(wait_until(Duration::from_secs(5), || {
            blocking.turn_on_count() >= 2
        }));
    }

    #[test]
    fn hub_scope_cooldown_rejects_other_targets() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                timeout_scope: HubDispatchTimeoutScope::Hub,
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[
            ("room1", "hub_a", group_target("room1")),
            ("room2", "hub_a", group_target("room2")),
        ]));

        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();
        outcomes
            .wait_for(Duration::from_secs(5), |o| {
                matches!(o.status, HubDispatchStatus::TimedOut { .. })
            })
            .expect("timeout outcome");

        let result = block_on(composite.turn_on("room2", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::Timeout(_))));
        blocking.release();
    }

    #[test]
    fn failed_dispatch_reports_outcome_but_accepts_command() {
        let mock = Arc::new(MockController::new("failing"));
        mock.set_fail(true);
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        // Enqueue succeeds — failure surfaces via the outcome event.
        block_on(composite.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();

        let failed = outcomes
            .wait_for(Duration::from_secs(5), |o| {
                matches!(&o.status, HubDispatchStatus::Failed { error } if error.contains("mock failure"))
            })
            .expect("failure outcome");
        assert_eq!(failed.node_id, "room1");
    }

    #[test]
    fn partial_hub_failure_still_dispatches_healthy_hub() {
        let healthy = Arc::new(MockController::new("healthy"));
        let failing = Arc::new(MockController::new("failing"));
        failing.set_fail(true);
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller("hub_a", healthy.clone());
        composite.register_controller("hub_b", failing.clone());
        composite.update_routing(route(&[
            ("hall", "hub_a", group_target("hall-a")),
            ("hall", "hub_b", group_target("hall-b")),
        ]));

        block_on(composite.turn_on("hall", LightingCommand::new(70, 3500))).unwrap();
        assert!(wait_until(Duration::from_secs(5), || {
            healthy.turn_on_count() == 1
        }));
        outcomes
            .wait_for(Duration::from_secs(5), |o| {
                o.hub_key == "hub_b" && !o.status.is_success()
            })
            .expect("failing hub outcome");
    }

    #[test]
    fn removed_controller_drops_pending_with_outcome() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller_with_policy(
            "hub_a",
            blocking.clone(),
            HubDispatchPolicy {
                dispatch_timeout: Duration::from_secs(10),
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[
            ("room1", "hub_a", group_target("room1")),
            ("room2", "hub_a", group_target("room2")),
        ]));

        // First occupies the single in-flight slot budget for its target;
        // second waits in the mailbox.
        block_on(composite.turn_on("room1", LightingCommand::new(10, 2000))).unwrap();
        assert!(blocking.wait_started_timeout(Duration::from_secs(5)));
        block_on(composite.turn_on("room1", LightingCommand::new(20, 2000))).unwrap();

        composite.remove_controller("hub_a");
        outcomes
            .wait_for(Duration::from_secs(5), |o| {
                matches!(o.status, HubDispatchStatus::Dropped { .. })
            })
            .expect("dropped outcome for pending command");
        blocking.release();
    }

    // ── Pacing ───────────────────────────────────────────────────────

    #[test]
    fn rate_limit_paces_dispatches() {
        let mock = Arc::new(MockController::new("paced"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "hue@bridge",
            mock.clone(),
            HubDispatchPolicy {
                rate_limit: Some(HubRateLimit {
                    capacity: 1.0,
                    refill_per_sec: 20.0,
                    group_cost: 1.0,
                    device_cost: 1.0,
                }),
                ..fast_timeout_policy()
            },
        );
        let rooms: Vec<String> = (0..5).map(|i| format!("room{i}")).collect();
        let mut table = HashMap::new();
        for room in &rooms {
            table.insert(
                room.clone(),
                vec![("hue@bridge".to_string(), group_target(room))],
            );
        }
        composite.update_routing(table);

        let started = Instant::now();
        for room in &rooms {
            block_on(composite.turn_on(room, LightingCommand::new(50, 3000))).unwrap();
        }
        assert!(wait_until(Duration::from_secs(5), || {
            mock.turn_on_count() == 5
        }));
        let elapsed = started.elapsed();
        // 1 token burst + 4 more at 20/s => at least ~200ms minus scheduling slack.
        assert!(
            elapsed >= Duration::from_millis(150),
            "5 paced dispatches finished in {:?}, pacing not applied",
            elapsed
        );
    }

    #[test]
    fn coalescing_updates_command_while_paced() {
        let mock = Arc::new(MockController::new("paced"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "hue@bridge",
            mock.clone(),
            HubDispatchPolicy {
                rate_limit: Some(HubRateLimit {
                    capacity: 1.0,
                    refill_per_sec: 5.0,
                    group_cost: 1.0,
                    device_cost: 1.0,
                }),
                ..fast_timeout_policy()
            },
        );
        composite.update_routing(route(&[
            ("room1", "hue@bridge", group_target("room1")),
            ("room2", "hue@bridge", group_target("room2")),
        ]));

        // Consume the burst token, then queue a paced command and update it.
        block_on(composite.turn_on("room1", LightingCommand::new(10, 2000))).unwrap();
        block_on(composite.turn_on("room2", LightingCommand::new(20, 2000))).unwrap();
        block_on(composite.turn_on("room2", LightingCommand::new(99, 2000))).unwrap();

        assert!(wait_until(Duration::from_secs(5), || {
            mock.turn_on_count() == 2
        }));
        let last = mock.last_turn_on().unwrap();
        assert_eq!(last.0, "room2");
        assert_eq!(last.1.brightness, 99);
    }

    // ── Queries ──────────────────────────────────────────────────────

    #[test]
    fn any_lights_on_returns_true_when_any_hub_reports_on() {
        let hub_a = Arc::new(MockController::new("a"));
        let hub_b = Arc::new(MockController::new("b"));
        hub_b.set_lights_on(true);
        let composite = CompositeController::new();
        composite.register_controller("hub_a", hub_a);
        composite.register_controller("hub_b", hub_b);
        composite.update_routing(route(&[
            ("room1", "hub_a", group_target("room1")),
            ("room1", "hub_b", group_target("room1")),
        ]));

        assert!(block_on(composite.any_lights_on("room1")).unwrap());
    }

    #[test]
    fn any_lights_on_false_when_all_off_or_failing() {
        let hub_a = Arc::new(MockController::new("a"));
        let hub_b = Arc::new(MockController::new("b"));
        hub_b.set_fail(true);
        let composite = CompositeController::new();
        composite.register_controller("hub_a", hub_a);
        composite.register_controller("hub_b", hub_b);
        composite.update_routing(route(&[
            ("room1", "hub_a", group_target("room1")),
            ("room1", "hub_b", group_target("room1")),
        ]));

        assert!(!block_on(composite.any_lights_on("room1")).unwrap());
    }

    #[test]
    fn any_lights_on_query_is_bounded_by_query_timeout() {
        let blocking = Arc::new(BlockingController::blocking_queries());
        let composite = CompositeController::new();
        composite.register_controller_with_policy("hub_a", blocking.clone(), fast_timeout_policy());
        composite.update_routing(route(&[("room1", "hub_a", group_target("room1"))]));

        let started = Instant::now();
        let result = block_on(composite.any_lights_on("room1"));
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(3),
            "query took {:?}, expected query timeout to bound it",
            elapsed
        );
        // Query failure is treated as "not on".
        assert!(!result.unwrap());
        blocking.release();
    }

    #[test]
    fn get_rooms_merges_and_dedupes() {
        let hub_a = Arc::new(MockController::with_rooms(
            "a",
            vec![
                Room::new("room1".to_string(), "Room 1".to_string()),
                Room::new("shared".to_string(), "Shared".to_string()),
            ],
        ));
        let hub_b = Arc::new(MockController::with_rooms(
            "b",
            vec![
                Room::new("room2".to_string(), "Room 2".to_string()),
                Room::new("shared".to_string(), "Shared".to_string()),
            ],
        ));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", hub_a);
        composite.register_controller("hub_b", hub_b);

        let rooms = block_on(composite.get_rooms()).unwrap();
        assert_eq!(rooms.len(), 3);
    }

    #[test]
    fn is_connected_when_any_hub_connected() {
        let hub_a = Arc::new(MockController::new("a"));
        let hub_b = Arc::new(MockController::new("b"));
        hub_a.set_fail(true);
        let composite = CompositeController::new();
        composite.register_controller("hub_a", hub_a.clone());
        composite.register_controller("hub_b", hub_b);
        assert!(block_on(composite.is_connected()));

        let lonely = CompositeController::new();
        let failing = Arc::new(MockController::new("x"));
        failing.set_fail(true);
        lonely.register_controller("hub_x", failing);
        assert!(!block_on(lonely.is_connected()));
    }

    #[test]
    fn asynchronous_controller_receipt_is_preserved_in_dispatch_outcome() {
        let controller = Arc::new(MockController::new("matter"));
        controller.set_accepted_receipt(vec![41, 42], "stream-a");
        let composite = CompositeController::new();
        let outcomes = OutcomeCollector::install(&composite);
        composite.register_controller("matter@local", controller);
        composite.update_routing(route(&[("room1", "matter@local", group_target("room1"))]));

        block_on(composite.turn_on("room1", LightingCommand::new(50, 3000))).unwrap();
        let outcome = outcomes
            .wait_for(Duration::from_secs(1), |outcome| {
                matches!(outcome.status, HubDispatchStatus::Accepted { .. })
            })
            .expect("accepted outcome");
        assert_eq!(
            outcome.status,
            HubDispatchStatus::Accepted {
                command_ids: vec![41, 42],
                controller_stream_id: "stream-a".to_string(),
            }
        );
    }
}
