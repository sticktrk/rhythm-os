//! Host adapter for plan-based light runtimes.
//!
//! `rhythm-os` owns topology, dispatch, persistence, HTTP/auth, and logging.
//! Light runtimes own behavior: they receive a neutral snapshot/event and
//! return a neutral plan for the host to apply.

use anyhow::Result;
use rhythm_core::{
    core_lighting_command_from_runtime, runtime_node_kind_from_light_node_kind,
    RhythmDispatchRecord, RhythmPeriodicPlanOutcome, RuntimeHandle,
};
use rhythm_runtime_api::{
    DiagnosticLevel, DispatchCommand, DispatchTarget, LightRuntime, NodeState, RuntimeEvent,
    RuntimeExtensionRequest, RuntimeExtensionResponse, RuntimeManifest, RuntimeNode, RuntimePlan,
    RuntimeSnapshot, StateWrite, TickContext,
};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::json;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::state::AppState;
use crate::state::SharedState;
use crate::storage::StoredLightRuntimeState;

pub const RHYTHM_ADAPTIVE_RUNTIME_ID: &str = "rhythm-adaptive";

pub type SharedLightRuntime = Arc<Mutex<Box<dyn LightRuntime>>>;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct LightRuntimeKind(String);

impl LightRuntimeKind {
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }

    pub fn new(runtime_id: impl Into<String>) -> Self {
        Self(runtime_id.into())
    }

    fn from_canonical_id(runtime_id: &str) -> Self {
        Self(runtime_id.to_string())
    }

    pub fn rhythm_adaptive() -> Self {
        Self::from_canonical_id(RHYTHM_ADAPTIVE_RUNTIME_ID)
    }
}

impl Default for LightRuntimeKind {
    fn default() -> Self {
        Self::from_canonical_id(RHYTHM_ADAPTIVE_RUNTIME_ID)
    }
}

impl FromStr for LightRuntimeKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            RHYTHM_ADAPTIVE_RUNTIME_ID | "rhythm" | "rhythm_adaptive" => {
                Ok(Self::from_canonical_id(RHYTHM_ADAPTIVE_RUNTIME_ID))
            }
            other if is_valid_runtime_id(other) => Ok(Self::new(other)),
            _ => Err(anyhow::anyhow!("invalid light runtime id '{}'", value)),
        }
    }
}

impl Serialize for LightRuntimeKind {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for LightRuntimeKind {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

#[derive(Clone, Copy)]
pub struct LightRuntimeModule {
    pub id: &'static str,
    pub aliases: &'static [&'static str],
    pub manifest: fn() -> RuntimeManifest,
    pub instance: LightRuntimeInstance,
}

impl LightRuntimeModule {
    pub const fn ephemeral(
        id: &'static str,
        aliases: &'static [&'static str],
        manifest: fn() -> RuntimeManifest,
        create: fn(Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime>,
    ) -> Self {
        Self {
            id,
            aliases,
            manifest,
            instance: LightRuntimeInstance::Ephemeral { create },
        }
    }

    pub const fn cached(
        id: &'static str,
        aliases: &'static [&'static str],
        manifest: fn() -> RuntimeManifest,
        ensure: fn(&SharedState, &LightRuntimeModule) -> Result<SharedLightRuntime>,
    ) -> Self {
        Self {
            id,
            aliases,
            manifest,
            instance: LightRuntimeInstance::Cached { ensure },
        }
    }

    pub fn runtime_kind(&self) -> LightRuntimeKind {
        LightRuntimeKind::from_canonical_id(self.id)
    }

    pub fn manifest(&self) -> RuntimeManifest {
        (self.manifest)()
    }
}

#[derive(Clone, Copy)]
pub enum LightRuntimeInstance {
    Ephemeral {
        create: fn(Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime>,
    },
    Cached {
        ensure: fn(&SharedState, &LightRuntimeModule) -> Result<SharedLightRuntime>,
    },
}

#[derive(Clone, Default)]
pub struct LightRuntimeRegistry {
    modules: BTreeMap<String, LightRuntimeModule>,
    aliases: BTreeMap<String, String>,
    order: Vec<String>,
}

impl LightRuntimeRegistry {
    pub fn register(&mut self, module: LightRuntimeModule) -> Result<()> {
        validate_runtime_id(module.id)?;
        let manifest = module.manifest();
        if manifest.id != module.id {
            return Err(anyhow::anyhow!(
                "light runtime module '{}' returned manifest id '{}'",
                module.id,
                manifest.id
            ));
        }
        if self.modules.contains_key(module.id) {
            return Err(anyhow::anyhow!(
                "light runtime module '{}' is already registered",
                module.id
            ));
        }

        self.aliases
            .insert(module.id.to_string(), module.id.to_string());
        for alias in module.aliases {
            validate_runtime_alias(alias)?;
            if self.modules.contains_key(*alias) || self.aliases.contains_key(*alias) {
                return Err(anyhow::anyhow!(
                    "light runtime alias '{}' is already registered",
                    alias
                ));
            }
            self.aliases
                .insert((*alias).to_string(), module.id.to_string());
        }
        self.modules.insert(module.id.to_string(), module);
        self.order.push(module.id.to_string());
        Ok(())
    }

    pub fn resolve_id<'a>(&'a self, runtime_id: &'a str) -> Option<&'a str> {
        self.aliases
            .get(runtime_id)
            .map(String::as_str)
            .or_else(|| self.modules.contains_key(runtime_id).then_some(runtime_id))
    }

    pub fn parse_runtime_kind(&self, runtime_id: &str) -> Result<LightRuntimeKind> {
        if let Some(canonical) = self.resolve_id(runtime_id) {
            return Ok(LightRuntimeKind::from_canonical_id(canonical));
        }
        let parsed = runtime_id.parse::<LightRuntimeKind>()?;
        let canonical = self
            .resolve_id(parsed.as_str())
            .ok_or_else(|| anyhow::anyhow!("unknown light runtime '{}'", runtime_id))?;
        Ok(LightRuntimeKind::from_canonical_id(canonical))
    }

    pub fn module_for_kind(&self, kind: &LightRuntimeKind) -> Option<LightRuntimeModule> {
        let canonical = self.resolve_id(kind.as_str())?;
        self.modules.get(canonical).copied()
    }

    pub fn module_for_id(&self, runtime_id: &str) -> Option<LightRuntimeModule> {
        let canonical = self.resolve_id(runtime_id)?;
        self.modules.get(canonical).copied()
    }

    pub fn manifests(&self) -> Vec<RuntimeManifest> {
        self.order
            .iter()
            .filter_map(|runtime_id| self.modules.get(runtime_id))
            .map(LightRuntimeModule::manifest)
            .collect()
    }

    pub fn available_runtime_ids(&self) -> Vec<LightRuntimeKind> {
        self.order
            .iter()
            .filter_map(|runtime_id| self.modules.get(runtime_id))
            .map(LightRuntimeModule::runtime_kind)
            .collect()
    }
}

pub fn register_light_runtime_module(s: &mut AppState, module: LightRuntimeModule) -> Result<()> {
    s.light_runtime_registry.register(module)
}

pub fn register_light_runtime_modules<I>(s: &mut AppState, modules: I) -> Result<()>
where
    I: IntoIterator<Item = LightRuntimeModule>,
{
    for module in modules {
        register_light_runtime_module(s, module)?;
    }
    Ok(())
}

pub fn light_runtime_manifests(state: &SharedState) -> Result<Vec<RuntimeManifest>> {
    let s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    Ok(s.light_runtime_registry.manifests())
}

pub fn light_runtime_manifest(state: &SharedState, runtime_id: &str) -> Result<RuntimeManifest> {
    let s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let module = s
        .light_runtime_registry
        .module_for_id(runtime_id)
        .ok_or_else(|| anyhow::anyhow!("unknown light runtime '{}'", runtime_id))?;
    Ok(module.manifest())
}

pub fn parse_light_runtime_id(state: &SharedState, runtime_id: &str) -> Result<LightRuntimeKind> {
    let s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    s.light_runtime_registry.parse_runtime_kind(runtime_id)
}

fn is_valid_runtime_id(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        && value
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
}

fn validate_runtime_id(value: &str) -> Result<()> {
    if is_valid_runtime_id(value) {
        Ok(())
    } else {
        Err(anyhow::anyhow!("invalid light runtime id '{}'", value))
    }
}

fn validate_runtime_alias(value: &str) -> Result<()> {
    if !value.is_empty()
        && value.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-' || byte == b'_'
        })
    {
        Ok(())
    } else {
        Err(anyhow::anyhow!("invalid light runtime alias '{}'", value))
    }
}

pub fn run_light_runtime_extension(
    state: &SharedState,
    runtime_id: &str,
    request: RuntimeExtensionRequest,
) -> Result<RuntimeExtensionResponse> {
    let requested_kind = parse_light_runtime_id(state, runtime_id)?;
    let (selected_kind, host_runtime, module) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let selected_kind = s.light_runtime_kind.clone();
        let module = s
            .light_runtime_registry
            .module_for_kind(&requested_kind)
            .ok_or_else(|| anyhow::anyhow!("unknown light runtime '{}'", runtime_id))?;
        (
            selected_kind,
            s.hub_runtime()
                .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?,
            module,
        )
    };

    if selected_kind != requested_kind {
        return Err(anyhow::anyhow!(
            "light runtime '{}' is not active; selected light runtime is '{}'",
            runtime_id,
            selected_kind.as_str()
        ));
    }

    with_light_runtime_instance(
        state,
        module,
        host_runtime,
        |light_runtime, host_runtime| {
            run_light_runtime_extension_with_handle(
                state,
                module.id,
                host_runtime,
                light_runtime,
                request,
            )
        },
    )
}

fn with_light_runtime_instance<T>(
    state: &SharedState,
    module: LightRuntimeModule,
    host_runtime: Arc<dyn RuntimeHandle>,
    callback: impl FnOnce(&mut dyn LightRuntime, &dyn RuntimeHandle) -> Result<T>,
) -> Result<T> {
    match module.instance {
        LightRuntimeInstance::Ephemeral { create } => {
            let mut light_runtime = create(host_runtime.clone());
            callback(light_runtime.as_mut(), host_runtime.as_ref())
        }
        LightRuntimeInstance::Cached { ensure } => {
            let light_runtime = ensure(state, &module)?;
            let mut light_runtime = light_runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("light runtime lock poisoned"))?;
            callback(light_runtime.as_mut(), host_runtime.as_ref())
        }
    }
}

fn run_light_runtime_extension_with_handle(
    state: &SharedState,
    runtime_id: &str,
    host_runtime: &dyn RuntimeHandle,
    light_runtime: &mut dyn LightRuntime,
    request: RuntimeExtensionRequest,
) -> Result<RuntimeExtensionResponse> {
    if light_runtime.runtime_id() != runtime_id {
        return Err(anyhow::anyhow!(
            "selected light runtime '{}' returned mismatched runtime id '{}'",
            runtime_id,
            light_runtime.runtime_id()
        ));
    }

    let runtime_state = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        s.light_runtime_state.get(runtime_id).cloned()
    };
    let snapshot = build_runtime_snapshot(host_runtime, runtime_state.as_ref());
    let response = light_runtime
        .handle_extension(&snapshot, request)
        .map_err(|error| anyhow::anyhow!("{} runtime failed: {}", light_runtime.name(), error))?;
    apply_runtime_plan_to_handle(state, runtime_id, host_runtime, &response.plan)?;
    Ok(response)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimePlanApplyReport {
    pub dispatch_count: usize,
    pub state_write_count: usize,
    pub diagnostic_count: usize,
}

/// Build the neutral snapshot passed to light runtimes.
pub fn build_runtime_snapshot(
    runtime: &dyn RuntimeHandle,
    runtime_state: Option<&BTreeMap<String, BTreeMap<String, serde_json::Value>>>,
) -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::default();

    for node in runtime
        .engine_all_effective_node_snapshots()
        .into_iter()
        .filter(|node| !crate::topology::is_internal_light_node_id(&node.id))
    {
        let kind = runtime_node_kind_from_light_node_kind(node.kind);
        let node_state = runtime_state
            .and_then(|state| state.get(&node.id))
            .cloned()
            .unwrap_or_default();

        snapshot.nodes.push(RuntimeNode {
            id: node.id.clone(),
            name: node.name.clone(),
            kind,
            parent_id: node.parent_id.clone(),
            properties: BTreeMap::from([
                ("disabled".to_string(), json!(node.disabled)),
                ("soft_off".to_string(), json!(node.soft_off)),
                ("hard_off".to_string(), json!(node.hard_off)),
                ("mood_active".to_string(), json!(node.mood_active)),
                ("standby_enabled".to_string(), json!(node.standby_enabled)),
            ]),
        });

        snapshot.state.insert(
            node.id,
            NodeState {
                power_on: !node.hard_off && !node.soft_off,
                rhythm_enabled: node.rhythm_enabled,
                brightness: None,
                kelvin: None,
                properties: node_state,
            },
        );
    }

    snapshot
}

pub fn build_runtime_snapshot_from_state(
    state: &SharedState,
    runtime_id: &str,
) -> Result<RuntimeSnapshot> {
    let (runtime, runtime_state) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        (
            s.hub_runtime()
                .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?,
            s.light_runtime_state.get(runtime_id).cloned(),
        )
    };
    Ok(build_runtime_snapshot(
        runtime.as_ref(),
        runtime_state.as_ref(),
    ))
}

pub fn apply_runtime_plan(
    state: &SharedState,
    runtime_id: &str,
    plan: &RuntimePlan,
) -> Result<RuntimePlanApplyReport> {
    let runtime = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        s.hub_runtime()
            .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?
    };
    apply_runtime_plan_to_handle(state, runtime_id, runtime.as_ref(), plan)
}

pub fn run_light_runtime_event(
    state: &SharedState,
    runtime_id: &str,
    light_runtime: &mut dyn LightRuntime,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    validate_runtime_selection(runtime_id, light_runtime, &event)?;
    let snapshot = build_runtime_snapshot_from_state(state, runtime_id)?;
    let plan = light_runtime
        .handle_event(&snapshot, event)
        .map_err(|error| anyhow::anyhow!("{} runtime failed: {}", light_runtime.name(), error))?;
    apply_runtime_plan(state, runtime_id, &plan)
}

pub fn apply_runtime_plan_to_handle(
    state: &SharedState,
    runtime_id: &str,
    runtime: &dyn RuntimeHandle,
    plan: &RuntimePlan,
) -> Result<RuntimePlanApplyReport> {
    emit_diagnostics(runtime_id, &plan.diagnostics);

    let mut dispatch_count = 0;
    for command in &plan.dispatch {
        if let Some((expanded, dispatch_records)) =
            expand_route_aware_room_turn_on(state, runtime, command)?
        {
            for expanded_command in &expanded {
                apply_dispatch_command(runtime, expanded_command)?;
                update_host_state_after_dispatch(state, runtime, expanded_command);
                dispatch_count += 1;
            }
            runtime.record_rhythm_dispatches(&dispatch_records)?;
            // Synthetic group route IDs are intentionally hidden, while
            // direct children update their own cache. Also update the public
            // parent whose room action produced this fan-out.
            update_host_state_after_dispatch(state, runtime, command);
        } else {
            apply_dispatch_command(runtime, command)?;
            update_host_state_after_dispatch(state, runtime, command);
            dispatch_count += 1;
        }
    }

    let state_write_count = apply_state_writes(state, runtime_id, &plan.state_writes)?;

    Ok(RuntimePlanApplyReport {
        dispatch_count,
        state_write_count,
        diagnostic_count: plan.diagnostics.len(),
    })
}

fn expand_route_aware_room_turn_on(
    state: &SharedState,
    runtime: &dyn RuntimeHandle,
    command: &DispatchCommand,
) -> Result<Option<(Vec<DispatchCommand>, Vec<RhythmDispatchRecord>)>> {
    let DispatchCommand::TurnOn {
        target: DispatchTarget::Node { node_id: room_id },
        command: room_command,
    } = command
    else {
        return Ok(None);
    };
    if runtime
        .engine_node_snapshot(room_id)
        .is_none_or(|node| !node.kind.is_room())
    {
        return Ok(None);
    }

    let mut routes = {
        let app = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let Some(composite) = app.composite_controller.as_ref() else {
            return Ok(None);
        };
        app.topology
            .periodic_light_nodes(&app.canonical_registry)
            .into_iter()
            .filter(|node| node.emit_node_id == *room_id && composite.has_active_route(&node.id))
            .collect::<Vec<_>>()
    };
    routes.sort_by(|left, right| left.id.cmp(&right.id));

    let direct_route_count = routes
        .iter()
        .filter(|route| route.source_node_id != *room_id)
        .count();
    if direct_route_count == 0 {
        return Ok(None);
    }

    log::info!(
        target: "light_runtime",
        "route_aware_room_dispatch: room={} routes={} direct_routes={}",
        room_id,
        routes.len(),
        direct_route_count,
    );

    let mut expanded = Vec::with_capacity(routes.len());
    let mut dispatch_records = Vec::new();
    for route in routes {
        if route.source_node_id == *room_id {
            expanded.push(DispatchCommand::TurnOn {
                target: DispatchTarget::Node { node_id: route.id },
                command: room_command.clone(),
            });
            continue;
        }

        let tick = TickContext {
            node_id: route.id,
            hour: runtime.current_hour() as f64,
            epoch_ms: Some(chrono::Utc::now().timestamp_millis()),
            metadata: BTreeMap::from([("reason".to_string(), json!("manual_room_route_refresh"))]),
        };
        match runtime.plan_forced_node_refresh(&tick, &route.source_node_id)? {
            RhythmPeriodicPlanOutcome::RequiresLightCheck => {
                return Err(anyhow::anyhow!(
                    "forced route refresh unexpectedly required a light-state check"
                ));
            }
            RhythmPeriodicPlanOutcome::Plan {
                plan,
                dispatch_records: records,
            } => {
                expanded.extend(plan.dispatch);
                dispatch_records.extend(records);
            }
        }
    }

    Ok(Some((expanded, dispatch_records)))
}

pub fn run_selected_light_runtime_event(
    state: &SharedState,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    let (runtime, module) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        let kind = s.light_runtime_kind.clone();
        let module = s
            .light_runtime_registry
            .module_for_kind(&kind)
            .ok_or_else(|| anyhow::anyhow!("unknown light runtime '{}'", kind.as_str()))?;
        (
            s.hub_runtime()
                .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?,
            module,
        )
    };

    with_light_runtime_instance(state, module, runtime, |light_runtime, host_runtime| {
        run_light_runtime_event_with_handle(state, module.id, host_runtime, light_runtime, event)
    })
}

pub fn run_light_runtime_event_with_handle(
    state: &SharedState,
    runtime_id: &str,
    host_runtime: &dyn RuntimeHandle,
    light_runtime: &mut dyn LightRuntime,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    validate_runtime_selection(runtime_id, light_runtime, &event)?;
    let runtime_state = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        s.light_runtime_state.get(runtime_id).cloned()
    };
    let snapshot = build_runtime_snapshot(host_runtime, runtime_state.as_ref());
    let plan = light_runtime
        .handle_event(&snapshot, event)
        .map_err(|error| anyhow::anyhow!("{} runtime failed: {}", light_runtime.name(), error))?;
    apply_runtime_plan_to_handle(state, runtime_id, host_runtime, &plan)
}

fn validate_runtime_selection(
    runtime_id: &str,
    light_runtime: &dyn LightRuntime,
    event: &RuntimeEvent,
) -> Result<()> {
    if light_runtime.runtime_id() != runtime_id {
        return Err(anyhow::anyhow!(
            "selected light runtime '{}' returned mismatched runtime id '{}'",
            runtime_id,
            light_runtime.runtime_id()
        ));
    }

    let capabilities = light_runtime.capabilities();
    match event {
        RuntimeEvent::Input(_) if !capabilities.input_events => Err(anyhow::anyhow!(
            "selected light runtime '{}' does not support input events",
            runtime_id
        )),
        RuntimeEvent::PeriodicTick(_) if !capabilities.periodic_ticks => Err(anyhow::anyhow!(
            "selected light runtime '{}' does not support periodic ticks",
            runtime_id
        )),
        RuntimeEvent::HostStateChanged { .. } if !capabilities.host_state_changes => {
            Err(anyhow::anyhow!(
                "selected light runtime '{}' does not support host state changes",
                runtime_id
            ))
        }
        RuntimeEvent::Input(_)
        | RuntimeEvent::PeriodicTick(_)
        | RuntimeEvent::HostStateChanged { .. } => Ok(()),
    }
}

pub fn reset_light_runtime_for_kind(s: &mut AppState, kind: LightRuntimeKind) {
    s.light_runtime_kind = kind;
    s.light_runtime = None;
    s.light_runtime_config_fingerprint = None;
}

pub fn apply_dispatch_command(
    runtime: &dyn RuntimeHandle,
    command: &DispatchCommand,
) -> Result<()> {
    match command {
        DispatchCommand::TurnOn {
            target: DispatchTarget::Node { node_id },
            command,
        } => runtime.apply_room_command(node_id, core_lighting_command_from_runtime(command)),
        DispatchCommand::TurnOff {
            target: DispatchTarget::Node { node_id },
            transition_ms,
        } => runtime.lights_off_room(node_id, *transition_ms),
        DispatchCommand::TurnOn { target, .. } | DispatchCommand::TurnOff { target, .. } => {
            Err(anyhow::anyhow!(
                "runtime dispatch target is not yet supported by the OS host adapter: {:?}",
                target
            ))
        }
    }
}

fn update_host_state_after_dispatch(
    state: &SharedState,
    runtime: &dyn RuntimeHandle,
    command: &DispatchCommand,
) {
    match command {
        DispatchCommand::TurnOn {
            target: DispatchTarget::Node { node_id },
            ..
        } => update_host_node_after_dispatch(state, runtime, node_id, true),
        DispatchCommand::TurnOff {
            target: DispatchTarget::Node { node_id },
            ..
        } => update_host_node_after_dispatch(state, runtime, node_id, false),
        DispatchCommand::TurnOn { .. } | DispatchCommand::TurnOff { .. } => {}
    }
}

fn update_host_node_after_dispatch(
    state: &SharedState,
    _runtime: &dyn RuntimeHandle,
    node_id: &str,
    lights_on: bool,
) {
    if crate::topology::is_internal_light_node_id(node_id) {
        return;
    }
    let runtime = state
        .lock()
        .ok()
        .and_then(|s| s.hub_runtime())
        .filter(|candidate| candidate.engine_node_snapshot(node_id).is_some());
    if let Some(runtime) = runtime {
        crate::commands::update_lights_on_cache_for_runtime_node(
            state, &runtime, node_id, lights_on,
        );
        crate::commands::emit_node_state_event_after_apply(state, &runtime, node_id);
    }
}

pub fn apply_state_writes(
    state: &SharedState,
    runtime_id: &str,
    writes: &[StateWrite],
) -> Result<usize> {
    if writes.is_empty() {
        return Ok(0);
    }

    let mut s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    apply_state_writes_locked(&mut s.light_runtime_state, runtime_id, writes);
    if let Some(storage) = s.storage.as_ref() {
        storage.save_light_runtime_state(&s.light_runtime_state)?;
    }
    Ok(writes.len())
}

pub fn apply_state_writes_locked(
    store: &mut StoredLightRuntimeState,
    runtime_id: &str,
    writes: &[StateWrite],
) {
    let runtime = store.entry(runtime_id.to_string()).or_default();
    for write in writes {
        runtime
            .entry(write.node_id.clone())
            .or_default()
            .insert(write.key.clone(), write.value.clone());
    }
}

fn emit_diagnostics(runtime_id: &str, diagnostics: &[rhythm_runtime_api::RuntimeDiagnostic]) {
    for diagnostic in diagnostics {
        match diagnostic.level {
            DiagnosticLevel::Debug => {
                log::debug!(target: "light_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Info => {
                log::info!(target: "light_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Warn => {
                log::warn!(target: "light_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Error => {
                log::error!(target: "light_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use async_trait::async_trait;
    use rhythm_core::{
        HubDispatchTarget, HubLightController, LightControlResult, LightProfileNodeOverride,
        RestoredNodeState, Room, RoomProfileSettings,
    };
    use rhythm_runtime_api::{
        DiagnosticLevel, InputAction, LightingCommand, RuntimeDiagnostic, RuntimeError,
        RuntimeHttpMethod, RuntimeInputEvent,
    };
    use std::sync::atomic::AtomicBool;
    use std::sync::{Arc, Mutex};

    #[derive(Default)]
    struct RecordingHubController {
        turn_on_calls: Mutex<Vec<(String, rhythm_core::LightingCommand)>>,
    }

    impl RecordingHubController {
        fn wait_for_turn_on_calls(
            &self,
            expected: usize,
        ) -> Vec<(String, rhythm_core::LightingCommand)> {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
            loop {
                let calls = self.turn_on_calls.lock().unwrap().clone();
                if calls.len() >= expected || std::time::Instant::now() >= deadline {
                    return calls;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    #[async_trait]
    impl HubLightController for RecordingHubController {
        async fn turn_on_target(
            &self,
            target: &HubDispatchTarget,
            command: rhythm_core::LightingCommand,
        ) -> LightControlResult<()> {
            self.turn_on_calls
                .lock()
                .unwrap()
                .push((target.label(), command));
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
            Ok(true)
        }

        fn name(&self) -> &str {
            "RecordingHub"
        }
    }

    #[test]
    fn state_writes_are_namespaced_by_runtime_and_node() {
        let mut store = StoredLightRuntimeState::new();
        apply_state_writes_locked(
            &mut store,
            "cached-lab",
            &[StateWrite {
                node_id: "kitchen".to_string(),
                key: "cached_runtime_state".to_string(),
                value: json!({ "is_on": true }),
            }],
        );

        assert_eq!(
            store["cached-lab"]["kitchen"]["cached_runtime_state"],
            json!({ "is_on": true })
        );
    }

    #[test]
    fn build_runtime_snapshot_excludes_internal_light_nodes() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        runtime.add_node(
            "__rhythm_light_node__|room=kitchen|kind=group|hub=hue@bridge|source=hue-room",
            "Kitchen",
            rhythm_core::LightNodeKind::Room,
            None,
        );

        let snapshot = build_runtime_snapshot(runtime.as_ref(), None);
        let ids: Vec<_> = snapshot.nodes.iter().map(|node| node.id.as_str()).collect();

        assert_eq!(ids, vec!["kitchen"]);
        assert!(!snapshot.state.contains_key(
            "__rhythm_light_node__|room=kitchen|kind=group|hub=hue@bridge|source=hue-room"
        ));
    }

    #[test]
    fn registry_registers_external_runtime_ids_and_aliases_in_order() {
        let mut registry = LightRuntimeRegistry::default();

        registry.register(external_runtime_module()).unwrap();
        registry.register(other_external_runtime_module()).unwrap();

        assert_eq!(
            registry.resolve_id(EXTERNAL_RUNTIME_ID),
            Some(EXTERNAL_RUNTIME_ID)
        );
        assert_eq!(
            registry.resolve_id(EXTERNAL_RUNTIME_ALIAS),
            Some(EXTERNAL_RUNTIME_ID)
        );
        assert_eq!(
            registry
                .parse_runtime_kind(EXTERNAL_RUNTIME_ALIAS)
                .unwrap()
                .as_str(),
            EXTERNAL_RUNTIME_ID
        );
        assert_eq!(
            registry.module_for_id(EXTERNAL_RUNTIME_ALIAS).unwrap().id,
            EXTERNAL_RUNTIME_ID
        );

        let manifest_ids: Vec<_> = registry
            .manifests()
            .into_iter()
            .map(|manifest| manifest.id)
            .collect();
        assert_eq!(
            manifest_ids,
            vec![
                EXTERNAL_RUNTIME_ID.to_string(),
                OTHER_EXTERNAL_RUNTIME_ID.to_string()
            ]
        );

        let available_ids: Vec<_> = registry
            .available_runtime_ids()
            .into_iter()
            .map(|kind| kind.as_str().to_string())
            .collect();
        assert_eq!(
            available_ids,
            vec![
                EXTERNAL_RUNTIME_ID.to_string(),
                OTHER_EXTERNAL_RUNTIME_ID.to_string()
            ]
        );
    }

    #[test]
    fn registry_rejects_duplicate_ids_alias_conflicts_and_bad_manifests() {
        let mut registry = LightRuntimeRegistry::default();
        registry.register(external_runtime_module()).unwrap();

        let duplicate_id = registry
            .register(external_runtime_module())
            .unwrap_err()
            .to_string();
        assert!(duplicate_id.contains("already registered"));

        let duplicate_alias = registry
            .register(conflicting_alias_runtime_module())
            .unwrap_err()
            .to_string();
        assert!(duplicate_alias.contains("alias"));
        assert!(duplicate_alias.contains("already registered"));

        let manifest_mismatch = LightRuntimeRegistry::default()
            .register(mismatched_manifest_runtime_module())
            .unwrap_err()
            .to_string();
        assert!(manifest_mismatch.contains("returned manifest id"));

        let invalid_id = LightRuntimeRegistry::default()
            .register(invalid_id_runtime_module())
            .unwrap_err()
            .to_string();
        assert!(invalid_id.contains("invalid light runtime id"));

        let invalid_alias = LightRuntimeRegistry::default()
            .register(invalid_alias_runtime_module())
            .unwrap_err()
            .to_string();
        assert!(invalid_alias.contains("invalid light runtime alias"));
    }

    #[test]
    fn registry_rejects_unregistered_runtime_ids() {
        let mut registry = LightRuntimeRegistry::default();
        registry.register(external_runtime_module()).unwrap();

        let unknown = registry
            .parse_runtime_kind("not-registered")
            .unwrap_err()
            .to_string();
        assert!(unknown.contains("unknown light runtime"));

        let invalid = registry
            .parse_runtime_kind("not_registered")
            .unwrap_err()
            .to_string();
        assert!(invalid.contains("invalid light runtime id"));
    }

    #[test]
    fn run_light_runtime_event_builds_snapshot_and_applies_plan() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        let mut light_runtime = SnapshotRuntime { saw_power: None };

        let report = run_light_runtime_event(
            &state,
            "snapshot-runtime",
            &mut light_runtime,
            RuntimeEvent::HostStateChanged {
                reason: "test".to_string(),
            },
        )
        .unwrap();

        assert_eq!(light_runtime.saw_power, Some(true));
        assert_eq!(report.dispatch_count, 1);
        assert_eq!(report.state_write_count, 1);
        assert_eq!(
            state.lock().unwrap().light_runtime_state["snapshot-runtime"]["kitchen"]["last"],
            json!("host-call")
        );
    }

    #[test]
    fn rejects_mismatched_selected_runtime_id() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        let mut light_runtime = SnapshotRuntime { saw_power: None };

        let error = run_light_runtime_event(
            &state,
            "other-runtime",
            &mut light_runtime,
            RuntimeEvent::HostStateChanged {
                reason: "test".to_string(),
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("mismatched runtime id"));
    }

    #[test]
    fn selected_default_runtime_runs_rhythm_adaptive() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);

        let report = run_selected_light_runtime_event(
            &state,
            RuntimeEvent::Input(RuntimeInputEvent {
                source_id: "switch-a".to_string(),
                target_id: "kitchen".to_string(),
                action: InputAction::On,
                epoch_ms: None,
                metadata: Default::default(),
            }),
        )
        .unwrap();

        assert_eq!(
            state.lock().unwrap().light_runtime_kind.as_str(),
            "rhythm-adaptive"
        );
        assert_eq!(report.dispatch_count, 1);
    }

    #[test]
    fn direct_bulb_profile_override_survives_manual_room_fanout() {
        use rhythm_core::runtime::registry::SimpleDeviceRegistry;
        use rhythm_core::runtime::scheduler::NoOpScheduler;
        use rhythm_core::runtime::time::MockTimeProvider;
        use rhythm_core::{CompositeController, RhythmRuntime, RuntimeConfig};

        let group_key = HubKey::new(HubType::new("hue"), "bridge");
        let direct_key = HubKey::new(HubType::new("hue_ble"), "local");
        let group_controller = Arc::new(RecordingHubController::default());
        let direct_controller = Arc::new(RecordingHubController::default());
        let composite = Arc::new(CompositeController::new());
        composite.register_controller(&group_key.to_string(), group_controller.clone());
        composite.register_controller(&direct_key.to_string(), direct_controller.clone());

        let runtime = Arc::new(RhythmRuntime::new(
            composite.clone(),
            MockTimeProvider::new(12.0, 172, 2024),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ));
        runtime.add_node("hallway", "Hallway", rhythm_core::LightNodeKind::Room, None);

        let state = make_state_with_runtime(runtime.clone());
        let (group_light_id, direct_light_id, routing) = {
            let mut app = state.lock().unwrap();
            let group_identity = crate::canonical::identity::DiscoveredIdentity {
                native_id: "hue-group-light".to_string(),
                room_id: Some("hue-hallway".to_string()),
                room_name: Some("Hallway".to_string()),
                name: "Grouped lamp".to_string(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: Vec::new(),
                manufacturer: None,
                model: None,
            };
            let direct_identity = crate::canonical::identity::DiscoveredIdentity {
                native_id: "hue-ble-direct".to_string(),
                room_id: None,
                room_name: None,
                name: "Direct lamp".to_string(),
                device_type: rhythm_core::runtime::hub_registry::DeviceType::Light,
                hardware_ids: Vec::new(),
                manufacturer: None,
                model: None,
            };
            let resolve_id = |result| match result {
                crate::canonical::registry::ResolveResult::Created { canonical_id }
                | crate::canonical::registry::ResolveResult::AlreadyKnown { canonical_id }
                | crate::canonical::registry::ResolveResult::ReApproved { canonical_id } => {
                    canonical_id
                }
                other => panic!("unexpected identity resolution: {other:?}"),
            };
            let group_light_id = resolve_id(app.canonical_registry.resolve(
                &group_identity,
                &group_key,
                1_000,
            ));
            let direct_light_id = resolve_id(app.canonical_registry.resolve(
                &direct_identity,
                &direct_key,
                1_000,
            ));

            let mut room = crate::topology::TopologyRoom::new("hallway", "Hallway");
            room.upsert_hub_room_binding(crate::topology::HubRoomBinding {
                hub_key: group_key.clone(),
                hub_room_id: "hue-hallway".to_string(),
                control_id: "grouped-light-hallway".to_string(),
                light_device_ids: vec!["hue-group-light".to_string()],
            });
            app.topology.insert_room(room);
            assert!(app
                .topology
                .attach_device_user_override("hallway", &group_light_id));
            assert!(app
                .topology
                .attach_device_user_override("hallway", &direct_light_id));
            app.composite_controller = Some(composite.clone());
            let routing = app.topology.composite_routing(&app.canonical_registry);
            (group_light_id, direct_light_id, routing)
        };
        composite.update_routing(routing);

        runtime.add_node(
            &group_light_id,
            "Grouped lamp",
            rhythm_core::LightNodeKind::LightDevice,
            Some("hallway".to_string()),
        );
        runtime.add_node(
            &direct_light_id,
            "Direct lamp",
            rhythm_core::LightNodeKind::LightDevice,
            Some("hallway".to_string()),
        );
        let mut profile_settings = RoomProfileSettings::default();
        profile_settings.profile_overrides.insert(
            rhythm_core::RHYTHM_PROFILE_ID.to_string(),
            LightProfileNodeOverride {
                max_brightness: Some(31),
                ..Default::default()
            },
        );
        runtime.restore_node_state(
            &direct_light_id,
            RestoredNodeState {
                rhythm_enabled: true,
                disabled: false,
                time_offset_minutes: 0.0,
                brightness_offset: 0.0,
                soft_off: false,
                mood_active: false,
                standby_enabled: false,
                hard_off: false,
                profile_settings,
            },
        );

        let report = run_selected_light_runtime_event(
            &state,
            RuntimeEvent::Input(RuntimeInputEvent {
                source_id: "app".to_string(),
                target_id: "hallway".to_string(),
                action: InputAction::Reset,
                epoch_ms: None,
                metadata: Default::default(),
            }),
        )
        .unwrap();

        assert_eq!(report.dispatch_count, 2);
        let group_calls = group_controller.wait_for_turn_on_calls(1);
        let direct_calls = direct_controller.wait_for_turn_on_calls(1);
        assert_eq!(group_calls.len(), 1);
        assert_eq!(group_calls[0].0, "hue-hallway (grouped-light-hallway)");
        assert!(group_calls[0].1.brightness > 31);
        assert_eq!(direct_calls.len(), 1);
        assert_eq!(direct_calls[0].0, "hue-ble-direct");
        assert_eq!(direct_calls[0].1.brightness, 31);
    }

    #[test]
    fn selected_cached_runtime_runs_and_persists_state() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        {
            let mut s = state.lock().unwrap();
            reset_light_runtime_for_kind(&mut s, LightRuntimeKind::new(CACHED_RUNTIME_ID));
        }

        let report = run_selected_light_runtime_event(
            &state,
            RuntimeEvent::Input(RuntimeInputEvent {
                source_id: "switch-a".to_string(),
                target_id: "kitchen".to_string(),
                action: InputAction::Named("cached_on".to_string()),
                epoch_ms: None,
                metadata: Default::default(),
            }),
        )
        .unwrap();

        assert_eq!(report.dispatch_count, 1);
        assert!(state.lock().unwrap().light_runtime.is_some());
        assert!(
            state.lock().unwrap().light_runtime_state[CACHED_RUNTIME_ID]["kitchen"]
                .contains_key("cached_runtime_state")
        );
    }

    #[test]
    fn selected_external_runtime_module_runs_and_namespaces_state() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        install_external_runtime_module(&state);
        select_external_runtime(&state, EXTERNAL_RUNTIME_ALIAS);

        let report = run_selected_light_runtime_event(
            &state,
            RuntimeEvent::HostStateChanged {
                reason: "topology-sync".to_string(),
            },
        )
        .unwrap();

        assert_eq!(report.dispatch_count, 1);
        assert_eq!(report.state_write_count, 1);
        assert_eq!(report.diagnostic_count, 1);
        let s = state.lock().unwrap();
        assert_eq!(s.light_runtime_kind.as_str(), EXTERNAL_RUNTIME_ID);
        assert_eq!(
            s.light_runtime_state[EXTERNAL_RUNTIME_ID]["kitchen"]["external_event"],
            json!("host_state_changed")
        );
        assert!(
            !s.light_runtime_state
                .contains_key(RHYTHM_ADAPTIVE_RUNTIME_ID),
            "external runtime state must not leak into the default runtime namespace"
        );
    }

    #[test]
    fn external_runtime_extension_routes_by_alias_and_applies_plan() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        install_external_runtime_module(&state);
        select_external_runtime(&state, EXTERNAL_RUNTIME_ALIAS);

        let response = run_light_runtime_extension(
            &state,
            EXTERNAL_RUNTIME_ALIAS,
            RuntimeExtensionRequest {
                method: RuntimeHttpMethod::Put,
                path: "/settings".to_string(),
                query: Default::default(),
                body: json!({"level": 7}),
            },
        )
        .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body["runtime_id"], EXTERNAL_RUNTIME_ID);
        assert_eq!(response.body["node_count"], 1);
        assert_eq!(
            state.lock().unwrap().light_runtime_state[EXTERNAL_RUNTIME_ID]["kitchen"]
                ["external_extension"],
            json!({"level": 7})
        );
    }

    #[test]
    fn external_runtime_extension_rejects_unknown_endpoint_without_state_write() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        install_external_runtime_module(&state);
        select_external_runtime(&state, EXTERNAL_RUNTIME_ALIAS);

        let error = run_light_runtime_extension(
            &state,
            EXTERNAL_RUNTIME_ALIAS,
            RuntimeExtensionRequest {
                method: RuntimeHttpMethod::Post,
                path: "/unknown".to_string(),
                query: Default::default(),
                body: json!({"level": 7}),
            },
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("unsupported runtime extension endpoint"));
        assert!(
            state.lock().unwrap().light_runtime_state.is_empty(),
            "rejected extension endpoints must not persist partial runtime state"
        );
    }

    #[test]
    fn cached_runtime_rebuilds_when_host_topology_changes() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime.clone());
        {
            let mut s = state.lock().unwrap();
            reset_light_runtime_for_kind(&mut s, LightRuntimeKind::new(CACHED_RUNTIME_ID));
        }

        ensure_registered_cached_runtime(&state).unwrap();
        let (first_runtime, first_fingerprint) = {
            let s = state.lock().unwrap();
            (
                s.light_runtime.clone().unwrap(),
                s.light_runtime_config_fingerprint.clone().unwrap(),
            )
        };

        runtime.add_node("bedroom", "Bedroom", rhythm_core::LightNodeKind::Room, None);
        ensure_registered_cached_runtime(&state).unwrap();

        let s = state.lock().unwrap();
        let second_runtime = s.light_runtime.clone().unwrap();
        let second_fingerprint = s.light_runtime_config_fingerprint.clone().unwrap();
        assert_ne!(first_fingerprint, second_fingerprint);
        assert!(!Arc::ptr_eq(&first_runtime, &second_runtime));
    }

    #[test]
    fn applies_node_dispatch_against_existing_runtime_handle() {
        let state = Arc::new(std::sync::Mutex::new(crate::state::AppState::default()));
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let plan = RuntimePlan {
            dispatch: vec![DispatchCommand::TurnOn {
                target: DispatchTarget::Node {
                    node_id: "kitchen".to_string(),
                },
                command: LightingCommand::new(42, 2700),
            }],
            state_writes: vec![StateWrite {
                node_id: "kitchen".to_string(),
                key: "last".to_string(),
                value: json!("tick"),
            }],
            diagnostics: vec![RuntimeDiagnostic {
                level: DiagnosticLevel::Info,
                message: "planned".to_string(),
            }],
        };

        let report =
            apply_runtime_plan_to_handle(&state, "test-runtime", runtime.as_ref(), &plan).unwrap();
        assert_eq!(report.dispatch_count, 1);
        assert_eq!(report.state_write_count, 1);
        assert_eq!(report.diagnostic_count, 1);
        assert_eq!(
            state.lock().unwrap().light_runtime_state["test-runtime"]["kitchen"]["last"],
            json!("tick")
        );
    }

    #[test]
    fn rejects_unsupported_group_dispatch_target_before_state_writes() {
        let state = Arc::new(std::sync::Mutex::new(crate::state::AppState::default()));
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let plan = RuntimePlan {
            dispatch: vec![DispatchCommand::TurnOn {
                target: DispatchTarget::Group {
                    hub_id: "matter@local".to_string(),
                    control_id: "group-1".to_string(),
                },
                command: LightingCommand::new(42, 2700),
            }],
            state_writes: vec![StateWrite {
                node_id: "kitchen".to_string(),
                key: "should_not_write".to_string(),
                value: json!(true),
            }],
            diagnostics: vec![RuntimeDiagnostic {
                level: DiagnosticLevel::Warn,
                message: "group target requested".to_string(),
            }],
        };

        let error = apply_runtime_plan_to_handle(&state, "test-runtime", runtime.as_ref(), &plan)
            .unwrap_err()
            .to_string();

        assert!(error.contains("runtime dispatch target is not yet supported"));
        assert!(
            state.lock().unwrap().light_runtime_state.is_empty(),
            "invalid dispatch targets must reject the plan before applying state writes"
        );
    }

    struct SnapshotRuntime {
        saw_power: Option<bool>,
    }

    impl LightRuntime for SnapshotRuntime {
        fn name(&self) -> &str {
            "snapshot-runtime"
        }

        fn handle_event(
            &mut self,
            snapshot: &RuntimeSnapshot,
            _event: RuntimeEvent,
        ) -> rhythm_runtime_api::RuntimeResult<RuntimePlan> {
            self.saw_power = snapshot.state.get("kitchen").map(|state| state.power_on);
            Ok(RuntimePlan {
                dispatch: vec![DispatchCommand::TurnOff {
                    target: DispatchTarget::Node {
                        node_id: "kitchen".to_string(),
                    },
                    transition_ms: Some(250),
                }],
                state_writes: vec![StateWrite {
                    node_id: "kitchen".to_string(),
                    key: "last".to_string(),
                    value: json!("host-call"),
                }],
                diagnostics: vec![],
            })
        }
    }

    const EXTERNAL_RUNTIME_ID: &str = "sunrise-lab";
    const EXTERNAL_RUNTIME_ALIAS: &str = "sunrise_lab";
    const OTHER_EXTERNAL_RUNTIME_ID: &str = "moonrise-lab";
    const CACHED_RUNTIME_ID: &str = "cached-lab";

    fn external_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            EXTERNAL_RUNTIME_ID,
            &[EXTERNAL_RUNTIME_ALIAS],
            external_runtime_manifest,
            create_external_runtime,
        )
    }

    fn other_external_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            OTHER_EXTERNAL_RUNTIME_ID,
            &["moonrise"],
            other_external_runtime_manifest,
            create_other_external_runtime,
        )
    }

    fn conflicting_alias_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            "alias-conflict",
            &[EXTERNAL_RUNTIME_ALIAS],
            alias_conflict_runtime_manifest,
            create_alias_conflict_runtime,
        )
    }

    fn mismatched_manifest_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            "manifest-mismatch",
            &[],
            mismatched_runtime_manifest,
            create_manifest_mismatch_runtime,
        )
    }

    fn invalid_id_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            "InvalidRuntime",
            &[],
            invalid_id_runtime_manifest,
            create_invalid_id_runtime,
        )
    }

    fn invalid_alias_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::ephemeral(
            "invalid-alias",
            &["bad alias"],
            invalid_alias_runtime_manifest,
            create_invalid_alias_runtime,
        )
    }

    fn cached_runtime_module() -> LightRuntimeModule {
        LightRuntimeModule::cached(
            CACHED_RUNTIME_ID,
            &[],
            cached_runtime_manifest,
            ensure_test_cached_runtime,
        )
    }

    fn external_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new(EXTERNAL_RUNTIME_ID, "Sunrise Lab")
            .with_description("External test runtime")
    }

    fn other_external_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new(OTHER_EXTERNAL_RUNTIME_ID, "Moonrise Lab")
    }

    fn alias_conflict_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new("alias-conflict", "Alias Conflict")
    }

    fn mismatched_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new("wrong-runtime-id", "Wrong Runtime Id")
    }

    fn invalid_id_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new("InvalidRuntime", "Invalid Runtime")
    }

    fn invalid_alias_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new("invalid-alias", "Invalid Alias")
    }

    fn cached_runtime_manifest() -> RuntimeManifest {
        RuntimeManifest::new(CACHED_RUNTIME_ID, "Cached Lab")
    }

    fn create_external_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime(EXTERNAL_RUNTIME_ID))
    }

    fn create_other_external_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime(OTHER_EXTERNAL_RUNTIME_ID))
    }

    fn create_alias_conflict_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime("alias-conflict"))
    }

    fn create_manifest_mismatch_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime("manifest-mismatch"))
    }

    fn create_invalid_id_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime("InvalidRuntime"))
    }

    fn create_invalid_alias_runtime(_: Arc<dyn RuntimeHandle>) -> Box<dyn LightRuntime> {
        Box::new(ExternalRuntime("invalid-alias"))
    }

    struct ExternalRuntime(&'static str);

    impl LightRuntime for ExternalRuntime {
        fn name(&self) -> &str {
            self.0
        }

        fn handle_event(
            &mut self,
            snapshot: &RuntimeSnapshot,
            event: RuntimeEvent,
        ) -> rhythm_runtime_api::RuntimeResult<RuntimePlan> {
            let node_id = first_snapshot_node_id(snapshot);
            let event_name = match event {
                RuntimeEvent::Input(_) => "input",
                RuntimeEvent::PeriodicTick(_) => "periodic_tick",
                RuntimeEvent::HostStateChanged { .. } => "host_state_changed",
            };
            Ok(RuntimePlan {
                dispatch: vec![DispatchCommand::TurnOn {
                    target: DispatchTarget::Node {
                        node_id: node_id.clone(),
                    },
                    command: LightingCommand::new(64, 2700),
                }],
                state_writes: vec![StateWrite {
                    node_id,
                    key: "external_event".to_string(),
                    value: json!(event_name),
                }],
                diagnostics: vec![RuntimeDiagnostic {
                    level: DiagnosticLevel::Info,
                    message: "external runtime handled event".to_string(),
                }],
            })
        }

        fn handle_extension(
            &mut self,
            snapshot: &RuntimeSnapshot,
            request: RuntimeExtensionRequest,
        ) -> rhythm_runtime_api::RuntimeResult<RuntimeExtensionResponse> {
            if request.path != "/settings" {
                return Err(RuntimeError::UnsupportedExtension(request.path));
            }
            let node_id = first_snapshot_node_id(snapshot);
            let body = request.body;
            Ok(RuntimeExtensionResponse::json(json!({
                "runtime_id": self.runtime_id(),
                "node_count": snapshot.nodes.len(),
            }))
            .with_plan(RuntimePlan {
                state_writes: vec![StateWrite {
                    node_id,
                    key: "external_extension".to_string(),
                    value: body,
                }],
                ..RuntimePlan::noop()
            }))
        }
    }

    struct CachedTestRuntime;

    impl LightRuntime for CachedTestRuntime {
        fn name(&self) -> &str {
            CACHED_RUNTIME_ID
        }

        fn handle_event(
            &mut self,
            snapshot: &RuntimeSnapshot,
            _event: RuntimeEvent,
        ) -> rhythm_runtime_api::RuntimeResult<RuntimePlan> {
            let node_id = first_snapshot_node_id(snapshot);
            Ok(RuntimePlan {
                dispatch: vec![DispatchCommand::TurnOn {
                    target: DispatchTarget::Node {
                        node_id: node_id.clone(),
                    },
                    command: LightingCommand::new(64, 2700),
                }],
                state_writes: vec![StateWrite {
                    node_id,
                    key: "cached_runtime_state".to_string(),
                    value: json!({"handled": true}),
                }],
                diagnostics: vec![],
            })
        }
    }

    fn first_snapshot_node_id(snapshot: &RuntimeSnapshot) -> String {
        snapshot
            .nodes
            .first()
            .map(|node| node.id.clone())
            .expect("test runtime snapshot should contain a node")
    }

    fn install_external_runtime_module(state: &SharedState) {
        let mut s = state.lock().unwrap();
        register_light_runtime_module(&mut s, external_runtime_module())
            .expect("external test runtime should register");
    }

    fn select_external_runtime(state: &SharedState, runtime_id: &str) {
        let kind = parse_light_runtime_id(state, runtime_id).unwrap();
        let mut s = state.lock().unwrap();
        reset_light_runtime_for_kind(&mut s, kind);
    }

    fn make_state_with_runtime(runtime: Arc<dyn RuntimeHandle>) -> SharedState {
        let mut app = crate::state::AppState::default();
        register_test_light_runtime_modules(&mut app);
        let hub_type = HubType::new("test");
        let hub_key = HubKey::new(hub_type.clone(), "hub.local");
        app.hubs.insert(
            hub_key.clone(),
            ActiveHub {
                hub_type,
                hub_key,
                runtime: Some(runtime),
                hub_data: Box::new(()),
                registry: None,
                discovery: None,
                shutdown: Arc::new(AtomicBool::new(false)),
            },
        );
        Arc::new(std::sync::Mutex::new(app))
    }

    fn register_test_light_runtime_modules(app: &mut AppState) {
        register_light_runtime_modules(
            app,
            [
                LightRuntimeModule::ephemeral(
                    RHYTHM_ADAPTIVE_RUNTIME_ID,
                    &["rhythm", "rhythm_adaptive"],
                    rhythm_adaptive::runtime_manifest,
                    create_test_rhythm_adaptive_runtime,
                ),
                cached_runtime_module(),
            ],
        )
        .expect("test light runtime modules should register");
    }

    fn create_test_rhythm_adaptive_runtime(
        runtime: Arc<dyn RuntimeHandle>,
    ) -> Box<dyn LightRuntime> {
        Box::new(rhythm_adaptive::RuntimeHandleAdaptiveRuntime::new(runtime))
    }

    fn ensure_registered_cached_runtime(state: &SharedState) -> Result<SharedLightRuntime> {
        let module = {
            let s = state.lock().unwrap();
            s.light_runtime_registry
                .module_for_kind(&LightRuntimeKind::new(CACHED_RUNTIME_ID))
                .unwrap()
        };
        let LightRuntimeInstance::Cached { ensure } = module.instance else {
            panic!("cached test module should be cached");
        };
        ensure(state, &module)
    }

    fn ensure_test_cached_runtime(
        state: &SharedState,
        module: &LightRuntimeModule,
    ) -> Result<SharedLightRuntime> {
        let (runtime, runtime_state, current_fingerprint, existing_runtime) = {
            let s = state
                .lock()
                .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
            let runtime = s
                .hub_runtime()
                .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?;
            (
                runtime,
                s.light_runtime_state.get(module.id).cloned(),
                s.light_runtime_config_fingerprint.clone(),
                s.light_runtime.clone(),
            )
        };
        let snapshot = build_runtime_snapshot(runtime.as_ref(), runtime_state.as_ref());
        let fingerprint = snapshot
            .nodes
            .iter()
            .map(|node| node.id.as_str())
            .collect::<Vec<_>>()
            .join("|");
        if current_fingerprint.as_deref() == Some(fingerprint.as_str()) {
            if let Some(existing_runtime) = existing_runtime {
                return Ok(existing_runtime);
            }
        }

        let light_runtime: SharedLightRuntime = Arc::new(Mutex::new(Box::new(CachedTestRuntime)));

        let mut s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        if s.light_runtime_kind.as_str() == module.id
            && (s.light_runtime.is_none()
                || s.light_runtime_config_fingerprint.as_deref() != Some(fingerprint.as_str()))
        {
            s.light_runtime = Some(light_runtime.clone());
            s.light_runtime_config_fingerprint = Some(fingerprint);
        }
        Ok(light_runtime)
    }

    fn test_runtime() -> Arc<dyn RuntimeHandle> {
        use rhythm_core::runtime::registry::SimpleDeviceRegistry;
        use rhythm_core::runtime::scheduler::NoOpScheduler;
        use rhythm_core::runtime::time::MockTimeProvider;
        use rhythm_core::{NoOpController, RhythmRuntime, RuntimeConfig};

        Arc::new(RhythmRuntime::new(
            Arc::new(NoOpController::new()),
            MockTimeProvider::default(),
            NoOpScheduler::new(),
            SimpleDeviceRegistry::new(),
            RuntimeConfig::default(),
        ))
    }
}
