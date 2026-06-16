//! Transport error propagation tests for rhythm-hue.
//!
//! These exercise the failure paths the real reqwest transport surfaces to
//! the controller (network drop, bridge rejecting auth with 401, rate-limit
//! 429, 5xx) using the `SpyHueTransport::set_should_fail` toggle. The rpiz
//! appliance must never panic on transport failures; it must propagate the
//! error cleanly so the engine can log and retry on the next tick.

use std::sync::{Arc, Mutex};

use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::{ButtonAction, InputEvent};

use rhythm_hue::controller::HueLightController;
use rhythm_hue::test_support::SpyHueTransport;

use rhythm_os::registry::HubDeviceRegistry;

fn make_pipeline() -> (Arc<dyn RuntimeHandle>, Arc<SpyHueTransport>) {
    let spy = Arc::new(SpyHueTransport::new());
    let registry = Arc::new(Mutex::new(HubDeviceRegistry::new()));
    registry
        .lock()
        .unwrap()
        .upsert_room("room1", "Living Room", "gl-room1", &[]);
    registry.lock().unwrap().upsert_device(
        "switch-1",
        Some("room1"),
        &[("btn-1".to_string(), 1)],
        DeviceType::Button,
    );

    let controller = HueLightController::new(spy.clone(), "testuser".to_string(), registry.clone());

    let runtime = RhythmRuntime::new(
        Arc::new(controller),
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    );
    runtime.add_room("room1", "Living Room");

    (Arc::new(runtime), spy)
}

#[test]
fn engine_does_not_panic_when_transport_returns_error() {
    let (runtime, spy) = make_pipeline();
    spy.set_should_fail(true);

    // A failing transport must not unwind the engine — handle_event returns
    // an error (or Ok without a pending command), but must not panic or
    // corrupt state.
    let input = InputEvent::new("room1", ButtonAction::OnPress);
    let result = runtime.handle_event(&input);
    assert!(
        result.is_err() || spy.set_grouped_light_calls().is_empty(),
        "with transport failing, either the handler returns Err or the transport wasn't called"
    );
}

#[test]
fn engine_recovers_after_transient_transport_failure() {
    let (runtime, spy) = make_pipeline();

    // First attempt against a failing transport (simulating a 5xx / network
    // drop). Engine must not crash.
    spy.set_should_fail(true);
    let _ = runtime.handle_event(&InputEvent::new("room1", ButtonAction::OnPress));

    // Transport recovers. A subsequent event must succeed.
    spy.set_should_fail(false);
    spy.reset();
    runtime
        .handle_event(&InputEvent::new("room1", ButtonAction::OnPress))
        .expect("healthy transport must succeed after transient failure");

    assert!(
        !spy.set_grouped_light_calls().is_empty(),
        "healthy transport after recovery should receive the command"
    );
}

#[test]
fn repeated_transport_failures_do_not_leak_unbounded_state() {
    // A flaky bridge that keeps returning errors must not cause unbounded
    // call-log growth inside the spy or any other observable side effect in
    // the engine (we can't measure engine-internal state directly, but we
    // assert there's no panic and that each attempt is independent).
    let (runtime, spy) = make_pipeline();
    spy.set_should_fail(true);

    for _ in 0..50 {
        let _ = runtime.handle_event(&InputEvent::new("room1", ButtonAction::OnPress));
    }

    // When the transport finally recovers, a single success call still works.
    spy.set_should_fail(false);
    spy.reset();
    runtime
        .handle_event(&InputEvent::new("room1", ButtonAction::OnPress))
        .expect("transport recovery works after many prior failures");
    assert!(!spy.set_grouped_light_calls().is_empty());
}

#[test]
fn failing_transport_on_off_press_does_not_corrupt_next_on_press() {
    // OnPress with failing transport, then OffPress with healthy transport:
    // the room's engine state must still be in a valid configuration so the
    // next OnPress dispatches normally.
    let (runtime, spy) = make_pipeline();

    spy.set_should_fail(true);
    let _ = runtime.handle_event(&InputEvent::new("room1", ButtonAction::OnPress));

    spy.set_should_fail(false);
    spy.reset();

    runtime
        .handle_event(&InputEvent::new("room1", ButtonAction::OffPress))
        .expect("OffPress after failed OnPress must not error");
    runtime
        .handle_event(&InputEvent::new("room1", ButtonAction::OnPress))
        .expect("OnPress after transport recovery must not error");

    assert!(
        !spy.set_grouped_light_calls().is_empty(),
        "commands after transport recovery must reach the transport"
    );
}
