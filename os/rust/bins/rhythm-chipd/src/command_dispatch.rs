use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::Result;

use rhythm_matter::transport::{
    MatterCommandOutcome, MatterCommandOutcomeStatus, MatterCommandStep, MatterCommandSubmission,
    MatterControllerEvent, MatterControllerEventBatch, MatterControllerEventCursor,
    MatterControllerEventEnvelope, MatterEndpointCommandPlan,
};

use crate::backend::ChipControllerBackend;

const EVENT_CAPACITY: usize = 2_048;
const MAX_EVENT_WAIT: Duration = Duration::from_secs(30);
/// One shared controller work budget governs every entry into the CHIP
/// controller: command steps *and* subscribe attempts. It is the only cap —
/// lane admission derives from it, so there is no second constant to drift.
pub(crate) const MAX_CONCURRENT_CONTROLLER_WORK: usize = 4;
static NEXT_STREAM_NONCE: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct WorkBudgetState {
    next_ticket: u64,
    serving_ticket: u64,
    in_flight: usize,
}

/// Fair controller-wide budget around endpoint discovery, CASE setup, and
/// interaction work. Endpoint lanes remain isolated, but unavailable peers
/// cannot all enter the constrained CHIP controller at once.
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

    pub(crate) fn acquire(&self) -> ControllerWorkPermit<'_> {
        let mut state = self.state.lock().expect("chipd work budget lock poisoned");
        let ticket = state.next_ticket;
        state.next_ticket = state.next_ticket.saturating_add(1);
        while ticket != state.serving_ticket || state.in_flight >= self.capacity {
            state = self
                .changed
                .wait(state)
                .expect("chipd work budget lock poisoned while waiting");
        }
        state.serving_ticket = state.serving_ticket.saturating_add(1);
        state.in_flight += 1;
        self.changed.notify_all();
        ControllerWorkPermit { budget: self }
    }

    /// Concurrent controller work this budget admits. Lane admission uses it so
    /// the dispatcher can never run more lanes than the budget will serve.
    pub(crate) fn capacity(&self) -> usize {
        self.capacity
    }
}

pub(crate) struct ControllerWorkPermit<'a> {
    budget: &'a ControllerWorkBudget,
}

impl Drop for ControllerWorkPermit<'_> {
    fn drop(&mut self) {
        let mut state = self
            .budget
            .state
            .lock()
            .expect("chipd work budget lock poisoned while releasing");
        state.in_flight = state.in_flight.saturating_sub(1);
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
            self.publish_outcome(&plan, MatterCommandOutcomeStatus::Superseded, None);
        }
        self.start_ready_lanes();
        Ok(submissions)
    }

    fn spawn_lane(self: &Arc<Self>, plan: MatterEndpointCommandPlan) -> Result<()> {
        let dispatcher = self.clone();
        std::thread::Builder::new()
            .name(format!("chipd-matter-{}-{}", plan.node_id, plan.endpoint))
            .spawn(move || dispatcher.run_lane(plan))?;
        Ok(())
    }

    /// Admit ready endpoints until the shared budget is full.
    ///
    /// Single loop, no recursion: a lane that fails to spawn publishes its
    /// failure and releases its slot through `release_lane_state`, which never
    /// admits work itself, so this loop stays the only admission path.
    fn start_ready_lanes(self: &Arc<Self>) {
        let capacity = self.work_budget.capacity();
        loop {
            let Some(plan) = ({
                let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
                if state.active_lanes >= capacity {
                    None
                } else {
                    let mut next = None;
                    while let Some(key) = state.ready.pop_front() {
                        let Some(slot) = state.slots.get_mut(&key) else {
                            continue;
                        };
                        if slot.running {
                            continue;
                        }
                        let Some(plan) = slot.pending.take() else {
                            continue;
                        };
                        slot.running = true;
                        state.active_lanes += 1;
                        next = Some(plan);
                        break;
                    }
                    next
                }
            }) else {
                return;
            };

            if let Err(error) = self.spawn_lane(plan.clone()) {
                self.publish_outcome(
                    &plan,
                    MatterCommandOutcomeStatus::Failed,
                    Some(format!("starting endpoint worker: {error:#}")),
                );
                self.release_lane_state(&plan);
                continue;
            }
        }
    }

    /// Run one plan to completion.
    ///
    /// There is no host wall-clock deadline here: a single unavailable bulb can
    /// legitimately hold its lane for the SDK's own operational-discovery and
    /// CASE timeouts, and killing chipd for that would restart every healthy
    /// endpoint too. The bounds that matter are the SDK's, the native bridge's
    /// last-resort wedge guard, and the client RPC timeout.
    fn run_lane(self: Arc<Self>, plan: MatterEndpointCommandPlan) {
        let result = self.execute(&plan);
        match result {
            Ok(()) => self.publish_outcome(&plan, MatterCommandOutcomeStatus::Succeeded, None),
            Err(error) => self.publish_outcome(
                &plan,
                MatterCommandOutcomeStatus::Failed,
                Some(format!("{error:#}")),
            ),
        }
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

    fn execute(&self, plan: &MatterEndpointCommandPlan) -> Result<()> {
        for (index, step) in plan.steps.iter().enumerate() {
            // The shared controller budget is held for exactly one step, so a
            // slow endpoint cannot hold a permit across its inter-step delay.
            let permit = self.work_budget.acquire();
            let backend = self.backend.read().expect("chipd backend lock poisoned");
            let step_result = execute_step(backend.as_ref(), plan.node_id, plan.endpoint, step);
            drop(backend);
            drop(permit);
            step_result?;

            if index + 1 < plan.steps.len() {
                if let Some(delay_ms) = plan.inter_step_delay_ms.filter(|delay| *delay > 0) {
                    std::thread::sleep(Duration::from_millis(delay_ms));
                }
            }
        }
        Ok(())
    }

    fn publish_outcome(
        &self,
        plan: &MatterEndpointCommandPlan,
        status: MatterCommandOutcomeStatus,
        detail: Option<String>,
    ) {
        self.broker.publish(MatterControllerEvent::CommandOutcome(
            MatterCommandOutcome {
                command_id: plan.command_id,
                node_id: plan.node_id,
                endpoint: plan.endpoint,
                status,
                detail,
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
    }

    struct BlockingBackend {
        state: Arc<BlockingState>,
    }

    impl BlockingBackend {
        fn new(state: Arc<BlockingState>) -> Self {
            Self { state }
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
                anyhow::bail!("synthetic offline endpoint {node_id}");
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
            Ok(())
        }
        fn set_xy(&self, _: u64, _: u16, _: f32, _: f32, _: Option<u32>) -> Result<()> {
            Ok(())
        }
        fn set_hue_saturation(&self, _: u64, _: u16, _: u8, _: u8, _: Option<u32>) -> Result<()> {
            Ok(())
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
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        let mut cursor: Option<MatterControllerEventCursor> = None;
        let mut outcomes: Vec<(u64, MatterCommandOutcomeStatus)> = Vec::new();

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
                    outcomes.push((outcome.command_id, outcome.status));
                }
            }
            cursor = Some(MatterControllerEventCursor {
                stream_id: batch.stream_id,
                sequence: last_sequence,
            });
        }

        outcomes
    }

    #[test]
    fn submissions_are_non_blocking_latest_wins_and_endpoint_isolated() {
        let blocking = Arc::new(BlockingState::default());
        let backend = Arc::new(RwLock::new(
            Box::new(BlockingBackend::new(blocking.clone())) as Box<dyn ChipControllerBackend>,
        ));
        let broker = Arc::new(ControllerEventBroker::new());
        let dispatcher = CommandDispatcher::new(backend.clone(), broker.clone());

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
            "healthy endpoints must drain while two unavailable peers retain their lanes"
        );
        assert!(
            blocking.max_active_calls.load(Ordering::SeqCst) <= MAX_CONCURRENT_CONTROLLER_WORK,
            "controller work exceeded the global endpoint budget"
        );
        assert_eq!(blocking.active_calls.load(Ordering::SeqCst), 2);

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
    fn event_stream_exposes_restart_identity_and_retention_gaps() {
        let broker = ControllerEventBroker::new();
        for command_id in 1..=(EVENT_CAPACITY as u64 + 1) {
            broker.publish(MatterControllerEvent::CommandOutcome(
                MatterCommandOutcome {
                    command_id,
                    node_id: 1,
                    endpoint: 1,
                    status: MatterCommandOutcomeStatus::Succeeded,
                    detail: None,
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
