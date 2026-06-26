//! Composite light controller for multi-hub fan-out.
//!
//! Wraps multiple per-hub [`HubLightController`] implementations and routes
//! commands to the correct hub(s) based on a room routing table. Supports
//! dynamic registration — controllers can be added/removed while the
//! `RhythmEngine` is running.
//!
//! ## Usage
//!
//! ```ignore
//! let composite = CompositeController::new();
//! composite.register_controller("hue@192.168.1.5", hue_controller);
//! composite.register_controller("hue@192.168.1.6", hue_controller_2);
//! composite.update_routing(hashmap! {
//!     "living-room" => vec![(
//!         "hue@192.168.1.5".to_string(),
//!         HubDispatchTarget::Group {
//!             room_id: "living-room".to_string(),
//!             control_id: "gl-living-room".to_string(),
//!         },
//!     )],
//!     "kitchen"     => vec![(
//!         "hue@192.168.1.6".to_string(),
//!         HubDispatchTarget::Group {
//!             room_id: "kitchen".to_string(),
//!             control_id: "gl-kitchen".to_string(),
//!         },
//!     )],
//! });
//! // Now turn_on("hallway", cmd) fans out to both bridges.
//! ```

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::warn;

use crate::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
};
use crate::lighting::LightingCommand;
use crate::room::Room;

const DISPATCH_WARN_MS: u128 = 1000;
const DEFAULT_HUB_QUEUE_CAPACITY: usize = 16;
const DEFAULT_HUB_DISPATCH_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_HUB_TIMEOUT_COOLDOWN: Duration = Duration::from_secs(30);
const DEFAULT_MATTER_TIMEOUT_COOLDOWN: Duration = Duration::from_secs(120);
const DISPATCH_RECEIPT_GRACE: Duration = Duration::from_millis(250);

/// Whether a command caller waits for the hub transport to complete.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubDispatchCompletion {
    /// Wait for the hub controller to finish the command.
    Wait,
    /// Return after the hub worker accepts the command.
    Enqueue,
}

/// Scope of the cooldown applied after a physical dispatch timeout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HubDispatchTimeoutScope {
    /// Only reject retries for the same target.
    Target,
    /// Reject all jobs for this hub while the timed-out job may still be alive.
    Hub,
}

/// Per-hub command queue policy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubDispatchPolicy {
    /// Maximum pending commands held for this hub.
    pub queue_capacity: usize,
    /// Completion behavior exposed to callers after enqueue.
    pub completion: HubDispatchCompletion,
    /// Whether this hub type needs burst pacing before dispatch.
    pub requires_staggering: bool,
    /// Worker-local minimum spacing between commands.
    pub min_dispatch_spacing: Duration,
    /// Maximum time a single physical dispatch may occupy the hub worker.
    pub dispatch_timeout: Duration,
    /// How long to reject commands to the same target after a worker timeout.
    pub timeout_cooldown: Duration,
    /// Whether timeout cooldown applies to one target or the whole hub.
    pub timeout_scope: HubDispatchTimeoutScope,
}

impl HubDispatchPolicy {
    /// Build the default policy for a registered hub key.
    pub fn for_hub_key(hub_key: &str) -> Self {
        match hub_type_from_key(hub_key) {
            "matter" => Self {
                queue_capacity: DEFAULT_HUB_QUEUE_CAPACITY,
                completion: HubDispatchCompletion::Enqueue,
                requires_staggering: false,
                min_dispatch_spacing: Duration::ZERO,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_MATTER_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Hub,
            },
            "hue" => Self {
                queue_capacity: DEFAULT_HUB_QUEUE_CAPACITY,
                completion: HubDispatchCompletion::Wait,
                requires_staggering: true,
                min_dispatch_spacing: Duration::ZERO,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_HUB_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Target,
            },
            _ => Self {
                queue_capacity: DEFAULT_HUB_QUEUE_CAPACITY,
                completion: HubDispatchCompletion::Wait,
                requires_staggering: false,
                min_dispatch_spacing: Duration::ZERO,
                dispatch_timeout: DEFAULT_HUB_DISPATCH_TIMEOUT,
                timeout_cooldown: DEFAULT_HUB_TIMEOUT_COOLDOWN,
                timeout_scope: HubDispatchTimeoutScope::Target,
            },
        }
    }
}

/// Public metadata describing a registered hub command queue.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HubDispatchMetadata {
    pub hub_key: String,
    pub hub_type: String,
    pub policy: HubDispatchPolicy,
}

pub(crate) fn format_node_log_label(node_id: &str, node_name: Option<&str>) -> String {
    match node_name
        .map(str::trim)
        .filter(|name| !name.is_empty() && *name != node_id)
    {
        Some(name) => format!("{} ({})", name, node_id),
        None => node_id.to_string(),
    }
}

/// Block on a future from a non-async context (spawned OS threads).
fn sync_block_on<F: std::future::Future>(f: F) -> F::Output {
    futures::executor::block_on(f)
}

fn hub_type_from_key(hub_key: &str) -> &str {
    hub_key
        .split_once('@')
        .map_or(hub_key, |(hub_type, _)| hub_type)
}

fn hub_worker_thread_name(hub_key: &str) -> String {
    let sanitized: String = hub_key
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("hub-dispatch-{sanitized}")
}

fn hub_job_thread_name(hub_key: &str, target_label: &str) -> String {
    let sanitized: String = format!("{hub_key}-{target_label}")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    format!("hub-dispatch-job-{sanitized}")
}

enum HubDispatchJob {
    TurnOn {
        target: HubDispatchTarget,
        command: LightingCommand,
        result_tx: Option<mpsc::Sender<LightControlResult<()>>>,
    },
    TurnOff {
        target: HubDispatchTarget,
        transition_ms: Option<u32>,
        result_tx: Option<mpsc::Sender<LightControlResult<()>>>,
    },
}

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

struct PreparedHubDispatchJob {
    target_label: String,
    action: HubDispatchAction,
    result_tx: Option<mpsc::Sender<LightControlResult<()>>>,
}

impl HubDispatchJob {
    fn prepare(self) -> PreparedHubDispatchJob {
        match self {
            Self::TurnOn {
                target,
                command,
                result_tx,
            } => PreparedHubDispatchJob {
                target_label: target.label(),
                action: HubDispatchAction::TurnOn { target, command },
                result_tx,
            },
            Self::TurnOff {
                target,
                transition_ms,
                result_tx,
            } => PreparedHubDispatchJob {
                target_label: target.label(),
                action: HubDispatchAction::TurnOff {
                    target,
                    transition_ms,
                },
                result_tx,
            },
        }
    }
}

impl HubDispatchAction {
    fn dispatch(self, controller: Arc<dyn HubLightController>) -> LightControlResult<()> {
        match self {
            Self::TurnOn { target, command } => {
                sync_block_on(controller.turn_on_target(&target, command))
            }
            Self::TurnOff {
                target,
                transition_ms,
            } => sync_block_on(controller.turn_off_target(&target, transition_ms)),
        }
    }
}

enum HubDispatchReceipt {
    Enqueued,
    Waiting {
        rx: Receiver<LightControlResult<()>>,
        timeout: Duration,
    },
}

impl HubDispatchReceipt {
    fn wait(self, hub_key: &str) -> LightControlResult<()> {
        match self {
            Self::Enqueued => Ok(()),
            Self::Waiting { rx, timeout } => match rx.recv_timeout(timeout) {
                Ok(result) => result,
                Err(RecvTimeoutError::Timeout) => Err(LightControlError::Timeout(format!(
                    "Hub {} dispatch did not complete within {}ms",
                    hub_key,
                    timeout.as_millis()
                ))),
                Err(RecvTimeoutError::Disconnected) => Err(LightControlError::Internal(format!(
                    "Hub {} dispatch worker stopped before returning a result",
                    hub_key
                ))),
            },
        }
    }
}

struct HubDispatchWorker {
    key: String,
    hub_type: String,
    controller: Arc<dyn HubLightController>,
    policy: HubDispatchPolicy,
    tx: SyncSender<HubDispatchJob>,
}

impl HubDispatchWorker {
    fn new(key: &str, controller: Arc<dyn HubLightController>, policy: HubDispatchPolicy) -> Self {
        let (tx, rx) = mpsc::sync_channel(policy.queue_capacity);
        let worker_key = key.to_string();
        let thread_controller = controller.clone();
        let thread_policy = policy.clone();
        std::thread::Builder::new()
            .name(hub_worker_thread_name(key))
            .spawn(move || {
                run_hub_dispatch_worker(worker_key, thread_controller, thread_policy, rx);
            })
            .expect("failed to spawn hub dispatch worker");

        Self {
            key: key.to_string(),
            hub_type: hub_type_from_key(key).to_string(),
            controller,
            policy,
            tx,
        }
    }

    fn metadata(&self) -> HubDispatchMetadata {
        HubDispatchMetadata {
            hub_key: self.key.clone(),
            hub_type: self.hub_type.clone(),
            policy: self.policy.clone(),
        }
    }

    fn enqueue_turn_on(
        &self,
        target: HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<HubDispatchReceipt> {
        let (result_tx, receipt) = self.result_channel();
        self.try_send(
            HubDispatchJob::TurnOn {
                target,
                command,
                result_tx,
            },
            receipt,
        )
    }

    fn enqueue_turn_off(
        &self,
        target: HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<HubDispatchReceipt> {
        let (result_tx, receipt) = self.result_channel();
        self.try_send(
            HubDispatchJob::TurnOff {
                target,
                transition_ms,
                result_tx,
            },
            receipt,
        )
    }

    fn result_channel(
        &self,
    ) -> (
        Option<mpsc::Sender<LightControlResult<()>>>,
        HubDispatchReceipt,
    ) {
        match self.policy.completion {
            HubDispatchCompletion::Wait => {
                let (tx, rx) = mpsc::channel();
                (
                    Some(tx),
                    HubDispatchReceipt::Waiting {
                        rx,
                        timeout: self.policy.dispatch_timeout + DISPATCH_RECEIPT_GRACE,
                    },
                )
            }
            HubDispatchCompletion::Enqueue => (None, HubDispatchReceipt::Enqueued),
        }
    }

    fn try_send(
        &self,
        job: HubDispatchJob,
        receipt: HubDispatchReceipt,
    ) -> LightControlResult<HubDispatchReceipt> {
        match self.tx.try_send(job) {
            Ok(()) => Ok(receipt),
            Err(TrySendError::Full(_)) => Err(LightControlError::CommandFailed(format!(
                "Hub {} dispatch queue is full",
                self.key
            ))),
            Err(TrySendError::Disconnected(_)) => Err(LightControlError::ConnectionError(format!(
                "Hub {} dispatch worker is not running",
                self.key
            ))),
        }
    }
}

fn run_hub_dispatch_worker(
    hub_key: String,
    controller: Arc<dyn HubLightController>,
    policy: HubDispatchPolicy,
    rx: Receiver<HubDispatchJob>,
) {
    let mut next_dispatch_at: Option<Instant> = None;
    let mut cooldowns: HashMap<String, Instant> = HashMap::new();
    let mut hub_cooldown_until: Option<Instant> = None;
    while let Ok(job) = rx.recv() {
        let PreparedHubDispatchJob {
            target_label,
            action,
            result_tx,
        } = job.prepare();

        let now = Instant::now();
        if hub_cooldown_until.is_some_and(|cooldown_until| cooldown_until <= now) {
            hub_cooldown_until = None;
        }
        if let Some(cooldown_until) = hub_cooldown_until {
            let remaining_ms = cooldown_until.saturating_duration_since(now).as_millis();
            let result = Err(LightControlError::Timeout(format!(
                "Hub {} dispatch is cooling down for {}ms after a timeout",
                hub_key, remaining_ms
            )));
            warn!(
                target: "composite",
                "hub {} dispatch to '{}' skipped during hub timeout cooldown ({}ms remaining)",
                hub_key,
                target_label,
                remaining_ms
            );
            if let Some(result_tx) = result_tx {
                let _ = result_tx.send(result);
            }
            continue;
        }

        cooldowns.retain(|_, cooldown_until| *cooldown_until > now);
        if let Some(cooldown_until) = cooldowns.get(&target_label).copied() {
            let remaining_ms = cooldown_until.saturating_duration_since(now).as_millis();
            let result = Err(LightControlError::Timeout(format!(
                "Hub {} dispatch to '{}' is cooling down for {}ms after a timeout",
                hub_key, target_label, remaining_ms
            )));
            warn!(
                target: "composite",
                "hub {} dispatch to '{}' skipped during timeout cooldown ({}ms remaining)",
                hub_key,
                target_label,
                remaining_ms
            );
            if let Some(result_tx) = result_tx {
                let _ = result_tx.send(result);
            }
            continue;
        }

        if let Some(deadline) = next_dispatch_at {
            let now = Instant::now();
            if deadline > now {
                std::thread::sleep(deadline - now);
            }
        }

        let started = Instant::now();
        let (timed_out, result) = run_hub_dispatch_job_with_timeout(
            &hub_key,
            &target_label,
            controller.clone(),
            action,
            policy.dispatch_timeout,
        );

        let latency_ms = started.elapsed().as_millis();
        if timed_out {
            let cooldown_until = Instant::now() + policy.timeout_cooldown;
            match policy.timeout_scope {
                HubDispatchTimeoutScope::Target => {
                    cooldowns.insert(target_label.clone(), cooldown_until);
                    warn!(
                        target: "composite",
                        "hub {} dispatch to '{}' timed out after {}ms; cooling target for {}ms",
                        hub_key,
                        target_label,
                        latency_ms,
                        policy.timeout_cooldown.as_millis()
                    );
                }
                HubDispatchTimeoutScope::Hub => {
                    hub_cooldown_until = Some(cooldown_until);
                    warn!(
                        target: "composite",
                        "hub {} dispatch to '{}' timed out after {}ms; cooling hub for {}ms",
                        hub_key,
                        target_label,
                        latency_ms,
                        policy.timeout_cooldown.as_millis()
                    );
                }
            }
        } else if let Err(e) = &result {
            warn!(
                target: "composite",
                "hub {} dispatch to '{}' failed after {}ms: {}",
                hub_key,
                target_label,
                latency_ms,
                e
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "hub_dispatch_slow",
                hub = %hub_key,
                target = %target_label,
                latency_ms,
                "Hub dispatch slow"
            );
        }

        if let Some(result_tx) = result_tx {
            let _ = result_tx.send(result);
        }

        if policy.min_dispatch_spacing > Duration::ZERO {
            next_dispatch_at = Some(Instant::now() + policy.min_dispatch_spacing);
        }
    }
}

fn run_hub_dispatch_job_with_timeout(
    hub_key: &str,
    target_label: &str,
    controller: Arc<dyn HubLightController>,
    action: HubDispatchAction,
    timeout: Duration,
) -> (bool, LightControlResult<()>) {
    let (tx, rx) = mpsc::channel();
    let thread_name = hub_job_thread_name(hub_key, target_label);
    if let Err(e) = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = action.dispatch(controller);
            let _ = tx.send(result);
        })
    {
        return (
            false,
            Err(LightControlError::Internal(format!(
                "Failed to spawn hub {} dispatch job for '{}': {}",
                hub_key, target_label, e
            ))),
        );
    }

    match rx.recv_timeout(timeout) {
        Ok(result) => (false, result),
        Err(RecvTimeoutError::Timeout) => (
            true,
            Err(LightControlError::Timeout(format!(
                "Hub {} dispatch to '{}' exceeded {}ms",
                hub_key,
                target_label,
                timeout.as_millis()
            ))),
        ),
        Err(RecvTimeoutError::Disconnected) => (
            false,
            Err(LightControlError::Internal(format!(
                "Hub {} dispatch job for '{}' stopped before returning a result",
                hub_key, target_label
            ))),
        ),
    }
}

/// A composite light controller that fans out commands to per-hub controllers.
///
/// Interior mutability via [`RwLock`] allows adding/removing controllers
/// and updating the routing table while the engine is running.
pub struct CompositeController {
    /// Per-hub dispatch workers keyed by hub identifier (e.g., "hue@192.168.1.5").
    controllers: RwLock<HashMap<String, Arc<HubDispatchWorker>>>,
    /// Room routing: topology_room_id → list of (hub_key, dispatch target) pairs.
    routing: RwLock<HashMap<String, Vec<(String, HubDispatchTarget)>>>,
    /// Human-readable node labels for logs keyed by public/synthetic node ID.
    node_labels: RwLock<HashMap<String, String>>,
}

impl CompositeController {
    /// Create an empty composite controller (no hubs registered).
    pub fn new() -> Self {
        Self {
            controllers: RwLock::new(HashMap::new()),
            routing: RwLock::new(HashMap::new()),
            node_labels: RwLock::new(HashMap::new()),
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
        if let Ok(mut controllers) = self.controllers.write() {
            controllers.insert(
                key.to_string(),
                Arc::new(HubDispatchWorker::new(key, controller, policy)),
            );
        }
    }

    /// Remove a per-hub controller.
    pub fn remove_controller(&self, key: &str) {
        if let Ok(mut controllers) = self.controllers.write() {
            controllers.remove(key);
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
                    .map(|worker| worker.metadata())
                    .collect()
            })
            .unwrap_or_default();
        metadata.sort_by(|a, b| a.hub_key.cmp(&b.hub_key));
        metadata
    }

    /// Get controller targets for a room from the routing table.
    ///
    /// Returns (hub_key, worker, target) triples.
    fn controllers_for_room(
        &self,
        room_id: &str,
    ) -> Vec<(String, Arc<HubDispatchWorker>, HubDispatchTarget)> {
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
                        .map(|worker| (hub_key.clone(), worker.clone(), target.clone()))
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

    /// Flash a concrete hub target for physical identification.
    ///
    /// This bypasses the runtime state machine intentionally: identification
    /// flashes should not change Rhythm's persisted room/node state.
    pub async fn flash_target(
        &self,
        hub_key: &str,
        target: HubDispatchTarget,
    ) -> LightControlResult<()> {
        let worker = self
            .controllers
            .read()
            .ok()
            .and_then(|controllers| controllers.get(hub_key).cloned())
            .ok_or_else(|| {
                LightControlError::RoomNotFound(format!(
                    "No controller registered for hub {}",
                    hub_key
                ))
            })?;

        worker.controller.flash_target(&target).await
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
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            let node_label = self.node_log_label(room_id);
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for node {}",
                node_label
            )));
        }

        let node_label = self.node_log_label(room_id);
        let started = Instant::now();

        let mut receipts = Vec::with_capacity(targets.len());
        for (key, worker, target) in &targets {
            match worker.enqueue_turn_on(target.clone(), command.clone()) {
                Ok(receipt) => receipts.push((key.clone(), receipt)),
                Err(e) => warn!(target: "composite", "hub {} failed: {}", key, e),
            }
        }

        let mut any_ok = false;
        for (key, receipt) in receipts {
            match receipt.wait(&key) {
                Ok(()) => any_ok = true,
                Err(e) => warn!(target: "composite", "hub {} failed: {}", key, e),
            }
        }

        let result = if any_ok {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(format!(
                "All controllers failed for node {}",
                node_label
            )))
        };

        let latency_ms = started.elapsed().as_millis();
        match &result {
            Ok(()) if latency_ms >= DISPATCH_WARN_MS => tracing::warn!(
                target: "cmd",
                event = "dispatch_turn_on",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                "Composite dispatch turn_on slow"
            ),
            Ok(()) => tracing::info!(
                target: "cmd",
                event = "dispatch_turn_on",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                "Composite dispatch turn_on"
            ),
            Err(e) => tracing::warn!(
                target: "cmd",
                event = "dispatch_turn_on_failed",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                error = %e,
                "Composite dispatch turn_on failed"
            ),
        }

        result
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        let targets = self.controllers_for_room(room_id);
        if targets.is_empty() {
            let node_label = self.node_log_label(room_id);
            return Err(LightControlError::RoomNotFound(format!(
                "No controllers for node {}",
                node_label
            )));
        }

        let node_label = self.node_log_label(room_id);
        let started = Instant::now();

        let mut receipts = Vec::with_capacity(targets.len());
        for (key, worker, target) in &targets {
            match worker.enqueue_turn_off(target.clone(), transition_ms) {
                Ok(receipt) => receipts.push((key.clone(), receipt)),
                Err(e) => warn!(target: "composite", "hub {} failed: {}", key, e),
            }
        }

        let mut any_ok = false;
        for (key, receipt) in receipts {
            match receipt.wait(&key) {
                Ok(()) => any_ok = true,
                Err(e) => warn!(target: "composite", "hub {} failed: {}", key, e),
            }
        }

        let result = if any_ok {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(format!(
                "All controllers failed for node {}",
                node_label
            )))
        };

        let latency_ms = started.elapsed().as_millis();
        match &result {
            Ok(()) if latency_ms >= DISPATCH_WARN_MS => tracing::warn!(
                target: "cmd",
                event = "dispatch_turn_off",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                "Composite dispatch turn_off slow"
            ),
            Ok(()) => tracing::info!(
                target: "cmd",
                event = "dispatch_turn_off",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                "Composite dispatch turn_off"
            ),
            Err(e) => tracing::warn!(
                target: "cmd",
                event = "dispatch_turn_off_failed",
                node_id = %room_id,
                node = %node_label,
                target_count = targets.len(),
                latency_ms,
                error = %e,
                "Composite dispatch turn_off failed"
            ),
        }

        result
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        let all_controllers: Vec<Arc<dyn HubLightController>> = self
            .controllers
            .read()
            .map(|c| c.values().map(|worker| worker.controller.clone()).collect())
            .unwrap_or_default();

        let mut rooms = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for controller in &all_controllers {
            if let Ok(hub_rooms) = controller.get_rooms().await {
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
        let all_controllers: Vec<Arc<dyn HubLightController>> = self
            .controllers
            .read()
            .map(|c| c.values().map(|worker| worker.controller.clone()).collect())
            .unwrap_or_default();

        for controller in &all_controllers {
            if controller.is_connected().await {
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

        for (key, worker, target) in &targets {
            match worker.controller.any_lights_on_target(target).await {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        target: "composite",
                        "any_lights_on '{}' via {}: {}",
                        target.label(),
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
    use std::sync::{Condvar, Mutex};

    /// Mock controller that records calls and can be configured to fail.
    struct MockController {
        name: String,
        should_fail: AtomicBool,
        turn_on_calls: Mutex<Vec<(String, LightingCommand)>>,
        turn_off_calls: Mutex<Vec<(String, Option<u32>)>>,
        lights_on: AtomicBool,
        rooms: Mutex<Vec<Room>>,
    }

    struct BlockingController {
        turn_on_calls: AtomicUsize,
        started: (Mutex<bool>, Condvar),
        release: (Mutex<bool>, Condvar),
    }

    impl BlockingController {
        fn new() -> Self {
            Self {
                turn_on_calls: AtomicUsize::new(0),
                started: (Mutex::new(false), Condvar::new()),
                release: (Mutex::new(false), Condvar::new()),
            }
        }

        fn wait_started(&self) {
            let (lock, cvar) = &self.started;
            let mut started = lock.lock().unwrap();
            while !*started {
                started = cvar.wait(started).unwrap();
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

        fn release(&self) {
            let (lock, cvar) = &self.release;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
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
            "blocking"
        }
    }

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

    impl MockController {
        fn new(name: &str) -> Self {
            Self {
                name: name.to_string(),
                should_fail: AtomicBool::new(false),
                turn_on_calls: Mutex::new(Vec::new()),
                turn_off_calls: Mutex::new(Vec::new()),
                lights_on: AtomicBool::new(false),
                rooms: Mutex::new(Vec::new()),
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

        fn turn_on_count(&self) -> usize {
            self.turn_on_calls.lock().unwrap().len()
        }

        fn turn_off_count(&self) -> usize {
            self.turn_off_calls.lock().unwrap().len()
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

    fn block_on<F: std::future::Future>(f: F) -> F::Output {
        // Minimal block_on for tests — no tokio/futures dependency needed.
        // async_trait methods are actually synchronous (blocking transport).
        let waker = std::task::Waker::noop();
        let mut cx = std::task::Context::from_waker(waker);
        let mut f = std::pin::pin!(f);
        loop {
            match f.as_mut().poll(&mut cx) {
                std::task::Poll::Ready(v) => return v,
                std::task::Poll::Pending => {
                    // In test context with blocking transports, this shouldn't happen.
                    // If it does, we'd spin — acceptable for tests only.
                    std::thread::yield_now();
                }
            }
        }
    }

    fn short_wait_policy() -> HubDispatchPolicy {
        HubDispatchPolicy {
            queue_capacity: 4,
            completion: HubDispatchCompletion::Wait,
            requires_staggering: false,
            min_dispatch_spacing: Duration::ZERO,
            dispatch_timeout: Duration::from_millis(75),
            timeout_cooldown: Duration::from_millis(500),
            timeout_scope: HubDispatchTimeoutScope::Target,
        }
    }

    fn short_hub_wait_policy() -> HubDispatchPolicy {
        HubDispatchPolicy {
            timeout_scope: HubDispatchTimeoutScope::Hub,
            ..short_wait_policy()
        }
    }

    fn turn_on_with_deadline(
        composite: Arc<CompositeController>,
        room_id: &str,
        command: LightingCommand,
        deadline: Duration,
    ) -> Result<(LightControlResult<()>, Duration), std::sync::mpsc::RecvTimeoutError> {
        let (tx, rx) = std::sync::mpsc::channel();
        let room_id = room_id.to_string();
        let started = Instant::now();
        std::thread::spawn(move || {
            let result = block_on(composite.turn_on(&room_id, command));
            let _ = tx.send((result, started.elapsed()));
        });
        rx.recv_timeout(deadline)
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

        assert_eq!(hue.policy.completion, HubDispatchCompletion::Wait);
        assert!(hue.policy.requires_staggering);
        assert_eq!(hue.policy.timeout_scope, HubDispatchTimeoutScope::Target);
        assert_eq!(matter.policy.completion, HubDispatchCompletion::Enqueue);
        assert!(!matter.policy.requires_staggering);
        assert_eq!(matter.policy.timeout_scope, HubDispatchTimeoutScope::Hub);
        assert_eq!(
            matter.policy.timeout_cooldown,
            DEFAULT_MATTER_TIMEOUT_COOLDOWN
        );
        assert_eq!(
            home_assistant.policy.completion,
            HubDispatchCompletion::Wait
        );
        assert!(!home_assistant.policy.requires_staggering);
        assert_eq!(
            home_assistant.policy.timeout_scope,
            HubDispatchTimeoutScope::Target
        );
    }

    #[test]
    fn register_controller_with_policy_uses_explicit_policy() {
        let composite = CompositeController::new();
        let policy = HubDispatchPolicy {
            queue_capacity: 4,
            completion: HubDispatchCompletion::Wait,
            requires_staggering: true,
            min_dispatch_spacing: Duration::from_millis(250),
            dispatch_timeout: Duration::from_secs(2),
            timeout_cooldown: Duration::from_secs(5),
            timeout_scope: HubDispatchTimeoutScope::Hub,
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

    // ── Single controller, single room ───────────────────────────────

    #[tokio::test]
    async fn single_controller_turn_on() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "hub_a".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        let cmd = LightingCommand::new(80, 4000);
        composite.turn_on("room1", cmd).await.unwrap();
        assert_eq!(mock.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn single_controller_turn_off() {
        let mock = Arc::new(MockController::new("hub_a"));
        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "hub_a".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        composite.turn_off("room1", None).await.unwrap();
        assert_eq!(mock.turn_off_count(), 1);
    }

    // ── Cross-hub fan-out ────────────────────────────────────────────

    #[tokio::test]
    async fn cross_hub_turn_on_fans_out() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a.clone());
        composite.register_controller("hub_b", mock_b.clone());
        composite.update_routing(HashMap::from([(
            "hallway".to_string(),
            vec![
                (
                    "hub_a".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "hallway".to_string(),
                        control_id: "hallway".to_string(),
                    },
                ),
                (
                    "hub_b".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "hallway".to_string(),
                        control_id: "hallway".to_string(),
                    },
                ),
            ],
        )]));

        let cmd = LightingCommand::new(60, 3500);
        composite.turn_on("hallway", cmd).await.unwrap();
        assert_eq!(mock_a.turn_on_count(), 1);
        assert_eq!(mock_b.turn_on_count(), 1);
    }

    #[tokio::test]
    async fn matter_turn_on_returns_after_enqueue() {
        let blocking = Arc::new(BlockingController::new());
        let composite = CompositeController::new();
        composite.register_controller("matter@local", blocking.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "matter@local".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        let started = Instant::now();
        composite
            .turn_on("room1", LightingCommand::new(80, 4000))
            .await
            .unwrap();
        assert!(
            started.elapsed() < Duration::from_millis(250),
            "Matter dispatch waited for worker completion"
        );

        blocking.wait_started();
        assert_eq!(blocking.turn_on_count(), 1);
        blocking.release();
    }

    #[test]
    fn wait_policy_turn_on_times_out_instead_of_waiting_forever() {
        let blocking = Arc::new(BlockingController::new());
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "hue@bridge",
            blocking.clone(),
            short_wait_policy(),
        );
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "hue@bridge".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        let outcome = turn_on_with_deadline(
            composite,
            "room1",
            LightingCommand::new(80, 4000),
            Duration::from_millis(500),
        )
        .unwrap_or_else(|_| {
            blocking.release();
            panic!("wait-policy dispatch did not release the caller before the deadline");
        });

        let started = blocking.wait_started_timeout(Duration::from_millis(250));
        blocking.release();

        assert!(
            outcome.0.is_err(),
            "timed-out dispatch should report failure"
        );
        assert!(
            outcome.1 < Duration::from_millis(500),
            "dispatch held caller for {:?}",
            outcome.1
        );
        assert!(started, "blocking controller job should have started");
        assert_eq!(blocking.turn_on_count(), 1);
    }

    #[test]
    fn timed_out_target_cools_down_without_blocking_other_targets() {
        let controller = Arc::new(SelectiveBlockingController::new("blocked"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "hue@bridge",
            controller.clone(),
            short_wait_policy(),
        );
        composite.update_routing(HashMap::from([
            (
                "blocked".to_string(),
                vec![(
                    "hue@bridge".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "blocked".to_string(),
                        control_id: "blocked".to_string(),
                    },
                )],
            ),
            (
                "other".to_string(),
                vec![(
                    "hue@bridge".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "other".to_string(),
                        control_id: "other".to_string(),
                    },
                )],
            ),
        ]));

        let first = turn_on_with_deadline(
            composite.clone(),
            "blocked",
            LightingCommand::new(80, 4000),
            Duration::from_millis(500),
        )
        .unwrap_or_else(|_| {
            controller.release();
            panic!("first blocked dispatch did not release the caller");
        });
        assert!(first.0.is_err());
        assert!(
            controller.wait_blocked_started_timeout(Duration::from_millis(250)),
            "blocked target dispatch should have started"
        );
        assert_eq!(controller.turn_on_count_for("blocked"), 1);

        let retry = turn_on_with_deadline(
            composite.clone(),
            "blocked",
            LightingCommand::new(80, 4000),
            Duration::from_millis(250),
        )
        .unwrap_or_else(|_| {
            controller.release();
            panic!("cooldown retry did not fail fast");
        });
        assert!(retry.0.is_err());
        assert!(
            retry.1 < Duration::from_millis(150),
            "cooldown retry took {:?}",
            retry.1
        );
        assert_eq!(
            controller.turn_on_count_for("blocked"),
            1,
            "cooldown should not start another job for the stuck target"
        );

        let other = turn_on_with_deadline(
            composite,
            "other",
            LightingCommand::new(70, 3500),
            Duration::from_millis(250),
        )
        .unwrap_or_else(|_| {
            controller.release();
            panic!("other target was blocked behind the stuck target");
        });
        controller.release();

        other.0.unwrap();
        assert!(
            other.1 < Duration::from_millis(150),
            "other target dispatch took {:?}",
            other.1
        );
        assert_eq!(controller.turn_on_count_for("other"), 1);
    }

    #[test]
    fn hub_scoped_timeout_cools_down_all_targets() {
        let controller = Arc::new(SelectiveBlockingController::new("blocked"));
        let composite = Arc::new(CompositeController::new());
        composite.register_controller_with_policy(
            "matter@local",
            controller.clone(),
            short_hub_wait_policy(),
        );
        composite.update_routing(HashMap::from([
            (
                "blocked".to_string(),
                vec![(
                    "matter@local".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "blocked".to_string(),
                        control_id: "blocked".to_string(),
                    },
                )],
            ),
            (
                "other".to_string(),
                vec![(
                    "matter@local".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "other".to_string(),
                        control_id: "other".to_string(),
                    },
                )],
            ),
        ]));

        let first = turn_on_with_deadline(
            composite.clone(),
            "blocked",
            LightingCommand::new(80, 4000),
            Duration::from_millis(500),
        )
        .unwrap_or_else(|_| {
            controller.release();
            panic!("first blocked dispatch did not release the caller");
        });
        assert!(first.0.is_err());
        assert!(
            controller.wait_blocked_started_timeout(Duration::from_millis(250)),
            "blocked target dispatch should have started"
        );

        let other = turn_on_with_deadline(
            composite,
            "other",
            LightingCommand::new(70, 3500),
            Duration::from_millis(250),
        )
        .unwrap_or_else(|_| {
            controller.release();
            panic!("hub cooldown retry did not fail fast");
        });
        controller.release();

        assert!(other.0.is_err());
        assert!(
            other.1 < Duration::from_millis(150),
            "hub cooldown retry took {:?}",
            other.1
        );
        assert_eq!(controller.turn_on_count_for("blocked"), 1);
        assert_eq!(
            controller.turn_on_count_for("other"),
            0,
            "hub cooldown should not start another job while the timed-out job may still be alive"
        );
    }

    // ── any_lights_on OR semantics ───────────────────────────────────

    #[test]
    fn any_lights_on_or_across_hubs() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_lights_on(false);
        mock_b.set_lights_on(true);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![
                (
                    "hub_a".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "room1".to_string(),
                        control_id: "room1".to_string(),
                    },
                ),
                (
                    "hub_b".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "room1".to_string(),
                        control_id: "room1".to_string(),
                    },
                ),
            ],
        )]));

        assert!(block_on(composite.any_lights_on("room1")).unwrap());
    }

    #[test]
    fn any_lights_on_all_off() {
        let mock = Arc::new(MockController::new("hub_a"));
        mock.set_lights_on(false);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "hub_a".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        assert!(!block_on(composite.any_lights_on("room1")).unwrap());
    }

    // ── Unknown room ─────────────────────────────────────────────────

    #[test]
    fn unknown_room_no_controllers_returns_error() {
        let composite = CompositeController::new();
        // No controllers registered at all
        let result = block_on(composite.turn_on("unknown", LightingCommand::new(50, 3000)));
        assert!(result.is_err());
    }

    #[test]
    fn unknown_room_error_uses_node_label_when_available() {
        let composite = CompositeController::new();
        composite.update_node_labels(HashMap::from([(
            "device-1".to_string(),
            "Kitchen Motion".to_string(),
        )]));

        let result = block_on(composite.turn_on("device-1", LightingCommand::new(50, 3000)))
            .expect_err("missing routing should return an error");

        assert!(result.to_string().contains("Kitchen Motion (device-1)"));
    }

    // ── Partial failure ──────────────────────────────────────────────

    #[tokio::test]
    async fn partial_failure_still_succeeds() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_fail(true); // hub_a will fail

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a.clone());
        composite.register_controller("hub_b", mock_b.clone());
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![
                (
                    "hub_a".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "room1".to_string(),
                        control_id: "room1".to_string(),
                    },
                ),
                (
                    "hub_b".to_string(),
                    HubDispatchTarget::Group {
                        room_id: "room1".to_string(),
                        control_id: "room1".to_string(),
                    },
                ),
            ],
        )]));

        // Should succeed because hub_b works
        let cmd = LightingCommand::new(80, 4000);
        composite.turn_on("room1", cmd).await.unwrap();
        assert_eq!(mock_a.turn_on_count(), 0); // failed
        assert_eq!(mock_b.turn_on_count(), 1); // succeeded
    }

    #[tokio::test]
    async fn all_controllers_fail_returns_error() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        mock_a.set_fail(true);

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.update_routing(HashMap::from([(
            "room1".to_string(),
            vec![(
                "hub_a".to_string(),
                HubDispatchTarget::Group {
                    room_id: "room1".to_string(),
                    control_id: "room1".to_string(),
                },
            )],
        )]));

        let result = composite
            .turn_on("room1", LightingCommand::new(50, 3000))
            .await;
        assert!(result.is_err());
    }

    // ── get_rooms deduplicates ───────────────────────────────────────

    #[test]
    fn get_rooms_deduplicates() {
        let room = Room::new("room1", "Living Room");

        let mock_a = Arc::new(MockController::with_rooms("hub_a", vec![room.clone()]));
        let mock_b = Arc::new(MockController::with_rooms("hub_b", vec![room]));

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);

        let rooms = block_on(composite.get_rooms()).unwrap();
        assert_eq!(rooms.len(), 1); // deduplicated
    }

    // ── is_connected ─────────────────────────────────────────────────

    #[test]
    fn is_connected_any_hub() {
        let mock_a = Arc::new(MockController::new("hub_a"));
        let mock_b = Arc::new(MockController::new("hub_b"));
        mock_a.set_fail(true); // disconnected
                               // mock_b is connected

        let composite = CompositeController::new();
        composite.register_controller("hub_a", mock_a);
        composite.register_controller("hub_b", mock_b);

        assert!(block_on(composite.is_connected()));
    }

    #[test]
    fn is_connected_none() {
        let composite = CompositeController::new();
        assert!(!block_on(composite.is_connected()));
    }

    // ── name ─────────────────────────────────────────────────────────

    #[test]
    fn name_is_composite() {
        let composite = CompositeController::new();
        assert_eq!(composite.name(), "Composite");
    }
}
