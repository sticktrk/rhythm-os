//! Rhythm adaptive light runtime.
//!
//! This crate hosts Rhythm's existing adaptive lighting behavior behind the
//! neutral `rhythm-runtime-api` contract. The heavy engine primitives still
//! live in `rhythm-core`; this wrapper is the light runtime boundary.

use std::sync::Arc;

use rhythm_core::{
    button_action_from_runtime_input, controller::LightController, runtime::DeviceRegistry,
    runtime::Scheduler, runtime::TimeProvider, InputEvent, RhythmInputPlanOutcome,
    RhythmPeriodicPlanOutcome, RhythmRuntime, RuntimeConfig, RuntimeError as CoreRuntimeError,
    RuntimeHandle,
};
use rhythm_runtime_api::{
    LightRuntime, RuntimeCapabilities, RuntimeError, RuntimeEvent, RuntimeManifest, RuntimePlan,
    RuntimeResult, RuntimeSnapshot,
};

pub const RUNTIME_ID: &str = "rhythm-adaptive";

pub fn runtime_manifest() -> RuntimeManifest {
    RuntimeManifest::new(RUNTIME_ID, "Rhythm Adaptive")
        .with_version(env!("CARGO_PKG_VERSION"))
        .with_description("Default adaptive light runtime backed by the Rhythm core engine.")
        .with_capabilities(RuntimeCapabilities::light_runtime())
}

pub struct RhythmAdaptiveRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    inner: RhythmRuntime<C, T, S, R>,
}

impl<C, T, S, R> RhythmAdaptiveRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    pub fn new(
        controller: Arc<C>,
        time_provider: T,
        scheduler: S,
        device_registry: R,
        config: RuntimeConfig,
    ) -> Self {
        Self {
            inner: RhythmRuntime::new(
                controller,
                time_provider,
                scheduler,
                device_registry,
                config,
            ),
        }
    }

    pub fn from_runtime(runtime: RhythmRuntime<C, T, S, R>) -> Self {
        Self { inner: runtime }
    }

    pub fn inner(&self) -> &RhythmRuntime<C, T, S, R> {
        &self.inner
    }

    pub fn inner_mut(&mut self) -> &mut RhythmRuntime<C, T, S, R> {
        &mut self.inner
    }

    pub fn into_inner(self) -> RhythmRuntime<C, T, S, R> {
        self.inner
    }
}

impl<C, T, S, R> LightRuntime for RhythmAdaptiveRuntime<C, T, S, R>
where
    C: LightController + Send + Sync + 'static,
    T: TimeProvider + Send + Sync + 'static,
    S: Scheduler + Send + Sync + 'static,
    R: DeviceRegistry + Send + Sync + 'static,
{
    fn name(&self) -> &str {
        RUNTIME_ID
    }

    fn runtime_id(&self) -> &str {
        RUNTIME_ID
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::light_runtime()
    }

    fn manifest(&self) -> RuntimeManifest {
        runtime_manifest()
    }

    fn handle_event(
        &mut self,
        snapshot: &RuntimeSnapshot,
        event: RuntimeEvent,
    ) -> RuntimeResult<RuntimePlan> {
        match event {
            RuntimeEvent::Input(input) => {
                let action = button_action_from_runtime_input(&input.action).ok_or_else(|| {
                    RuntimeError::InvalidEvent(format!(
                        "unsupported Rhythm input action: {:?}",
                        input.action
                    ))
                })?;
                let event = InputEvent {
                    room_id: input.target_id.clone(),
                    action,
                    device_id: Some(input.source_id),
                };
                match self
                    .inner
                    .plan_input_event(&event, snapshot_power(snapshot, &event.room_id))
                    .map_err(map_core_runtime_error)?
                {
                    RhythmInputPlanOutcome::RequiresLightCheck => Err(RuntimeError::Runtime(
                        format!("snapshot is missing power state for '{}'", event.room_id),
                    )),
                    RhythmInputPlanOutcome::Plan { plan, .. } => Ok(plan),
                }
            }
            RuntimeEvent::PeriodicTick(tick) => {
                let source_node_id = tick
                    .metadata
                    .get("source_node_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or(&tick.node_id)
                    .to_string();
                match self
                    .inner
                    .plan_periodic_node_tick(
                        &tick,
                        &source_node_id,
                        snapshot_power(snapshot, &tick.node_id),
                    )
                    .map_err(map_core_runtime_error)?
                {
                    RhythmPeriodicPlanOutcome::RequiresLightCheck => Err(RuntimeError::Runtime(
                        format!("snapshot is missing power state for '{}'", tick.node_id),
                    )),
                    RhythmPeriodicPlanOutcome::Plan { plan, .. } => Ok(plan),
                }
            }
            RuntimeEvent::HostStateChanged { .. } => Ok(RuntimePlan::noop()),
        }
    }
}

fn snapshot_power(snapshot: &RuntimeSnapshot, node_id: &str) -> Option<bool> {
    snapshot.state.get(node_id).map(|state| state.power_on)
}

fn map_core_runtime_error(error: CoreRuntimeError) -> RuntimeError {
    RuntimeError::Runtime(error.to_string())
}

/// `LightRuntime` adapter over the live OS `RuntimeHandle`.
///
/// This is the production-facing Rhythm light runtime boundary: the OS keeps
/// owning hub controllers and engine state, while the light runtime call site
/// receives neutral snapshots/events and gets back neutral plans.
pub struct RuntimeHandleAdaptiveRuntime {
    inner: Arc<dyn RuntimeHandle>,
}

impl RuntimeHandleAdaptiveRuntime {
    pub fn new(runtime: Arc<dyn RuntimeHandle>) -> Self {
        Self { inner: runtime }
    }

    pub fn inner(&self) -> &Arc<dyn RuntimeHandle> {
        &self.inner
    }
}

impl LightRuntime for RuntimeHandleAdaptiveRuntime {
    fn name(&self) -> &str {
        RUNTIME_ID
    }

    fn runtime_id(&self) -> &str {
        RUNTIME_ID
    }

    fn capabilities(&self) -> RuntimeCapabilities {
        RuntimeCapabilities::light_runtime()
    }

    fn manifest(&self) -> RuntimeManifest {
        runtime_manifest()
    }

    fn handle_event(
        &mut self,
        snapshot: &RuntimeSnapshot,
        event: RuntimeEvent,
    ) -> RuntimeResult<RuntimePlan> {
        match event {
            RuntimeEvent::Input(input) => {
                let action = button_action_from_runtime_input(&input.action).ok_or_else(|| {
                    RuntimeError::InvalidEvent(format!(
                        "unsupported Rhythm input action: {:?}",
                        input.action
                    ))
                })?;
                let event = InputEvent {
                    room_id: input.target_id.clone(),
                    action,
                    device_id: Some(input.source_id),
                };
                match self
                    .inner
                    .plan_input_event(&event, snapshot_power(snapshot, &event.room_id))
                    .map_err(map_handle_error)?
                {
                    RhythmInputPlanOutcome::RequiresLightCheck => {
                        let lights_on = self
                            .inner
                            .any_lights_on(&event.room_id)
                            .map_err(map_handle_error)?;
                        match self
                            .inner
                            .plan_input_event(&event, Some(lights_on))
                            .map_err(map_handle_error)?
                        {
                            RhythmInputPlanOutcome::RequiresLightCheck => {
                                Err(RuntimeError::Runtime(format!(
                                    "runtime still requires light check for '{}'",
                                    event.room_id
                                )))
                            }
                            RhythmInputPlanOutcome::Plan { plan, .. } => Ok(plan),
                        }
                    }
                    RhythmInputPlanOutcome::Plan { plan, .. } => Ok(plan),
                }
            }
            RuntimeEvent::PeriodicTick(tick) => {
                let source_node_id = tick
                    .metadata
                    .get("source_node_id")
                    .and_then(|value| value.as_str())
                    .unwrap_or(&tick.node_id)
                    .to_string();
                match self
                    .inner
                    .plan_periodic_node_tick(
                        &tick,
                        &source_node_id,
                        snapshot_power(snapshot, &tick.node_id),
                    )
                    .map_err(map_handle_error)?
                {
                    RhythmPeriodicPlanOutcome::RequiresLightCheck => {
                        let lights_on = self
                            .inner
                            .any_lights_on(&tick.node_id)
                            .map_err(map_handle_error)?;
                        match self
                            .inner
                            .plan_periodic_node_tick(&tick, &source_node_id, Some(lights_on))
                            .map_err(map_handle_error)?
                        {
                            RhythmPeriodicPlanOutcome::RequiresLightCheck => {
                                Err(RuntimeError::Runtime(format!(
                                    "runtime still requires light check for '{}'",
                                    tick.node_id
                                )))
                            }
                            RhythmPeriodicPlanOutcome::Plan { plan, .. } => Ok(plan),
                        }
                    }
                    RhythmPeriodicPlanOutcome::Plan { plan, .. } => Ok(plan),
                }
            }
            RuntimeEvent::HostStateChanged { .. } => Ok(RuntimePlan::noop()),
        }
    }
}

fn map_handle_error(error: anyhow::Error) -> RuntimeError {
    RuntimeError::Runtime(error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_core::runtime::registry::SimpleDeviceRegistry;
    use rhythm_core::runtime::scheduler::NoOpScheduler;
    use rhythm_core::runtime::time::MockTimeProvider;
    use rhythm_core::NoOpController;
    use rhythm_runtime_api::{
        DispatchCommand, DispatchTarget, InputAction, NodeState, RuntimeInputEvent, RuntimeNode,
        RuntimeNodeKind,
    };

    fn test_runtime(
    ) -> RhythmAdaptiveRuntime<NoOpController, MockTimeProvider, NoOpScheduler, SimpleDeviceRegistry>
    {
        RhythmAdaptiveRuntime::new(
            Arc::new(NoOpController::new()),
            MockTimeProvider::default(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        )
    }

    #[test]
    fn rhythm_adaptive_runtime_returns_neutral_plan() {
        let mut runtime = test_runtime();
        let snapshot = RuntimeSnapshot {
            nodes: vec![RuntimeNode {
                id: "room-a".to_string(),
                name: "Room A".to_string(),
                kind: RuntimeNodeKind::Area,
                parent_id: None,
                properties: Default::default(),
            }],
            state: [(
                "room-a".to_string(),
                NodeState {
                    power_on: false,
                    rhythm_enabled: true,
                    brightness: None,
                    kelvin: None,
                    properties: Default::default(),
                },
            )]
            .into_iter()
            .collect(),
        };

        let plan = runtime
            .handle_event(
                &snapshot,
                RuntimeEvent::Input(RuntimeInputEvent {
                    source_id: "switch-a".to_string(),
                    target_id: "room-a".to_string(),
                    action: InputAction::On,
                    epoch_ms: None,
                    metadata: Default::default(),
                }),
            )
            .unwrap();

        assert!(matches!(
            plan.dispatch.first(),
            Some(DispatchCommand::TurnOn {
                target: DispatchTarget::Node { node_id },
                ..
            }) if node_id == "room-a"
        ));
    }
}
