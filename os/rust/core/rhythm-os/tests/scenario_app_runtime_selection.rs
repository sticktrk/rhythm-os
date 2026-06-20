//! Scenario: app-runtime selection.
//!
//! Verifies that the production OS host can run multiple light app runtimes
//! against the same synced topology, controller dispatch path, and app-state
//! store. `rhythm-adaptive` is the default Basic runtime; `removed-circadian`
//! is selected for Expert mode and can be switched back without keeping an
//! embedded removed-project runtime alive.

mod harness;

use harness::*;
use rhythm_os::app_runtime::{
    self, LightingRuntimeKind, removed_circadian_RUNTIME_ID, RHYTHM_ADAPTIVE_RUNTIME_ID,
};
use rhythm_os::commands;
use rhythm_runtime_api::{InputAction, RuntimeEvent, RuntimeInputEvent};

fn input_event(target_id: &str, action: InputAction) -> RuntimeEvent {
    RuntimeEvent::Input(RuntimeInputEvent {
        source_id: "test-switch".to_string(),
        target_id: target_id.to_string(),
        action,
        epoch_ms: None,
        metadata: Default::default(),
    })
}

#[test]
fn selected_light_app_runtimes_share_os_host_interfaces() {
    let (rooms, devices) = rooms_with_lights(&[("kitchen", "Kitchen")]);
    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();
    let kitchen = harness.resolve("kitchen");

    // Basic mode is the default and runs Rhythm's adaptive planner through the
    // neutral app-runtime host.
    let report = app_runtime::run_selected_app_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();
    assert_eq!(report.dispatch_count, 1);
    assert_eq!(
        harness.state.lock().unwrap().lighting_runtime_kind,
        LightingRuntimeKind::RhythmAdaptive
    );
    assert_eq!(spy.turn_on_count(), 1);

    // Expert mode selects removed-project. It uses the same OS host/controller path,
    // and its private runtime state is isolated under the removed-project runtime id.
    commands::do_lighting_runtime_set(&harness.state, LightingRuntimeKind::removed-projectCircadian)
        .unwrap();
    spy.reset();

    let report = app_runtime::run_selected_app_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::On),
    )
    .unwrap();
    assert_eq!(report.dispatch_count, 1);
    assert_eq!(spy.turn_on_count(), 1);
    {
        let state = harness.state.lock().unwrap();
        assert_eq!(
            state.lighting_runtime_kind,
            LightingRuntimeKind::removed-projectCircadian
        );
        assert!(state.app_runtime.is_some());
        assert!(
            state.app_runtime_state[removed_circadian_RUNTIME_ID][&kitchen]
                .contains_key("removed-project_area_runtime_state")
        );
        assert!(
            !state
                .app_runtime_state
                .get(RHYTHM_ADAPTIVE_RUNTIME_ID)
                .is_some_and(|runtime_state| runtime_state.contains_key(&kitchen)),
            "removed-project writes must not leak into rhythm-adaptive state"
        );
    }

    // Returning to Basic mode clears the cached removed-project runtime and dispatches
    // back through Rhythm's adaptive runtime against the same topology.
    commands::do_lighting_runtime_set(&harness.state, LightingRuntimeKind::RhythmAdaptive).unwrap();
    spy.reset();

    let report = app_runtime::run_selected_app_runtime_event(
        &harness.state,
        input_event(&kitchen, InputAction::Off),
    )
    .unwrap();

    assert_eq!(report.dispatch_count, 1);
    assert_eq!(spy.turn_off_count(), 1);
    let state = harness.state.lock().unwrap();
    assert_eq!(
        state.lighting_runtime_kind,
        LightingRuntimeKind::RhythmAdaptive
    );
    assert!(
        state.app_runtime.is_none(),
        "Basic mode must not retain the removed-project runtime instance"
    );
}
