use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use rhythm_matter::transport::{
    MatterCommandFailureClass, MatterCommandOutcome, MatterCommandOutcomeStatus, MatterCommandStep,
    MatterCommandSubmission, MatterControllerEvent, MatterControllerEventBatch,
    MatterControllerEventCursor, MatterControllerEventEnvelope, MatterEndpointCommandPlan,
};

use crate::backend::ChipControllerBackend;

const EVENT_CAPACITY: usize = 2_048;
const MAX_EVENT_WAIT: Duration = Duration::from_secs(30);
/// One shared budget bounds endpoint command plans and subscription attempts.
/// Lane admission derives from it, so there is no second constant to drift.
pub(crate) const MAX_CONCURRENT_CONTROLLER_WORK: usize = 4;
static NEXT_STREAM_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct WorkBudgetState {
    next_ticket: u64,
    serving_ticket: u64,
    recovery_waiters: usize,
    in_flight: usize,
    recovery_in_flight: usize,
}

/// Shared bound for commands and subscription setup. Unknown/unavailable peers
/// share one recovery slot; they cannot consume the capacity reserved for peers
/// with current controller proof of life. Ready work bypasses recovery waiters.
pub(crate) struct ControllerWorkBudget {
    capacity: usize,
    state: Mutex<WorkBudgetState>,
    changed: Condvar,
}

impl ControllerWorkBudget {
    pub(crate) fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "controller work capacity must be non-zero");
        Self {
            capacity,
            state: Mutex::new(WorkBudgetState::default()),
            changed: Condvar::new(),
        }
    }

    /// Subscription setup can block its RPC caller, but never takes a command
    /// worker while waiting. FIFO applies within recovery, not across healthy work.
    pub(crate) fn acquire(self: &Arc<Self>) -> ControllerWorkPermit {
        let mut state = self.state.lock().expect("chipd work budget lock poisoned");
        let ticket = state.next_ticket;
        state.next_ticket += 1;
        state.recovery_waiters += 1;
        while ticket != state.serving_ticket
            || state.in_flight >= self.capacity
            || state.recovery_in_flight != 0
        {
            state = self
                .changed
                .wait(state)
                .expect("chipd work budget lock poisoned while waiting");
        }
        state.serving_ticket += 1;
        state.recovery_waiters -= 1;
        state.in_flight += 1;
        state.recovery_in_flight += 1;
        ControllerWorkPermit {
            budget: self.clone(),
            recovery: true,
        }
    }

    fn try_acquire(self: &Arc<Self>, recovery: bool) -> Option<ControllerWorkPermit> {
        let mut state = self.state.lock().expect("chipd work budget lock poisoned");
        if state.in_flight >= self.capacity
            || (!recovery
                && state.in_flight - state.recovery_in_flight
                    >= self.capacity.saturating_sub(1).max(1))
            || (recovery && (state.recovery_in_flight != 0 || state.recovery_waiters != 0))
        {
            return None;
        }
        state.in_flight += 1;
        state.recovery_in_flight += usize::from(recovery);
        Some(ControllerWorkPermit {
            budget: self.clone(),
            recovery,
        })
    }

    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

pub(crate) struct ControllerWorkPermit {
    budget: Arc<ControllerWorkBudget>,
    recovery: bool,
}

impl Drop for ControllerWorkPermit {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .expect("chipd work budget lock poisoned while releasing");
        state.in_flight -= 1;
        state.recovery_in_flight -= usize::from(self.recovery);
        self.budget.changed.notify_all();
    }
}

#[derive(Default)]
struct BrokerState {
    next_sequence: u64,
    events: VecDeque<MatterControllerEventEnvelope>,
}

/// Bounded, restart-aware controller event stream shared by command workers
/// and long-poll RPCs.
pub struct ControllerEventBroker {
    stream_id: String,
    state: Mutex<BrokerState>,
    changed: Condvar,
}

impl ControllerEventBroker {
    pub fn new() -> Self {
        let started = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let nonce = NEXT_STREAM_NONCE.fetch_add(1, Ordering::Relaxed);
        Self {
            stream_id: format!("{}-{started}-{nonce}", std::process::id()),
            state: Mutex::new(BrokerState {
                next_sequence: 1,
                events: VecDeque::new(),
            }),
            changed: Condvar::new(),
        }
    }

    pub fn publish(&self, event: MatterControllerEvent) {
        let mut state = self.state.lock().expect("chipd event broker lock poisoned");
        let sequence = state.next_sequence;
        state.next_sequence = state.next_sequence.saturating_add(1);
        state
            .events
            .push_back(MatterControllerEventEnvelope { sequence, event });
        while state.events.len() > EVENT_CAPACITY {
            state.events.pop_front();
        }
        self.changed.notify_all();
    }

    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    pub fn wait(
        &self,
        cursor: Option<&MatterControllerEventCursor>,
        max_wait: Duration,
    ) -> MatterControllerEventBatch {
        let mut state = self.state.lock().expect("chipd event broker lock poisoned");
        let wait = max_wait.min(MAX_EVENT_WAIT);
        let cursor_sequence = cursor
            .filter(|cursor| cursor.stream_id == self.stream_id)
            .map(|cursor| cursor.sequence)
            .unwrap_or(0);

        if !state
            .events
            .iter()
            .any(|event| event.sequence > cursor_sequence)
            && !wait.is_zero()
        {
            let (next, _) = self
                .changed
                .wait_timeout(state, wait)
                .expect("chipd event broker lock poisoned while waiting");
            state = next;
        }

        let oldest_sequence = state
            .events
            .front()
            .map(|event| event.sequence)
            .unwrap_or(state.next_sequence);
        let events = state
            .events
            .iter()
            .filter(|event| event.sequence > cursor_sequence)
            .cloned()
            .collect();

        MatterControllerEventBatch {
            stream_id: self.stream_id.clone(),
            oldest_sequence,
            events,
        }
    }
}

#[derive(Default)]
struct EndpointSlot {
    running: bool,
    pending: Option<MatterEndpointCommandPlan>,
}

#[derive(Default)]
struct DispatchState {
    slots: HashMap<(u64, u16), EndpointSlot>,
    active_ids: HashSet<u64>,
    ready: VecDeque<(u64, u16)>,
    active_lanes: usize,
    health: HashMap<(u64, u16), EndpointHealth>,
}

#[derive(Default)]
struct EndpointHealth {
    proof_at: Option<Instant>,
    failures: u32,
    failed_at: Option<Instant>,
    retry_at: Option<Instant>,
}

impl EndpointHealth {
    fn ready(&self) -> bool {
        self.proof_at.is_some() && self.retry_at.is_none()
    }
    fn cooling_down(&self) -> bool {
        self.retry_at.is_some_and(|at| Instant::now() < at)
    }
}

/// Fair, bounded per-endpoint command executor.
///
/// Exactly one plan can run for an endpoint; one newer desired state is
/// retained and any older queued state is terminally superseded. Controller
/// work is capped globally so a large room fan-out cannot synchronize dozens
/// of operational-discovery or CASE attempts. Ready endpoints are admitted in
/// FIFO order, so an unavailable endpoint cannot repeatedly jump ahead of a
/// healthy peer.
pub struct CommandDispatcher {
    backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
    broker: Arc<ControllerEventBroker>,
    work_budget: Arc<ControllerWorkBudget>,
    state: Mutex<DispatchState>,
}

impl CommandDispatcher {
    /// Standalone dispatcher with its own controller work budget.
    ///
    /// The daemon always shares one budget with the service
    /// (`with_work_budget`); this plain constructor is for standalone use.
    #[allow(dead_code)]
    pub fn new(
        backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
        broker: Arc<ControllerEventBroker>,
    ) -> Arc<Self> {
        let work_budget = Arc::new(ControllerWorkBudget::new(MAX_CONCURRENT_CONTROLLER_WORK));
        Self::with_work_budget(backend, broker, work_budget)
    }

    pub(crate) fn with_work_budget(
        backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
        broker: Arc<ControllerEventBroker>,
        work_budget: Arc<ControllerWorkBudget>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            broker,
            work_budget,
            state: Mutex::new(DispatchState::default()),
        })
    }

    pub fn submit(
        self: &Arc<Self>,
        plans: Vec<MatterEndpointCommandPlan>,
    ) -> Result<Vec<MatterCommandSubmission>> {
        validate_plans(&plans)?;

        let mut submissions = Vec::with_capacity(plans.len());
        let mut superseded = Vec::new();
        {
            let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
            if let Some(command_id) = plans
                .iter()
                .map(|plan| plan.command_id)
                .find(|command_id| state.active_ids.contains(command_id))
            {
                anyhow::bail!("Matter command id {} is already active", command_id);
            }
            for plan in plans {
                state.active_ids.insert(plan.command_id);

                let key = (plan.node_id, plan.endpoint);
                let slot = state.slots.entry(key).or_default();
                let was_idle = !slot.running && slot.pending.is_none();
                if let Some(previous) = slot.pending.replace(plan.clone()) {
                    state.active_ids.remove(&previous.command_id);
                    superseded.push(previous);
                }
                if was_idle {
                    state.ready.push_back(key);
                }
                submissions.push(MatterCommandSubmission {
                    command_id: plan.command_id,
                    completed: false,
                    controller_stream_id: Some(self.broker.stream_id().to_string()),
                });
            }
        }

        for plan in superseded {
            self.publish_outcome(&plan, MatterCommandOutcomeStatus::Superseded, None, None);
        }
        self.start_ready_lanes();
        Ok(submissions)
    }

    fn spawn_lane(
        self: &Arc<Self>,
        plan: MatterEndpointCommandPlan,
        permit: ControllerWorkPermit,
    ) -> Result<()> {
        let dispatcher = self.clone();
        std::thread::Builder::new()
            .name(format!("chipd-matter-{}-{}", plan.node_id, plan.endpoint))
            .spawn(move || dispatcher.run_lane(plan, permit))?;
        Ok(())
    }

    /// Reserve controller capacity before spawning. A pending recovery target
    /// cannot occupy a worker or block a ready target behind it in the queue.
    pub(crate) fn start_ready_lanes(self: &Arc<Self>) {
        loop {
            let selected = {
                let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
                if state.active_lanes >= self.work_budget.capacity() {
                    return;
                }
                let mut selected = None;
                for _ in 0..state.ready.len() {
                    let Some(key) = state.ready.pop_front() else {
                        break;
                    };
                    let Some(slot) = state.slots.get(&key) else {
                        continue;
                    };
                    if slot.running || slot.pending.is_none() {
                        continue;
                    }
                    let health = state.health.get(&key);
                    if health.is_some_and(EndpointHealth::cooling_down) {
                        let plan = state.slots.remove(&key).unwrap().pending.unwrap();
                        state.active_ids.remove(&plan.command_id);
                        selected = Some((plan, None));
                        break;
                    }
                    let recovery = !health.is_some_and(EndpointHealth::ready);
                    let Some(permit) = self.work_budget.try_acquire(recovery) else {
                        state.ready.push_back(key);
                        continue;
                    };
                    let slot = state.slots.get_mut(&key).unwrap();
                    let plan = slot.pending.take().unwrap();
                    slot.running = true;
                    state.active_lanes += 1;
                    selected = Some((plan, Some(permit)));
                    break;
                }
                selected
            };
            let Some((plan, permit)) = selected else {
                return;
            };
            let Some(permit) = permit else {
                self.publish_outcome(
                    &plan,
                    MatterCommandOutcomeStatus::Failed,
                    Some("Matter endpoint is awaiting connectivity recovery".into()),
                    Some(MatterCommandFailureClass::Connectivity),
                );
                continue;
            };
            if let Err(error) = self.spawn_lane(plan.clone(), permit) {
                self.publish_outcome(
                    &plan,
                    MatterCommandOutcomeStatus::Failed,
                    Some(format!("starting endpoint worker: {error:#}")),
                    Some(MatterCommandFailureClass::Other),
                );
                self.release_lane_state(&plan);
            }
        }
    }

    /// Only native command completion or a received report is proof of life.
    /// Subscription admission alone must not promote an unreachable target.
    pub(crate) fn record_endpoint_proof(self: &Arc<Self>, node_id: u64, endpoint: u16) {
        self.record_endpoint_proof_at(node_id, endpoint, Instant::now());
    }

    pub(crate) fn record_endpoint_proof_at(
        self: &Arc<Self>,
        node_id: u64,
        endpoint: u16,
        received: Instant,
    ) {
        let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
        let health = state.health.entry((node_id, endpoint)).or_default();
        if health.failed_at.is_some_and(|failed| received <= failed)
            || health.proof_at.is_some_and(|proof| received <= proof)
        {
            return;
        }
        *health = EndpointHealth {
            proof_at: Some(received),
            ..Default::default()
        };
        drop(state);
        self.start_ready_lanes();
    }

    pub(crate) fn record_connectivity_failure(
        &self,
        node_id: u64,
        endpoint: u16,
        started: Instant,
    ) {
        let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
        let health = state.health.entry((node_id, endpoint)).or_default();
        // A late timeout must not overwrite proof received after its operation began.
        if health.proof_at.is_some_and(|proof| proof > started) {
            return;
        }
        health.failures = health.failures.saturating_add(1);
        let delay =
            Duration::from_secs((5_u64 << health.failures.saturating_sub(1).min(6)).min(300));
        health.proof_at = None;
        health.failed_at = Some(Instant::now());
        health.retry_at = Some(Instant::now() + delay);
    }

    pub(crate) fn forget_node(&self, node_id: u64) {
        self.state
            .lock()
            .expect("chipd dispatch lock poisoned")
            .health
            .retain(|(node, _), _| *node != node_id);
    }

    pub(crate) fn reset_reachability(&self) {
        self.state
            .lock()
            .expect("chipd dispatch lock poisoned")
            .health
            .clear();
    }

    /// Run one plan to completion.
    ///
    /// There is no host wall-clock deadline here: a single unavailable bulb can
    /// legitimately hold its lane for the SDK's own operational-discovery and
    /// CASE timeouts, and killing chipd for that would restart every healthy
    /// endpoint too. The bounds that matter are the SDK's, the native bridge's
    /// last-resort wedge guard, and the client RPC timeout.
    fn run_lane(self: Arc<Self>, plan: MatterEndpointCommandPlan, permit: ControllerWorkPermit) {
        let started = Instant::now();
        let result = self.execute(&plan);
        match result {
            Ok(detail) => {
                self.record_endpoint_proof(plan.node_id, plan.endpoint);
                self.publish_outcome(&plan, MatterCommandOutcomeStatus::Succeeded, detail, None)
            }
            Err(error) => {
                let failure_class = if is_connectivity_failure(&error) {
                    self.record_connectivity_failure(plan.node_id, plan.endpoint, started);
                    MatterCommandFailureClass::Connectivity
                } else {
                    MatterCommandFailureClass::Other
                };
                self.publish_outcome(
                    &plan,
                    MatterCommandOutcomeStatus::Failed,
                    Some(format!("{error:#}")),
                    Some(failure_class),
                )
            }
        }
        drop(permit);
        self.finish_lane(&plan);
    }

    /// Release one running plan and put any newer state for the same endpoint
    /// at the back of the FIFO.
    ///
    /// Deliberately does not admit work, so it is safe to call from inside
    /// `start_ready_lanes` (the spawn-failure path) without recursing.
    fn release_lane_state(&self, plan: &MatterEndpointCommandPlan) {
        let key = (plan.node_id, plan.endpoint);
        let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
        state.active_ids.remove(&plan.command_id);
        let Some(slot) = state.slots.get_mut(&key) else {
            state.active_lanes = state.active_lanes.saturating_sub(1);
            return;
        };
        slot.running = false;
        if slot.pending.is_some() {
            state.ready.push_back(key);
        } else {
            state.slots.remove(&key);
        }
        state.active_lanes = state.active_lanes.saturating_sub(1);
    }

    /// Release a finished lane and admit whatever the freed slot allows.
    fn finish_lane(self: &Arc<Self>, plan: &MatterEndpointCommandPlan) {
        self.release_lane_state(plan);
        self.start_ready_lanes();
    }

    /// Run every step of one plan.
    ///
    /// A rejected color step does not stop the level or on/off step behind
    /// it: a light that comes on at the wrong color beats a light that stays
    /// dark. The plan still succeeds, and the color failure travels in the
    /// outcome detail so it is visible. When no later step succeeds there is
    /// nothing to salvage and the plan fails. A color step that fails because
    /// the node is unreachable, and any other step failure, still aborts the
    /// plan immediately.
    fn execute(&self, plan: &MatterEndpointCommandPlan) -> Result<Option<String>> {
        let mut color_failure: Option<anyhow::Error> = None;
        let mut succeeded_after_color_failure = false;
        for (index, step) in plan.steps.iter().enumerate() {
            // Admission reserved a permit for this endpoint plan. Recovery
            // plans cannot acquire the slots reserved for responsive peers.
            let backend = self.backend.read().expect("chipd backend lock poisoned");
            let step_result = execute_step(backend.as_ref(), plan.node_id, plan.endpoint, step);
            drop(backend);
            match step_result {
                Ok(()) => {
                    if color_failure.is_some() {
                        succeeded_after_color_failure = true;
                    }
                }
                Err(error) if is_color_step(step) && !is_connectivity_failure(&error) => {
                    color_failure.get_or_insert(error);
                }
                Err(error) => {
                    return Err(match color_failure.take() {
                        Some(color_error) => {
                            error.context(format!("after rejected color step: {color_error:#}"))
                        }
                        None => error,
                    });
                }
            }

            if index + 1 < plan.steps.len() {
                if let Some(delay_ms) = plan.inter_step_delay_ms.filter(|delay| *delay > 0) {
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
            }
        }
        match color_failure {
            Some(error) if succeeded_after_color_failure => Ok(Some(format!(
                "color step rejected, later steps succeeded: {error:#}"
            ))),
            Some(error) => Err(error),
            None => Ok(None),
        }
    }

    fn publish_outcome(
        &self,
        plan: &MatterEndpointCommandPlan,
        status: MatterCommandOutcomeStatus,
        detail: Option<String>,
        failure_class: Option<MatterCommandFailureClass>,
    ) {
        self.broker.publish(MatterControllerEvent::CommandOutcome(
            MatterCommandOutcome {
                command_id: plan.command_id,
                node_id: plan.node_id,
                endpoint: plan.endpoint,
                status,
                completed_at_unix_ms: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|duration| duration.as_millis() as u64),
                detail,
                failure_class,
            },
        ));
    }
}

fn validate_plans(plans: &[MatterEndpointCommandPlan]) -> Result<()> {
    let mut ids = HashSet::new();
    for plan in plans {
        if plan.command_id == 0 {
            anyhow::bail!("Matter command id must be non-zero");
        }
        if !ids.insert(plan.command_id) {
            anyhow::bail!(
                "Matter command id {} is duplicated in submission",
                plan.command_id
            );
        }
        if plan.endpoint == 0 {
            anyhow::bail!("Matter command {} targets endpoint zero", plan.command_id);
        }
        if plan.steps.is_empty() {
            anyhow::bail!("Matter command {} has no steps", plan.command_id);
        }
    }
    Ok(())
}

fn is_color_step(step: &MatterCommandStep) -> bool {
    matches!(
        step,
        MatterCommandStep::SetColorTemperature { .. }
            | MatterCommandStep::SetXy { .. }
            | MatterCommandStep::SetHueSaturation { .. }
    )
}

/// A failure that says the node could not be reached, as opposed to a
/// cluster rejecting the command. Mirrors
/// `MatterLightController::looks_like_connectivity_timeout` in rhythm-matter.
pub(crate) fn is_connectivity_failure(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        let lower = cause.to_string().to_ascii_lowercase();
        lower.contains("timeout")
            || lower.contains("timed out")
            || lower.contains("chip error 0x00000032")
            || lower.contains("operational discovery failed")
            || lower.contains("failed to connect")
    })
}

fn execute_step(
    backend: &dyn ChipControllerBackend,
    node_id: u64,
    endpoint: u16,
    step: &MatterCommandStep,
) -> Result<()> {
    match *step {
        MatterCommandStep::SetOnOff { on } => backend.set_on_off(node_id, endpoint, on),
        MatterCommandStep::Identify { duration_secs } => {
            backend.identify_light(node_id, endpoint, duration_secs)
        }
        MatterCommandStep::SetBrightness {
            level,
            transition_ms,
        } => backend.set_brightness(node_id, endpoint, level, transition_ms),
        MatterCommandStep::RunLevel {
            command,
            level_or_step,
            step_mode,
            transition_ms,
        } => backend.run_level_command(
            node_id,
            endpoint,
            command,
            level_or_step,
            step_mode,
            transition_ms,
        ),
        MatterCommandStep::SetColorTemperature {
            kelvin,
            transition_ms,
        } => backend.set_color_temperature(node_id, endpoint, kelvin, transition_ms),
        MatterCommandStep::SetXy {
            x,
            y,
            transition_ms,
        } => backend.set_xy(node_id, endpoint, x, y, transition_ms),
        MatterCommandStep::SetHueSaturation {
            hue,
            saturation,
            transition_ms,
        } => backend.set_hue_saturation(node_id, endpoint, hue, saturation, transition_ms),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    use rhythm_matter::chip_rpc::ChipInitControllerResponse;
    use rhythm_matter::transport::{
        CommissionedDevice, MatterAttributeReport, MatterCommissionRequest, MatterGroup,
        MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode,
        MatterSubscriptionTarget, MatterSubscriptionTermination,
    };

    use crate::service::CommissioningState;

    #[derive(Default)]
    struct BlockingState {
        release_first: AtomicBool,
        fail_next: AtomicBool,
        calls: AtomicUsize,
        active_calls: AtomicUsize,
        max_active_calls: AtomicUsize,
        blocked_endpoints: AtomicUsize,
        healthy_calls: AtomicUsize,
        second_endpoint_ran: AtomicBool,
        failed_nodes: Mutex<HashSet<u64>>,
        fail_color: AtomicBool,
        fail_color_with_timeout: AtomicBool,
        fail_level: AtomicBool,
        level_calls: AtomicUsize,
    }

    struct BlockingBackend {
        state: Arc<BlockingState>,
    }

    impl BlockingBackend {
        fn new(state: Arc<BlockingState>) -> Self {
            Self { state }
        }

        fn fail_if_color_rejected(&self) -> Result<()> {
            if self.state.fail_color_with_timeout.load(Ordering::SeqCst) {
                anyhow::bail!(
                    "setting Matter color temperature: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout"
                );
            }
            if self.state.fail_color.load(Ordering::SeqCst) {
                anyhow::bail!("synthetic UNSUPPORTED_COMMAND for color step");
            }
            Ok(())
        }
    }

    impl ChipControllerBackend for BlockingBackend {
        fn init_controller(
            &mut self,
            _: &CommissioningState,
            _: Option<u16>,
            _: &[CommissionedDevice],
        ) -> Result<ChipInitControllerResponse> {
            unreachable!()
        }
        fn commission_light(&self, _: &MatterCommissionRequest) -> Result<CommissionedDevice> {
            unreachable!()
        }
        fn probe_light(&self, _: u64) -> Result<CommissionedDevice> {
            unreachable!()
        }
        fn decommission_device(&self, _: u64, _: bool) -> Result<()> {
            Ok(())
        }
        fn set_on_off(&self, node_id: u64, endpoint: u16, _: bool) -> Result<()> {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            let active = self.state.active_calls.fetch_add(1, Ordering::SeqCst) + 1;
            self.state
                .max_active_calls
                .fetch_max(active, Ordering::SeqCst);
            if self.state.fail_next.swap(false, Ordering::SeqCst) {
                self.state.active_calls.fetch_sub(1, Ordering::SeqCst);
                anyhow::bail!("synthetic endpoint failure");
            }
            let blocked_through = self.state.blocked_endpoints.load(Ordering::SeqCst);
            let should_block = if blocked_through == 0 {
                endpoint == 1
            } else {
                usize::from(endpoint) <= blocked_through
            };
            if should_block && !self.state.release_first.load(Ordering::SeqCst) {
                while !self.state.release_first.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
            } else if blocked_through > 0 {
                self.state.healthy_calls.fetch_add(1, Ordering::SeqCst);
            }
            if endpoint == 2 {
                self.state.second_endpoint_ran.store(true, Ordering::SeqCst);
            }
            self.state.active_calls.fetch_sub(1, Ordering::SeqCst);
            if self.state.failed_nodes.lock().unwrap().contains(&node_id) {
                anyhow::bail!("failed to connect to synthetic offline endpoint {node_id}");
            }
            Ok(())
        }
        fn configure_group(&self, _: &MatterGroup) -> Result<()> {
            Ok(())
        }
        fn remove_group(&self, _: u16, _: &[MatterGroupMember]) -> Result<()> {
            Ok(())
        }
        fn set_group_on_off(&self, _: u16, _: bool) -> Result<()> {
            Ok(())
        }
        fn identify_group(&self, _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        fn set_group_brightness(&self, _: u16, _: u8, _: Option<u32>) -> Result<()> {
            Ok(())
        }
        fn set_group_color_temperature(&self, _: u16, _: u16, _: Option<u32>) -> Result<()> {
            Ok(())
        }
        fn set_group_xy(&self, _: u16, _: f32, _: f32, _: Option<u32>) -> Result<()> {
            Ok(())
        }
        fn set_group_hue_saturation(&self, _: u16, _: u8, _: u8, _: Option<u32>) -> Result<()> {
            Ok(())
        }
        fn identify_light(&self, _: u64, _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        fn set_brightness(&self, _: u64, _: u16, _: u8, _: Option<u32>) -> Result<()> {
            self.state.level_calls.fetch_add(1, Ordering::SeqCst);
            if self.state.fail_level.load(Ordering::SeqCst) {
                anyhow::bail!("synthetic level failure");
            }
            Ok(())
        }
        fn run_level_command(
            &self,
            _: u64,
            _: u16,
            _: MatterLevelCommandVariant,
            _: u8,
            _: Option<MatterLevelStepMode>,
            _: Option<u32>,
        ) -> Result<()> {
            Ok(())
        }
        fn set_color_temperature(&self, _: u64, _: u16, _: u16, _: Option<u32>) -> Result<()> {
            self.fail_if_color_rejected()
        }
        fn set_xy(&self, _: u64, _: u16, _: f32, _: f32, _: Option<u32>) -> Result<()> {
            self.fail_if_color_rejected()
        }
        fn set_hue_saturation(&self, _: u64, _: u16, _: u8, _: u8, _: Option<u32>) -> Result<()> {
            self.fail_if_color_rejected()
        }
        fn read_on_off(&self, _: u64, _: u16) -> Result<bool> {
            Ok(false)
        }
        fn read_light_capability_snapshot(&self, _: u64, _: u16) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn read_light_state(&self, _: u64, _: u16) -> Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
        fn subscribe_on_off(&self, _: &[MatterSubscriptionTarget], _: u16, _: u16) -> Result<()> {
            Ok(())
        }
        fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
            Ok(Vec::new())
        }
        fn drain_subscription_terminations(&self) -> Result<Vec<MatterSubscriptionTermination>> {
            Ok(Vec::new())
        }
    }

    fn plan(command_id: u64, endpoint: u16, on: bool) -> MatterEndpointCommandPlan {
        plan_for(command_id, 9, endpoint, on)
    }

    fn plan_for(
        command_id: u64,
        node_id: u64,
        endpoint: u16,
        on: bool,
    ) -> MatterEndpointCommandPlan {
        MatterEndpointCommandPlan {
            command_id,
            node_id,
            endpoint,
            steps: vec![MatterCommandStep::SetOnOff { on }],
            inter_step_delay_ms: None,
        }
    }

    fn wait_for_outcomes(
        broker: &ControllerEventBroker,
        expected_command_ids: &[u64],
    ) -> Vec<(u64, MatterCommandOutcomeStatus)> {
        wait_for_outcome_details(broker, expected_command_ids)
            .into_iter()
            .map(|(command_id, status, _)| (command_id, status))
            .collect()
    }

    fn wait_for_outcome_details(
        broker: &ControllerEventBroker,
        expected_command_ids: &[u64],
    ) -> Vec<(u64, MatterCommandOutcomeStatus, Option<String>)> {
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut cursor: Option<MatterControllerEventCursor> = None;
        let mut outcomes: Vec<(u64, MatterCommandOutcomeStatus, Option<String>)> = Vec::new();

        while !expected_command_ids
            .iter()
            .all(|command_id| outcomes.iter().any(|outcome| outcome.0 == *command_id))
        {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            assert!(
                !remaining.is_zero(),
                "timed out waiting for command outcomes {expected_command_ids:?}; received {outcomes:?}"
            );
            let batch = broker.wait(cursor.as_ref(), remaining);
            let mut last_sequence = cursor
                .as_ref()
                .filter(|cursor| cursor.stream_id == batch.stream_id)
                .map(|cursor| cursor.sequence)
                .unwrap_or(0);
            for envelope in batch.events {
                last_sequence = last_sequence.max(envelope.sequence);
                if let MatterControllerEvent::CommandOutcome(outcome) = envelope.event {
                    outcomes.push((outcome.command_id, outcome.status, outcome.detail));
                }
            }
            cursor = Some(MatterControllerEventCursor {
                stream_id: batch.stream_id,
                sequence: last_sequence,
            });
        }

        outcomes
    }

    fn color_then_level_plan(command_id: u64, with_level: bool) -> MatterEndpointCommandPlan {
        let mut steps = vec![MatterCommandStep::SetHueSaturation {
            hue: 21,
            saturation: 165,
            transition_ms: None,
        }];
        if with_level {
            steps.push(MatterCommandStep::SetBrightness {
                level: 128,
                transition_ms: None,
            });
        }
        MatterEndpointCommandPlan {
            command_id,
            node_id: 9,
            endpoint: 1,
            steps,
            inter_step_delay_ms: None,
        }
    }

    #[test]
    fn rejected_color_step_does_not_block_the_level_step() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.fail_color.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        dispatcher
            .submit(vec![color_then_level_plan(20, true)])
            .unwrap();

        let outcomes = wait_for_outcome_details(&broker, &[20]);
        let (_, status, detail) = &outcomes[0];
        assert_eq!(*status, MatterCommandOutcomeStatus::Succeeded);
        assert!(
            detail
                .as_deref()
                .is_some_and(|detail| detail.contains("color step rejected")),
            "expected the color failure in the outcome detail, got {detail:?}"
        );
        assert_eq!(blocking.level_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn rejected_color_step_with_nothing_after_it_fails_the_plan() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.fail_color.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        dispatcher
            .submit(vec![color_then_level_plan(21, false)])
            .unwrap();

        let outcomes = wait_for_outcomes(&broker, &[21]);
        assert_eq!(outcomes, vec![(21, MatterCommandOutcomeStatus::Failed)]);
        assert_eq!(blocking.level_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn color_step_connectivity_failure_aborts_the_plan() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking
            .fail_color_with_timeout
            .store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        dispatcher
            .submit(vec![color_then_level_plan(22, true)])
            .unwrap();

        let outcomes = wait_for_outcomes(&broker, &[22]);
        assert_eq!(outcomes, vec![(22, MatterCommandOutcomeStatus::Failed)]);
        assert_eq!(blocking.level_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn later_step_failure_keeps_rejected_color_step_in_detail() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.fail_color.store(true, Ordering::SeqCst);
        blocking.fail_level.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking)) as Box<dyn ChipControllerBackend>
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        dispatcher
            .submit(vec![color_then_level_plan(23, true)])
            .unwrap();

        let outcomes = wait_for_outcome_details(&broker, &[23]);
        let (_, status, detail) = &outcomes[0];
        assert_eq!(*status, MatterCommandOutcomeStatus::Failed);
        let detail = detail.as_deref().unwrap_or_default();
        assert!(detail.contains("synthetic UNSUPPORTED_COMMAND"));
        assert!(detail.contains("after rejected color step"));
    }

    #[test]
    fn submissions_are_non_blocking_latest_wins_and_endpoint_isolated() {
        let blocking = Arc::new(BlockingState::default());
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend.clone(), broker.clone());

        dispatcher.record_endpoint_proof(9, 2);
        let started = std::time::Instant::now();
        let submissions = dispatcher.submit(vec![plan(1, 1, true)]).unwrap();
        assert!(started.elapsed() < Duration::from_millis(50));
        assert_eq!(
            submissions[0].controller_stream_id.as_deref(),
            Some(broker.stream_id())
        );
        dispatcher.submit(vec![plan(2, 1, false)]).unwrap();
        dispatcher.submit(vec![plan(3, 1, true)]).unwrap();
        dispatcher.submit(vec![plan(4, 2, true)]).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while !blocking.second_endpoint_ran.load(Ordering::SeqCst)
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert!(blocking.second_endpoint_ran.load(Ordering::SeqCst));

        blocking.release_first.store(true, Ordering::SeqCst);
        let outcomes = wait_for_outcomes(&broker, &[1, 2, 3, 4]);
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 3);
        assert!(outcomes.contains(&(2, MatterCommandOutcomeStatus::Superseded)));
        assert!(outcomes.contains(&(1, MatterCommandOutcomeStatus::Succeeded)));
        assert!(outcomes.contains(&(3, MatterCommandOutcomeStatus::Succeeded)));
        assert!(outcomes.contains(&(4, MatterCommandOutcomeStatus::Succeeded)));
    }

    #[test]
    fn failed_plan_releases_lane_without_a_retry_timer() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.fail_next.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        dispatcher.submit(vec![plan(10, 1, true)]).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while blocking.calls.load(Ordering::SeqCst) < 1 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        dispatcher.submit(vec![plan(11, 1, false)]).unwrap();
        let outcomes = wait_for_outcomes(&broker, &[10, 11]);
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);
        assert!(outcomes.contains(&(10, MatterCommandOutcomeStatus::Failed)));
        assert!(outcomes.contains(&(11, MatterCommandOutcomeStatus::Succeeded)));
    }

    #[test]
    fn twenty_two_endpoint_fanout_is_bounded_and_healthy_peers_make_progress() {
        let blocking = Arc::new(BlockingState::default());
        blocking.blocked_endpoints.store(2, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        let plans: Vec<_> = (1_u16..=22)
            .map(|endpoint| plan_for(u64::from(endpoint), u64::from(endpoint), endpoint, false))
            .collect();

        for endpoint in 3..=22 {
            dispatcher.record_endpoint_proof(u64::from(endpoint), endpoint);
        }
        dispatcher.submit(plans).unwrap();

        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while blocking.healthy_calls.load(Ordering::SeqCst) < 20
            && std::time::Instant::now() < deadline
        {
            std::thread::yield_now();
        }
        assert_eq!(
            blocking.healthy_calls.load(Ordering::SeqCst),
            20,
            "healthy endpoints must drain while unavailable peers await recovery"
        );
        assert!(
            blocking.max_active_calls.load(Ordering::SeqCst) <= MAX_CONCURRENT_CONTROLLER_WORK,
            "controller work exceeded the global endpoint budget"
        );
        assert_eq!(blocking.active_calls.load(Ordering::SeqCst), 1);

        blocking.release_first.store(true, Ordering::SeqCst);
        let command_ids: Vec<_> = (1_u64..=22).collect();
        let outcomes = wait_for_outcomes(&broker, &command_ids);
        assert_eq!(outcomes.len(), 22);
        assert!(outcomes
            .iter()
            .all(|(_, status)| *status == MatterCommandOutcomeStatus::Succeeded));
    }

    #[test]
    fn twenty_two_endpoint_fanout_survives_six_unavailable_peers() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.failed_nodes.lock().unwrap().extend(1_u64..=6_u64);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        let plans: Vec<_> = (1_u16..=22)
            .map(|endpoint| plan_for(u64::from(endpoint), u64::from(endpoint), endpoint, false))
            .collect();

        dispatcher.submit(plans).unwrap();
        let command_ids: Vec<_> = (1_u64..=22).collect();
        let outcomes = wait_for_outcomes(&broker, &command_ids);

        assert_eq!(outcomes.len(), 22);
        for failed in 1_u64..=6_u64 {
            assert!(outcomes.contains(&(failed, MatterCommandOutcomeStatus::Failed)));
        }
        for healthy in 7_u64..=22_u64 {
            assert!(outcomes.contains(&(healthy, MatterCommandOutcomeStatus::Succeeded)));
        }
        assert!(
            blocking.max_active_calls.load(Ordering::SeqCst) <= MAX_CONCURRENT_CONTROLLER_WORK,
            "six unavailable peers must not exceed the controller budget or starve healthy work"
        );
    }

    #[test]
    fn responsive_peer_completes_while_six_peers_are_still_in_discovery() {
        let blocking = Arc::new(BlockingState::default());
        blocking.blocked_endpoints.store(6, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        // Establish real success for the responsive peer before the outage fan-out.
        dispatcher.submit(vec![plan_for(100, 7, 7, true)]).unwrap();
        wait_for_outcomes(&broker, &[100]);
        dispatcher
            .submit((1..=6).map(|n| plan_for(n, n, n as u16, true)).collect())
            .unwrap();
        dispatcher.submit(vec![plan_for(200, 7, 7, false)]).unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut responsive_completed = false;
        while std::time::Instant::now() < deadline {
            responsive_completed = broker
                .wait(None, Duration::from_millis(10))
                .events
                .iter()
                .any(|e| {
                    matches!(&e.event, MatterControllerEvent::CommandOutcome(o)
                    if o.command_id == 200 && o.status == MatterCommandOutcomeStatus::Succeeded)
                });
            if responsive_completed {
                break;
            }
        }
        // Always release blocked fake I/O, including on the expected red run.
        blocking.release_first.store(true, Ordering::SeqCst);
        wait_for_outcomes(&broker, &[1, 2, 3, 4, 5, 6, 200]);
        assert!(
            responsive_completed,
            "responsive peer waited for unrelated discovery to finish"
        );
        assert!(blocking.max_active_calls.load(Ordering::SeqCst) <= MAX_CONCURRENT_CONTROLLER_WORK);
    }

    #[test]
    fn failed_peer_backs_off_until_new_proof_and_ignores_delayed_old_reports() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.failed_nodes.lock().unwrap().insert(1);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        let old_report = Instant::now() - Duration::from_secs(1);
        dispatcher.submit(vec![plan_for(1, 1, 1, true)]).unwrap();
        assert!(wait_for_outcomes(&broker, &[1]).contains(&(1, MatterCommandOutcomeStatus::Failed)));
        // A new desired state is terminally rejected during recovery, without
        // restarting a 45-second native discovery operation.
        dispatcher.record_endpoint_proof_at(1, 1, old_report);
        dispatcher.submit(vec![plan_for(2, 1, 1, false)]).unwrap();
        assert!(wait_for_outcomes(&broker, &[2]).contains(&(2, MatterCommandOutcomeStatus::Failed)));
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 1);
        // A real report clears cooldown immediately; the latest request reaches I/O.
        blocking.failed_nodes.lock().unwrap().clear();
        dispatcher.record_endpoint_proof(1, 1);
        dispatcher.submit(vec![plan_for(3, 1, 1, false)]).unwrap();
        assert!(
            wait_for_outcomes(&broker, &[3]).contains(&(3, MatterCommandOutcomeStatus::Succeeded))
        );
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn subscription_recovery_does_not_block_ready_commands_or_admit_cold_workers() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let budget = Arc::new(ControllerWorkBudget::new(4));
        let dispatcher =
            CommandDispatcher::with_work_budget(backend, broker.clone(), budget.clone());
        dispatcher.submit(vec![plan_for(100, 7, 7, true)]).unwrap();
        wait_for_outcomes(&broker, &[100]);
        let subscription = budget.acquire();
        dispatcher
            .submit((1..=6).map(|n| plan_for(n, n, n as u16, true)).collect())
            .unwrap();
        dispatcher.submit(vec![plan_for(200, 7, 7, false)]).unwrap();
        assert!(wait_for_outcomes(&broker, &[200])
            .contains(&(200, MatterCommandOutcomeStatus::Succeeded)));
        assert_eq!(
            blocking.calls.load(Ordering::SeqCst),
            2,
            "cold commands entered controller work during subscription recovery"
        );
        drop(subscription);
        // Service wakeup must run on subscription failure as well as success.
        dispatcher.start_ready_lanes();
        wait_for_outcomes(&broker, &[1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn stale_in_flight_timeout_cannot_erase_newer_endpoint_proof() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        let old_attempt = Instant::now() - Duration::from_secs(1);
        dispatcher.record_endpoint_proof(1, 1);
        dispatcher.record_connectivity_failure(1, 1, old_attempt);
        dispatcher.submit(vec![plan_for(1, 1, 1, true)]).unwrap();
        assert!(
            wait_for_outcomes(&broker, &[1]).contains(&(1, MatterCommandOutcomeStatus::Succeeded))
        );
    }

    #[test]
    fn expired_cooldown_allows_recovery_without_a_subscription_report() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        blocking.failed_nodes.lock().unwrap().insert(1);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());
        dispatcher.submit(vec![plan_for(1, 1, 1, true)]).unwrap();
        wait_for_outcomes(&broker, &[1]);
        // Advance only this endpoint's retry deadline; avoid wall-clock sleeps.
        dispatcher
            .state
            .lock()
            .unwrap()
            .health
            .get_mut(&(1, 1))
            .unwrap()
            .retry_at = Some(Instant::now() - Duration::from_secs(1));
        blocking.failed_nodes.lock().unwrap().clear();
        dispatcher.submit(vec![plan_for(2, 1, 1, false)]).unwrap();
        assert!(
            wait_for_outcomes(&broker, &[2]).contains(&(2, MatterCommandOutcomeStatus::Succeeded))
        );
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn event_stream_exposes_restart_identity_and_retention_gaps() {
        let broker = ControllerEventBroker::new();
        for command_id in 1..=(EVENT_CAPACITY as u64 + 1) {
            broker.publish(MatterControllerEvent::CommandOutcome(
                MatterCommandOutcome {
                    command_id,
                    node_id: 1,
                    endpoint: 1,
                    status: MatterCommandOutcomeStatus::Succeeded,
                    completed_at_unix_ms: None,
                    detail: None,
                    failure_class: None,
                },
            ));
        }
        let batch = broker.wait(None, Duration::ZERO);
        assert_eq!(batch.oldest_sequence, 2);
        assert_eq!(batch.events.len(), EVENT_CAPACITY);

        let restarted = ControllerEventBroker::new();
        assert_ne!(
            batch.stream_id,
            restarted.wait(None, Duration::ZERO).stream_id
        );
    }

    #[test]
    fn lane_admission_derives_from_the_shared_work_budget() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let work_budget = Arc::new(ControllerWorkBudget::new(2));
        assert_eq!(work_budget.capacity(), 2);
        let dispatcher =
            CommandDispatcher::with_work_budget(backend, broker.clone(), work_budget.clone());

        let plans: Vec<_> = (1_u16..=8)
            .map(|endpoint| plan_for(u64::from(endpoint), u64::from(endpoint), endpoint, false))
            .collect();
        dispatcher.submit(plans).unwrap();

        let command_ids: Vec<_> = (1_u64..=8).collect();
        let outcomes = wait_for_outcomes(&broker, &command_ids);
        assert_eq!(outcomes.len(), 8);
        assert!(
            blocking.max_active_calls.load(Ordering::SeqCst) <= work_budget.capacity(),
            "lane admission must follow the shared budget capacity, not a separate constant"
        );
    }

    #[test]
    fn releasing_a_lane_requeues_pending_work_without_admitting_it() {
        let blocking = Arc::new(BlockingState::default());
        blocking.release_first.store(true, Ordering::SeqCst);
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend, broker.clone());

        let running = plan(1, 3, true);
        let queued = plan(2, 3, false);
        let key = (running.node_id, running.endpoint);
        {
            let mut state = dispatcher
                .state
                .lock()
                .expect("chipd dispatch lock poisoned");
            state.active_ids.insert(running.command_id);
            state.active_ids.insert(queued.command_id);
            state.slots.insert(
                key,
                EndpointSlot {
                    running: true,
                    pending: Some(queued.clone()),
                },
            );
            state.active_lanes = 1;
        }

        // The spawn-failure path: release the slot without re-entering
        // admission. Nothing may run as a side effect of releasing.
        dispatcher.release_lane_state(&running);

        {
            let state = dispatcher
                .state
                .lock()
                .expect("chipd dispatch lock poisoned");
            assert_eq!(state.active_lanes, 0);
            assert_eq!(state.ready.iter().copied().collect::<Vec<_>>(), vec![key]);
            assert!(!state.slots[&key].running);
            assert!(!state.active_ids.contains(&running.command_id));
        }
        assert_eq!(
            blocking.calls.load(Ordering::SeqCst),
            0,
            "releasing a lane must not admit work"
        );

        dispatcher.start_ready_lanes();
        let outcomes = wait_for_outcomes(&broker, &[2]);
        assert!(outcomes.contains(&(2, MatterCommandOutcomeStatus::Succeeded)));
    }
}
