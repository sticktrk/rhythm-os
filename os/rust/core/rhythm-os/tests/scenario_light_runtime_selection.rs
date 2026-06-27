//! Scenario: light-runtime selection.
//!
//! Verifies that the production OS host can run multiple light runtimes
//! against the same synced topology, controller dispatch path, and app-state
//! store. `rhythm-adaptive` is the default runtime, and externally registered
//! runtimes can be selected and switched back without leaking private state.

mod harness;

use harness::*;
use rhythm_os::light_runtime::{self, LightRuntimeKind, RHYTHM_ADAPTIVE_RUNTIME_ID};
use rhythm_os::{commands, handlers};
use rhythm_runtime_api::{
    DispatchCommand, DispatchTarget, InputAction, LightRuntime, LightingCommand, RuntimeEvent,
    RuntimeInputEvent, RuntimeManifest, RuntimePlan, RuntimeSnapshot, StateWrite, TickContext,
};

fn input_event(target_id: &str, action: InputAction) -> RuntimeEvent {
    RuntimeEvent::Input(RuntimeInputEvent {
        source_id: "test-switch".to_string(),
        target_id: target_id.to_string(),
        action,
        epoch_ms: None,
        metadata: Default::default(),
    })
}

fn tick_event(target_id: &str, hour: f64) -> RuntimeEvent {
    RuntimeEvent::PeriodicTick(TickContext {
        node_id: target_id.to_string(),
        hour,
        epoch_ms: None,
        metadata: Default::default(),
    })
}

const SCENARIO_EXTERNAL_RUNTIME_ID: &str = "scenario-lab";
const SCENARIO_EXTERNAL_RUNTIME_ALIAS: &str = "scenario_lab";

#[test]
fn default_runtime_api_and_settings_report_rhythm_adaptive() {
    let harness = TestHarness::new();

    let settings: serde_json::Value =
        serde_json::from_str(&commands::build_settings(&harness.state).unwrap()).unwrap();
    assert_eq!(settings["light_runtime"], RHYTHM_ADAPTIVE_RUNTIME_ID);

    let selector = handlers::handle_get_light_runtime(&harness.state);
    assert_eq!(selector.status, 200);
    let selector: serde_json::Value = serde_json::from_str(&selector.body).unwrap();
    assert_eq!(selector["runtime_id"], RHYTHM_ADAPTIVE_RUNTIME_ID);
    assert!(selector["available_runtime_ids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == RHYTHM_ADAPTIVE_RUNTIME_ID));

    let manifest =
        handlers::handle_get_light_runtime_manifest(&harness.state, RHYTHM_ADAPTIVE_RUNTIME_ID);
    assert_eq!(manifest.status, 200);
    let manifest: serde_json::Value = serde_json::from_str(&manifest.body).unwrap();
    assert_eq!(manifest["id"], RHYTHM_ADAPTIVE_RUNTIME_ID);
    assert_eq!(manifest["name"], "Rhythm Adaptive");
}

#[test]
fn externally_registered_runtime_selects_and_runs_through_host_interfaces() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");
    register_scenario_external_runtime(&harness.state);

    let manifest = handlers::handle_get_light_runtime_manifest(
        &harness.state,
        SCENARIO_EXTERNAL_RUNTIME_ALIAS,
    );
    assert_eq!(manifest.status, 200);
    let manifest: serde_json::Value = serde_json::from_str(&manifest.body).unwrap();
    assert_eq!(manifest["id"], SCENARIO_EXTERNAL_RUNTIME_ID);
    assert_eq!(manifest["name"], "Scenario Lab");

    let selected = handlers::handle_put_light_runtime(
        &harness.state,
        &serde_json::json!({"runtime_id": SCENARIO_EXTERNAL_RUNTIME_ALIAS}),
    );
    assert_eq!(selected.status, 200);
    let selected: serde_json::Value = serde_json::from_str(&selected.body).unwrap();
    assert_eq!(selected["runtime_id"], SCENARIO_EXTERNAL_RUNTIME_ID);
    assert!(selected["available_runtime_ids"]
        .as_array()
        .unwrap()
        .iter()
        .any(|id| id == SCENARIO_EXTERNAL_RUNTIME_ID));

    spy.reset();
    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();

    assert_eq!(report.dispatch_count, 1);
    assert_eq!(report.state_write_count, 1);
    assert_eq!(spy.turn_on_count(), 1);
    assert!(harness.lights_on("kitchen"));

    let state = harness.state.lock().unwrap();
    assert_eq!(
        state.light_runtime_kind.as_str(),
        SCENARIO_EXTERNAL_RUNTIME_ID
    );
    assert_eq!(
        state.light_runtime_state[SCENARIO_EXTERNAL_RUNTIME_ID][&kitchen]
            ["scenario_external_runtime"],
        serde_json::json!("handled")
    );
    assert!(
        !state
            .light_runtime_state
            .contains_key(RHYTHM_ADAPTIVE_RUNTIME_ID),
        "custom runtime state must not leak into built-in runtime state"
    );
}

#[test]
fn default_rhythm_adaptive_dispatches_without_stateful_runtime_instance() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");

    {
        let state = harness.state.lock().unwrap();
        assert_eq!(
            state.light_runtime_kind,
            LightRuntimeKind::rhythm_adaptive()
        );
        assert!(state.light_runtime.is_none());
        assert!(!state
            .light_runtime_state
            .contains_key(RHYTHM_ADAPTIVE_RUNTIME_ID));
    }

    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();

    assert_eq!(report.dispatch_count, 1);
    assert_eq!(report.state_write_count, 0);
    assert_eq!(spy.turn_on_count(), 1);
    assert!(harness.lights_on("kitchen"));

    let state = harness.state.lock().unwrap();
    assert_eq!(
        state.light_runtime_kind,
        LightRuntimeKind::rhythm_adaptive()
    );
    assert!(
        state.light_runtime.is_none(),
        "default rhythm-adaptive runtime must not allocate a cached runtime instance"
    );
    assert!(
        !state
            .light_runtime_state
            .contains_key(RHYTHM_ADAPTIVE_RUNTIME_ID),
        "default rhythm-adaptive dispatch should not create private runtime state"
    );
}

fn register_scenario_external_runtime(state: &rhythm_os::state::SharedState) {
    let mut s = state.lock().unwrap();
    light_runtime::register_light_runtime_module(
        &mut s,
        light_runtime::LightRuntimeModule::ephemeral(
            SCENARIO_EXTERNAL_RUNTIME_ID,
            &[SCENARIO_EXTERNAL_RUNTIME_ALIAS],
            scenario_external_runtime_manifest,
            create_scenario_external_runtime,
        ),
    )
    .expect("scenario external runtime should register");
}

fn scenario_external_runtime_manifest() -> RuntimeManifest {
    RuntimeManifest::new(SCENARIO_EXTERNAL_RUNTIME_ID, "Scenario Lab")
}

fn create_scenario_external_runtime(
    _: std::sync::Arc<dyn rhythm_core::RuntimeHandle>,
) -> Box<dyn LightRuntime> {
    Box::new(ScenarioExternalRuntime)
}

struct ScenarioExternalRuntime;

impl LightRuntime for ScenarioExternalRuntime {
    fn name(&self) -> &str {
        SCENARIO_EXTERNAL_RUNTIME_ID
    }

    fn handle_event(
        &mut self,
        snapshot: &RuntimeSnapshot,
        event: RuntimeEvent,
    ) -> rhythm_runtime_api::RuntimeResult<RuntimePlan> {
        let node_id = match event {
            RuntimeEvent::Input(input) => input.target_id,
            RuntimeEvent::PeriodicTick(tick) => tick.node_id,
            RuntimeEvent::HostStateChanged { .. } => snapshot
                .nodes
                .first()
                .map(|node| node.id.clone())
                .expect("scenario runtime snapshot should contain a node"),
        };

        Ok(RuntimePlan {
            dispatch: vec![DispatchCommand::TurnOn {
                target: DispatchTarget::Node {
                    node_id: node_id.clone(),
                },
                command: LightingCommand::new(73, 3000),
            }],
            state_writes: vec![StateWrite {
                node_id,
                key: "scenario_external_runtime".to_string(),
                value: serde_json::json!("handled"),
            }],
            diagnostics: vec![],
        })
    }
}

#[test]
fn default_rhythm_adaptive_periodic_tick_uses_selected_runtime_path() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");
    spy.reset();

    let report =
        light_runtime::run_selected_light_runtime_event(&harness.state, tick_event(&kitchen, 14.5))
            .unwrap();

    assert_eq!(report.dispatch_count, 1);
    assert_eq!(report.state_write_count, 0);
    assert_eq!(spy.turn_on_count(), 1);
    let state = harness.state.lock().unwrap();
    assert_eq!(
        state.light_runtime_kind,
        LightRuntimeKind::rhythm_adaptive()
    );
    assert!(state.light_runtime.is_none());
}

#[test]
fn selected_light_runtimes_share_os_host_interfaces() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");
    register_scenario_external_runtime(&harness.state);

    // The default runtime runs Rhythm's adaptive planner through the neutral
    // light-runtime host.
    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();
    assert_eq!(report.dispatch_count, 1);
    assert_eq!(
        harness.state.lock().unwrap().light_runtime_kind,
        LightRuntimeKind::rhythm_adaptive()
    );
    assert_eq!(spy.turn_on_count(), 1);

    // A custom runtime uses the same OS host/controller path, and its private
    // runtime state is isolated under its own runtime id.
    commands::do_light_runtime_settings_set(
        &harness.state,
        LightRuntimeKind::new(SCENARIO_EXTERNAL_RUNTIME_ALIAS),
    )
    .unwrap();
    spy.reset();

    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();
    assert_eq!(report.dispatch_count, 1);
    assert_eq!(spy.turn_on_count(), 1);
    {
        let state = harness.state.lock().unwrap();
        assert_eq!(
            state.light_runtime_kind,
            LightRuntimeKind::new(SCENARIO_EXTERNAL_RUNTIME_ID)
        );
        assert!(state.light_runtime.is_none());
        assert_eq!(
            state.light_runtime_state[SCENARIO_EXTERNAL_RUNTIME_ID][&kitchen]
                ["scenario_external_runtime"],
            serde_json::json!("handled")
        );
        assert!(
            !state
                .light_runtime_state
                .get(RHYTHM_ADAPTIVE_RUNTIME_ID)
                .is_some_and(|runtime_state| runtime_state.contains_key(&kitchen)),
            "custom runtime writes must not leak into rhythm-adaptive state"
        );
    }

    // Returning to the built-in runtime dispatches back through Rhythm's
    // adaptive runtime against the same topology.
    commands::do_light_runtime_settings_set(&harness.state, LightRuntimeKind::rhythm_adaptive())
        .unwrap();
    spy.reset();

    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::Off),
    )
    .unwrap();

    assert_eq!(report.dispatch_count, 1);
    assert_eq!(spy.turn_off_count(), 1);
    let state = harness.state.lock().unwrap();
    assert_eq!(
        state.light_runtime_kind,
        LightRuntimeKind::rhythm_adaptive()
    );
    assert!(
        state.light_runtime.is_none(),
        "built-in rhythm-adaptive runtime must not retain a custom runtime instance"
    );
}
