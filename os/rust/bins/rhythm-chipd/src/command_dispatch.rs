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
static NEXT_STREAM_NONCE: AtomicU64 = AtomicU64::new(1);

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
    in_flight: bool,
    pending: Option<MatterEndpointCommandPlan>,
}

#[derive(Default)]
struct DispatchState {
    slots: HashMap<(u64, u16), EndpointSlot>,
    active_ids: HashSet<u64>,
}

/// Per-endpoint command executor. Exactly one plan can be in flight for an
/// endpoint; one newer desired state is retained and any older queued state is
/// terminally superseded. A slow endpoint never owns another endpoint's lane.
pub struct CommandDispatcher {
    backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
    broker: Arc<ControllerEventBroker>,
    state: Mutex<DispatchState>,
}

impl CommandDispatcher {
    pub fn new(
        backend: Arc<RwLock<Box<dyn ChipControllerBackend>>>,
        broker: Arc<ControllerEventBroker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            backend,
            broker,
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
        let mut starters = Vec::new();
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
                if slot.in_flight {
                    if let Some(previous) = slot.pending.replace(plan.clone()) {
                        state.active_ids.remove(&previous.command_id);
                        superseded.push(previous);
                    }
                } else {
                    slot.in_flight = true;
                    starters.push(plan.clone());
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
        for plan in starters {
            if let Err(error) = self.spawn_lane(plan.clone()) {
                self.fail_lane_to_spawn(plan, format!("starting endpoint worker: {error:#}"));
            }
        }
        Ok(submissions)
    }

    fn spawn_lane(self: &Arc<Self>, plan: MatterEndpointCommandPlan) -> Result<()> {
        let dispatcher = self.clone();
        std::thread::Builder::new()
            .name(format!("chipd-matter-{}-{}", plan.node_id, plan.endpoint))
            .spawn(move || dispatcher.run_lane(plan))?;
        Ok(())
    }

    fn fail_lane_to_spawn(
        self: &Arc<Self>,
        mut plan: MatterEndpointCommandPlan,
        mut detail: String,
    ) {
        loop {
            self.publish_outcome(
                &plan,
                MatterCommandOutcomeStatus::Failed,
                Some(detail.clone()),
            );
            let next = {
                let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
                state.active_ids.remove(&plan.command_id);
                let key = (plan.node_id, plan.endpoint);
                let Some(slot) = state.slots.get_mut(&key) else {
                    return;
                };
                match slot.pending.take() {
                    Some(next) => Some(next),
                    None => {
                        state.slots.remove(&key);
                        None
                    }
                }
            };
            let Some(next) = next else {
                return;
            };
            match self.spawn_lane(next.clone()) {
                Ok(()) => return,
                Err(error) => {
                    plan = next;
                    detail = format!("starting endpoint worker: {error:#}");
                }
            }
        }
    }

    fn run_lane(self: Arc<Self>, mut plan: MatterEndpointCommandPlan) {
        loop {
            let result = self.execute(&plan);
            match result {
                Ok(()) => self.publish_outcome(&plan, MatterCommandOutcomeStatus::Succeeded, None),
                Err(error) => self.publish_outcome(
                    &plan,
                    MatterCommandOutcomeStatus::Failed,
                    Some(format!("{error:#}")),
                ),
            }

            let next = {
                let mut state = self.state.lock().expect("chipd dispatch lock poisoned");
                state.active_ids.remove(&plan.command_id);
                let key = (plan.node_id, plan.endpoint);
                let Some(slot) = state.slots.get_mut(&key) else {
                    return;
                };
                match slot.pending.take() {
                    Some(next) => Some(next),
                    None => {
                        slot.in_flight = false;
                        state.slots.remove(&key);
                        None
                    }
                }
            };
            let Some(next) = next else {
                return;
            };
            plan = next;
        }
    }

    fn execute(&self, plan: &MatterEndpointCommandPlan) -> Result<()> {
        for (index, step) in plan.steps.iter().enumerate() {
            let backend = self.backend.read().expect("chipd backend lock poisoned");
            execute_step(backend.as_ref(), plan.node_id, plan.endpoint, step)?;
            drop(backend);

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
        MatterSubscriptionTarget,
    };

    use crate::service::CommissioningState;

    #[derive(Default)]
    struct BlockingState {
        release_first: AtomicBool,
        fail_next: AtomicBool,
        calls: AtomicUsize,
        second_endpoint_ran: AtomicBool,
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
        fn set_on_off(&self, _: u64, endpoint: u16, _: bool) -> Result<()> {
            self.state.calls.fetch_add(1, Ordering::SeqCst);
            if self.state.fail_next.swap(false, Ordering::SeqCst) {
                anyhow::bail!("synthetic endpoint failure");
            }
            if endpoint == 1 && !self.state.release_first.load(Ordering::SeqCst) {
                while !self.state.release_first.load(Ordering::SeqCst) {
                    std::thread::yield_now();
                }
            }
            if endpoint == 2 {
                self.state.second_endpoint_ran.store(true, Ordering::SeqCst);
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
    }

    fn plan(command_id: u64, endpoint: u16, on: bool) -> MatterEndpointCommandPlan {
        MatterEndpointCommandPlan {
            command_id,
            node_id: 9,
            endpoint,
            steps: vec![MatterCommandStep::SetOnOff { on }],
            inter_step_delay_ms: None,
        }
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
        let deadline = std::time::Instant::now() + Duration::from_secs(1);
        while blocking.calls.load(Ordering::SeqCst) < 3 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 3);

        let batch = broker.wait(None, Duration::ZERO);
        let outcomes: Vec<_> = batch
            .events
            .into_iter()
            .filter_map(|event| match event.event {
                MatterControllerEvent::CommandOutcome(outcome) => {
                    Some((outcome.command_id, outcome.status))
                }
                _ => None,
            })
            .collect();
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
        while blocking.calls.load(Ordering::SeqCst) < 2 && std::time::Instant::now() < deadline {
            std::thread::yield_now();
        }
        assert_eq!(blocking.calls.load(Ordering::SeqCst), 2);

        let batch = broker.wait(None, Duration::ZERO);
        let outcomes: Vec<_> = batch
            .events
            .into_iter()
            .filter_map(|event| match event.event {
                MatterControllerEvent::CommandOutcome(outcome) => {
                    Some((outcome.command_id, outcome.status))
                }
                _ => None,
            })
            .collect();
        assert!(outcomes.contains(&(10, MatterCommandOutcomeStatus::Failed)));
        assert!(outcomes.contains(&(11, MatterCommandOutcomeStatus::Succeeded)));
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
}
