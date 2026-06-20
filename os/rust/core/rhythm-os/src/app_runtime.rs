//! Host adapter for plan-based light app runtimes.
//!
//! `rhythm-os` owns topology, dispatch, persistence, HTTP/auth, and logging.
//! Light applications own behavior: they receive a neutral snapshot/event and
//! return a neutral plan for the host to apply.

use anyhow::Result;
use removed_circadian::{
    removed-projectAreaConfig, removed-projectAreaEntry, removed-projectDesignerConfig, removed-projectRuntime, removed-projectStateFile,
    removed-projectZoneConfig,
};
use rhythm_core::{
    core_lighting_command_from_runtime, runtime_node_kind_from_light_node_kind, RuntimeHandle,
};
use rhythm_runtime_api::{
    DiagnosticLevel, DispatchCommand, DispatchTarget, LightingRuntime, NodeState, RuntimeEvent,
    RuntimeNode, RuntimeNodeKind, RuntimePlan, RuntimeSnapshot, StateWrite,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::{Arc, Mutex};

use crate::state::AppState;
use crate::state::SharedState;
use crate::storage::StoredAppRuntimeState;

pub const RHYTHM_ADAPTIVE_RUNTIME_ID: &str = "rhythm-adaptive";
pub const removed_circadian_RUNTIME_ID: &str = "removed-circadian";

pub type SharedLightingRuntime = Arc<Mutex<Box<dyn LightingRuntime>>>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum LightingRuntimeKind {
    #[serde(
        rename = "rhythm-adaptive",
        alias = "rhythm",
        alias = "rhythm_adaptive"
    )]
    #[default]
    RhythmAdaptive,
    #[serde(
        rename = "removed-circadian",
        alias = "removed-project-circadian",
        alias = "removed-project",
        alias = "removed_circadian"
    )]
    removed-projectCircadian,
}

impl LightingRuntimeKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::RhythmAdaptive => RHYTHM_ADAPTIVE_RUNTIME_ID,
            Self::removed-projectCircadian => removed_circadian_RUNTIME_ID,
        }
    }
}

impl FromStr for LightingRuntimeKind {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        match value {
            RHYTHM_ADAPTIVE_RUNTIME_ID | "rhythm" | "rhythm_adaptive" => Ok(Self::RhythmAdaptive),
            removed_circadian_RUNTIME_ID | "removed-project" | "removed_circadian" => {
                Ok(Self::removed-projectCircadian)
            }
            _ => Err(anyhow::anyhow!("unknown lighting runtime '{}'", value)),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RuntimePlanApplyReport {
    pub dispatch_count: usize,
    pub state_write_count: usize,
    pub diagnostic_count: usize,
}

/// Build the neutral snapshot passed to app runtimes.
pub fn build_runtime_snapshot(
    runtime: &dyn RuntimeHandle,
    runtime_state: Option<&BTreeMap<String, BTreeMap<String, serde_json::Value>>>,
) -> RuntimeSnapshot {
    let mut snapshot = RuntimeSnapshot::default();

    for node in runtime.engine_all_effective_node_snapshots() {
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
            s.app_runtime_state.get(runtime_id).cloned(),
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

pub fn run_app_runtime_event(
    state: &SharedState,
    runtime_id: &str,
    app_runtime: &mut dyn LightingRuntime,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    validate_runtime_selection(runtime_id, app_runtime, &event)?;
    let snapshot = build_runtime_snapshot_from_state(state, runtime_id)?;
    let plan = app_runtime
        .handle_event(&snapshot, event)
        .map_err(|error| anyhow::anyhow!("{} runtime failed: {}", app_runtime.name(), error))?;
    apply_runtime_plan(state, runtime_id, &plan)
}

pub fn apply_runtime_plan_to_handle(
    state: &SharedState,
    runtime_id: &str,
    runtime: &dyn RuntimeHandle,
    plan: &RuntimePlan,
) -> Result<RuntimePlanApplyReport> {
    emit_diagnostics(runtime_id, &plan.diagnostics);

    for command in &plan.dispatch {
        apply_dispatch_command(runtime, command)?;
        update_host_state_after_dispatch(state, runtime, command);
    }

    let state_write_count = apply_state_writes(state, runtime_id, &plan.state_writes)?;

    Ok(RuntimePlanApplyReport {
        dispatch_count: plan.dispatch.len(),
        state_write_count,
        diagnostic_count: plan.diagnostics.len(),
    })
}

pub fn run_selected_app_runtime_event(
    state: &SharedState,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    let (kind, runtime) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        (
            s.lighting_runtime_kind,
            s.hub_runtime()
                .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?,
        )
    };

    match kind {
        LightingRuntimeKind::RhythmAdaptive => {
            let mut app_runtime = rhythm_adaptive::RuntimeHandleAdaptiveRuntime::new(runtime);
            let host_runtime = app_runtime.inner().clone();
            run_app_runtime_event_with_handle(
                state,
                kind.as_str(),
                host_runtime.as_ref(),
                &mut app_runtime,
                event,
            )
        }
        LightingRuntimeKind::removed-projectCircadian => {
            ensure_removed-project_runtime(state)?;
            let app_runtime = {
                let s = state
                    .lock()
                    .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
                s.app_runtime
                    .clone()
                    .ok_or_else(|| anyhow::anyhow!("removed-project runtime is not initialized"))?
            };
            let mut app_runtime = app_runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("app runtime lock poisoned"))?;
            run_app_runtime_event_with_handle(
                state,
                kind.as_str(),
                runtime.as_ref(),
                app_runtime.as_mut(),
                event,
            )
        }
    }
}

pub fn run_app_runtime_event_with_handle(
    state: &SharedState,
    runtime_id: &str,
    host_runtime: &dyn RuntimeHandle,
    app_runtime: &mut dyn LightingRuntime,
    event: RuntimeEvent,
) -> Result<RuntimePlanApplyReport> {
    validate_runtime_selection(runtime_id, app_runtime, &event)?;
    let runtime_state = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        s.app_runtime_state.get(runtime_id).cloned()
    };
    let snapshot = build_runtime_snapshot(host_runtime, runtime_state.as_ref());
    let plan = app_runtime
        .handle_event(&snapshot, event)
        .map_err(|error| anyhow::anyhow!("{} runtime failed: {}", app_runtime.name(), error))?;
    apply_runtime_plan_to_handle(state, runtime_id, host_runtime, &plan)
}

fn validate_runtime_selection(
    runtime_id: &str,
    app_runtime: &dyn LightingRuntime,
    event: &RuntimeEvent,
) -> Result<()> {
    if app_runtime.runtime_id() != runtime_id {
        return Err(anyhow::anyhow!(
            "selected lighting runtime '{}' returned mismatched runtime id '{}'",
            runtime_id,
            app_runtime.runtime_id()
        ));
    }

    let capabilities = app_runtime.capabilities();
    match event {
        RuntimeEvent::Input(_) if !capabilities.input_events => Err(anyhow::anyhow!(
            "selected lighting runtime '{}' does not support input events",
            runtime_id
        )),
        RuntimeEvent::PeriodicTick(_) if !capabilities.periodic_ticks => Err(anyhow::anyhow!(
            "selected lighting runtime '{}' does not support periodic ticks",
            runtime_id
        )),
        RuntimeEvent::HostStateChanged { .. } if !capabilities.host_state_changes => {
            Err(anyhow::anyhow!(
                "selected lighting runtime '{}' does not support host state changes",
                runtime_id
            ))
        }
        RuntimeEvent::Input(_)
        | RuntimeEvent::PeriodicTick(_)
        | RuntimeEvent::HostStateChanged { .. } => Ok(()),
    }
}

pub fn ensure_removed-project_runtime(state: &SharedState) -> Result<()> {
    let (runtime, runtime_state, hour, sun_times, current_fingerprint, has_runtime) = {
        let s = state
            .lock()
            .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
        if s.lighting_runtime_kind != LightingRuntimeKind::removed-projectCircadian {
            return Ok(());
        }
        let runtime = s
            .hub_runtime()
            .ok_or_else(|| anyhow::anyhow!("no runtime is available"))?;
        (
            runtime,
            s.app_runtime_state
                .get(removed_circadian_RUNTIME_ID)
                .cloned(),
            s.hub_runtime()
                .map(|runtime| runtime.current_hour() as f64)
                .unwrap_or(12.0),
            removed-project_sun_times_from_state(&s),
            s.app_runtime_config_fingerprint.clone(),
            s.app_runtime.is_some(),
        )
    };
    let snapshot = build_runtime_snapshot(runtime.as_ref(), runtime_state.as_ref());
    let fingerprint = removed-project_config_fingerprint_from_snapshot(&snapshot);
    if has_runtime && current_fingerprint.as_deref() == Some(fingerprint.as_str()) {
        return Ok(());
    }

    let config = removed-project_designer_config_from_snapshot(&snapshot);
    let state_file = removed-project_state_file_from_store(runtime_state.as_ref());
    let mut app_runtime = removed-projectRuntime::new();
    app_runtime.load_designer_config(&config, &state_file, hour, sun_times, 0.0);

    let mut s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    if s.lighting_runtime_kind == LightingRuntimeKind::removed-projectCircadian && s.app_runtime.is_none() {
        s.app_runtime = Some(Arc::new(Mutex::new(Box::new(app_runtime))));
        s.app_runtime_config_fingerprint = Some(fingerprint);
    } else if s.lighting_runtime_kind == LightingRuntimeKind::removed-projectCircadian
        && s.app_runtime_config_fingerprint.as_deref() != Some(fingerprint.as_str())
    {
        s.app_runtime = Some(Arc::new(Mutex::new(Box::new(app_runtime))));
        s.app_runtime_config_fingerprint = Some(fingerprint);
    }
    Ok(())
}

pub fn reset_app_runtime_for_kind(s: &mut AppState, kind: LightingRuntimeKind) {
    s.lighting_runtime_kind = kind;
    s.app_runtime = None;
    s.app_runtime_config_fingerprint = None;
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
    apply_state_writes_locked(&mut s.app_runtime_state, runtime_id, writes);
    if let Some(storage) = s.storage.as_ref() {
        storage.save_app_runtime_state(&s.app_runtime_state)?;
    }
    Ok(writes.len())
}

pub fn apply_state_writes_locked(
    store: &mut StoredAppRuntimeState,
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
                log::debug!(target: "app_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Info => {
                log::info!(target: "app_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Warn => {
                log::warn!(target: "app_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
            DiagnosticLevel::Error => {
                log::error!(target: "app_runtime", "{}: {}", runtime_id, diagnostic.message)
            }
        }
    }
}

fn removed-project_designer_config_from_snapshot(snapshot: &RuntimeSnapshot) -> removed-projectDesignerConfig {
    let areas = snapshot
        .nodes
        .iter()
        .filter(|node| node.kind == RuntimeNodeKind::Area)
        .map(|node| {
            removed-projectAreaEntry::Object(removed-projectAreaConfig {
                id: node.id.clone(),
                name: Some(node.name.clone()),
                ..removed-projectAreaConfig::default()
            })
        })
        .collect();

    let mut glozones = BTreeMap::new();
    glozones.insert(
        removed_circadian::config::INITIAL_ZONE_NAME.to_string(),
        removed-projectZoneConfig {
            areas,
            is_default: true,
            ..removed-projectZoneConfig::default()
        },
    );

    removed-projectDesignerConfig {
        glozones,
        ..removed-projectDesignerConfig::default()
    }
}

fn removed-project_config_fingerprint_from_snapshot(snapshot: &RuntimeSnapshot) -> String {
    let mut areas = snapshot
        .nodes
        .iter()
        .filter(|node| node.kind == RuntimeNodeKind::Area)
        .map(|node| format!("{}\0{}", node.id, node.name))
        .collect::<Vec<_>>();
    areas.sort();
    areas.join("\u{1f}")
}

fn removed-project_state_file_from_store(
    runtime_state: Option<&BTreeMap<String, BTreeMap<String, serde_json::Value>>>,
) -> removed-projectStateFile {
    let mut state = removed-projectStateFile::default();
    let Some(runtime_state) = runtime_state else {
        return state;
    };

    for (node_id, values) in runtime_state {
        if let Some(value) = values.get("removed-project_area_runtime_state") {
            if let Ok(area) = serde_json::from_value(value.clone()) {
                state.areas.insert(node_id.clone(), area);
            }
        }
        if let Some(value) = values.get("removed-project_zone_runtime_state") {
            if let Ok(zone) = serde_json::from_value(value.clone()) {
                state.zones.insert(node_id.clone(), zone);
            }
        }
    }

    state
}

fn removed-project_sun_times_from_state(s: &AppState) -> removed_circadian::SunTimes {
    let mut sun_times = removed_circadian::SunTimes::default();
    let Some(lat) = s.latitude else {
        return sun_times;
    };
    let Some(lon) = s.longitude else {
        return sun_times;
    };

    let Some(tz_name) = s.timezone_name.as_deref() else {
        sun_times.solar_noon = s.solar_noon_hour() as f64;
        sun_times.solar_mid = (sun_times.solar_noon + 12.0) % 24.0;
        return sun_times;
    };
    let tz = rhythm_core::Timezone::new(tz_name);
    let (year, month, day, _) = tz.local_date_hour_from_utc(chrono::Utc::now().naive_utc());
    let core = rhythm_core::calculate_sun_times(lat, lon, year, month, day, &tz);
    sun_times.sunrise = core.sunrise as f64;
    sun_times.sunset = core.sunset as f64;
    sun_times.solar_noon = s.solar_noon_hour() as f64;
    sun_times.solar_mid = (sun_times.solar_noon + 12.0) % 24.0;
    sun_times.outdoor_source = "host".to_string();
    sun_times
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::canonical::identity::HubKey;
    use crate::hub::{ActiveHub, HubType};
    use rhythm_runtime_api::{
        DiagnosticLevel, InputAction, LightingCommand, RuntimeDiagnostic, RuntimeInputEvent,
    };
    use std::sync::atomic::AtomicBool;
    use std::sync::Arc;

    #[test]
    fn state_writes_are_namespaced_by_runtime_and_node() {
        let mut store = StoredAppRuntimeState::new();
        apply_state_writes_locked(
            &mut store,
            "removed-project",
            &[StateWrite {
                node_id: "kitchen".to_string(),
                key: "removed-project_area_runtime_state".to_string(),
                value: json!({ "is_on": true }),
            }],
        );

        assert_eq!(
            store["removed-project"]["kitchen"]["removed-project_area_runtime_state"],
            json!({ "is_on": true })
        );
    }

    #[test]
    fn run_app_runtime_event_builds_snapshot_and_applies_plan() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        let mut app_runtime = SnapshotRuntime { saw_power: None };

        let report = run_app_runtime_event(
            &state,
            "snapshot-runtime",
            &mut app_runtime,
            RuntimeEvent::HostStateChanged {
                reason: "test".to_string(),
            },
        )
        .unwrap();

        assert_eq!(app_runtime.saw_power, Some(true));
        assert_eq!(report.dispatch_count, 1);
        assert_eq!(report.state_write_count, 1);
        assert_eq!(
            state.lock().unwrap().app_runtime_state["snapshot-runtime"]["kitchen"]["last"],
            json!("host-call")
        );
    }

    #[test]
    fn rejects_mismatched_selected_runtime_id() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        let mut app_runtime = SnapshotRuntime { saw_power: None };

        let error = run_app_runtime_event(
            &state,
            "removed-circadian",
            &mut app_runtime,
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

        let report = run_selected_app_runtime_event(
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
            state.lock().unwrap().lighting_runtime_kind.as_str(),
            "rhythm-adaptive"
        );
        assert_eq!(report.dispatch_count, 1);
    }

    #[test]
    fn selected_removed-project_runtime_runs_and_persists_state() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime);
        {
            let mut s = state.lock().unwrap();
            reset_app_runtime_for_kind(&mut s, LightingRuntimeKind::removed-projectCircadian);
        }

        let report = run_selected_app_runtime_event(
            &state,
            RuntimeEvent::Input(RuntimeInputEvent {
                source_id: "switch-a".to_string(),
                target_id: "kitchen".to_string(),
                action: InputAction::Named("circadian_on".to_string()),
                epoch_ms: None,
                metadata: Default::default(),
            }),
        )
        .unwrap();

        assert_eq!(report.dispatch_count, 1);
        assert!(state.lock().unwrap().app_runtime.is_some());
        assert!(
            state.lock().unwrap().app_runtime_state[removed_circadian_RUNTIME_ID]["kitchen"]
                .contains_key("removed-project_area_runtime_state")
        );
    }

    #[test]
    fn removed-project_runtime_rebuilds_when_host_topology_changes() {
        let runtime = test_runtime();
        runtime.add_node("kitchen", "Kitchen", rhythm_core::LightNodeKind::Room, None);
        let state = make_state_with_runtime(runtime.clone());
        {
            let mut s = state.lock().unwrap();
            reset_app_runtime_for_kind(&mut s, LightingRuntimeKind::removed-projectCircadian);
        }

        ensure_removed-project_runtime(&state).unwrap();
        let (first_runtime, first_fingerprint) = {
            let s = state.lock().unwrap();
            (
                s.app_runtime.clone().unwrap(),
                s.app_runtime_config_fingerprint.clone().unwrap(),
            )
        };

        runtime.add_node("bedroom", "Bedroom", rhythm_core::LightNodeKind::Room, None);
        ensure_removed-project_runtime(&state).unwrap();

        let s = state.lock().unwrap();
        let second_runtime = s.app_runtime.clone().unwrap();
        let second_fingerprint = s.app_runtime_config_fingerprint.clone().unwrap();
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
            state.lock().unwrap().app_runtime_state["test-runtime"]["kitchen"]["last"],
            json!("tick")
        );
    }

    struct SnapshotRuntime {
        saw_power: Option<bool>,
    }

    impl LightingRuntime for SnapshotRuntime {
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

    fn make_state_with_runtime(runtime: Arc<dyn RuntimeHandle>) -> SharedState {
        let mut app = crate::state::AppState::default();
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
