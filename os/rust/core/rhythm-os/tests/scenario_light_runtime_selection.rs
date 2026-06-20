//! Scenario: light-runtime selection.
//!
//! Verifies that the production OS host can run multiple light runtimes
//! against the same synced topology, controller dispatch path, and app-state
//! store. `rhythm-adaptive` is the default Basic runtime; `removed-circadian`
//! is selected for Expert mode and can be switched back without keeping an
//! embedded removed-project runtime alive.

mod harness;

use harness::*;
use rhythm_os::light_runtime::{
    self, LightRuntimeKind, removed_circadian_RUNTIME_ID, RHYTHM_ADAPTIVE_RUNTIME_ID,
};
use rhythm_os::{commands, handlers};
use rhythm_runtime_api::{InputAction, RuntimeEvent, RuntimeInputEvent, TickContext};

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

    let manifest = handlers::handle_get_light_runtime_manifest(RHYTHM_ADAPTIVE_RUNTIME_ID);
    assert_eq!(manifest.status, 200);
    let manifest: serde_json::Value = serde_json::from_str(&manifest.body).unwrap();
    assert_eq!(manifest["id"], RHYTHM_ADAPTIVE_RUNTIME_ID);
    assert_eq!(manifest["name"], "Rhythm Adaptive");
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
        assert_eq!(state.light_runtime_kind, LightRuntimeKind::RhythmAdaptive);
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
    assert_eq!(state.light_runtime_kind, LightRuntimeKind::RhythmAdaptive);
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
    assert_eq!(state.light_runtime_kind, LightRuntimeKind::RhythmAdaptive);
    assert!(state.light_runtime.is_none());
}

#[test]
fn selected_light_runtimes_share_os_host_interfaces() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");

    // Basic mode is the default and runs Rhythm's adaptive planner through the
    // neutral light-runtime host.
    let report = light_runtime::run_selected_light_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();
    assert_eq!(report.dispatch_count, 1);
    assert_eq!(
        harness.state.lock().unwrap().light_runtime_kind,
        LightRuntimeKind::RhythmAdaptive
    );
    assert_eq!(spy.turn_on_count(), 1);

    // Expert mode selects removed-project. It uses the same OS host/controller path,
    // and its private runtime state is isolated under the removed-project runtime id.
    commands::do_light_runtime_settings_set(&harness.state, LightRuntimeKind::removed-projectCircadian)
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
        assert_eq!(state.light_runtime_kind, LightRuntimeKind::removed-projectCircadian);
        assert!(state.light_runtime.is_some());
        assert!(
            state.light_runtime_state[removed_circadian_RUNTIME_ID][&kitchen]
                .contains_key("removed-project_area_runtime_state")
        );
        assert!(
            !state
                .light_runtime_state
                .get(RHYTHM_ADAPTIVE_RUNTIME_ID)
                .is_some_and(|runtime_state| runtime_state.contains_key(&kitchen)),
            "removed-project writes must not leak into rhythm-adaptive state"
        );
    }

    // Returning to Basic mode clears the cached removed-project runtime and dispatches
    // back through Rhythm's adaptive runtime against the same topology.
    commands::do_light_runtime_settings_set(&harness.state, LightRuntimeKind::RhythmAdaptive)
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
    assert_eq!(state.light_runtime_kind, LightRuntimeKind::RhythmAdaptive);
    assert!(
        state.light_runtime.is_none(),
        "Basic mode must not retain the removed-project runtime instance"
    );
}
