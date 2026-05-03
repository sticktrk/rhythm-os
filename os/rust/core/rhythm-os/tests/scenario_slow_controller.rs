//! Scenario: a slow hub controller (e.g. a Hue bridge that hangs for 200ms
//! per call) must not starve other rooms attached to the same engine.
//!
//! These scenarios drive `handle_hub_event` directly so they exercise the
//! real button-ingress path that spawns controller work off the event loop.
//! If the controller blocks, later button ingress must still return quickly,
//! subsequent presses must still be applied, and room state must remain sane.

mod harness;

use async_trait::async_trait;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use harness::{rooms_with_lights, TestHarness};
use rhythm_core::controller::{LightControlResult, LightController};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_core::runtime::events::InputEvent;
use rhythm_core::runtime::handle::RuntimeHandle;
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::runtime::orchestrator::RhythmRuntime;
use rhythm_core::runtime::registry::SimpleDeviceRegistry;
use rhythm_core::runtime::scheduler::NoOpScheduler;
use rhythm_core::runtime::time::MockTimeProvider;
use rhythm_core::runtime::RuntimeConfig;
use rhythm_core::ButtonAction;
use rhythm_os::canonical::identity::{DiscoveredIdentity, HardwareId};
use rhythm_os::canonical::registry::ResolveResult;
use rhythm_os::event_loop::{handle_hub_event, MotionTimerState};
use rhythm_os::hub::HubEvent;
use rhythm_os::topology::DevicePlacement;

fn dispatch_button_event(
    harness: &TestHarness,
    motion: &mut MotionTimerState,
    room_id: &str,
    action: ButtonAction,
    device_id: &str,
) -> Duration {
    let started = Instant::now();
    handle_hub_event(
        &harness.state,
        HubEvent::Button {
            hub_key: Some(harness.hub_key.clone()),
            room_id: room_id.to_string(),
            action,
            device_id: Some(device_id.to_string()),
        },
        motion,
    );
    started.elapsed()
}

fn add_button_source(harness: &TestHarness, native_id: &str, room_id: &str) {
    let topology_room_id = harness.resolve(room_id);
    let identity = DiscoveredIdentity {
        native_id: native_id.to_string(),
        room_id: room_id.to_string(),
        room_name: room_id.to_string(),
        name: native_id.to_string(),
        device_type: DeviceType::Button,
        hardware_ids: vec![HardwareId::matter(native_id)],
        manufacturer: None,
        model: None,
    };

    let mut state = harness.state.lock().unwrap();
    let canonical_id = match state
        .canonical_registry
        .resolve(&identity, &harness.hub_key, 1)
    {
        ResolveResult::Created { canonical_id } | ResolveResult::AlreadyKnown { canonical_id } => {
            canonical_id
        }
        other => panic!("unexpected resolve result: {:?}", other),
    };
    assert!(state
        .canonical_registry
        .assign_room(&canonical_id, Some(&topology_room_id)));
    state.topology.ensure_standalone_device(&canonical_id);
    assert!(state.topology.assign_device(
        &canonical_id,
        Some(&topology_room_id),
        DevicePlacement::UserOverride,
    ));
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool, failure: &str) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(predicate(), "{failure}");
}

struct BlockingController {
    started: Arc<(Mutex<bool>, Condvar)>,
    release: Arc<(Mutex<bool>, Condvar)>,
}

impl BlockingController {
    fn new() -> Self {
        Self {
            started: Arc::new((Mutex::new(false), Condvar::new())),
            release: Arc::new((Mutex::new(false), Condvar::new())),
        }
    }

    fn wait_started(&self, timeout: Duration) -> bool {
        let (lock, cvar) = &*self.started;
        let started = lock.lock().unwrap();
        let (started, _) = cvar
            .wait_timeout_while(started, timeout, |started| !*started)
            .unwrap();
        *started
    }

    fn release(&self) {
        let (lock, cvar) = &*self.release;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }

    fn block_until_released(&self) {
        {
            let (lock, cvar) = &*self.started;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }

        let (lock, cvar) = &*self.release;
        let released = lock.lock().unwrap();
        let _released = cvar.wait_while(released, |released| !*released).unwrap();
    }
}

#[async_trait]
impl LightController for BlockingController {
    async fn turn_on(&self, _room_id: &str, _command: LightingCommand) -> LightControlResult<()> {
        self.block_until_released();
        Ok(())
    }

    async fn turn_off(
        &self,
        _room_id: &str,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(vec![])
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on(&self, _room_id: &str) -> LightControlResult<bool> {
        Ok(false)
    }

    fn name(&self) -> &str {
        "blocking-test"
    }
}

#[test]
fn in_flight_button_command_does_not_hold_engine_state_lock() {
    let controller = Arc::new(BlockingController::new());
    let runtime = Arc::new(RhythmRuntime::new(
        controller.clone(),
        MockTimeProvider::new(14.0, 172, 2026),
        NoOpScheduler::new(),
        SimpleDeviceRegistry::new(),
        RuntimeConfig::default(),
    ));
    RuntimeHandle::add_room(runtime.as_ref(), "room-a", "Room A");
    RuntimeHandle::add_room(runtime.as_ref(), "room-b", "Room B");

    let event_runtime = runtime.clone();
    let event_thread = std::thread::spawn(move || {
        RuntimeHandle::handle_event(
            event_runtime.as_ref(),
            &InputEvent::new("room-a", ButtonAction::OnPress),
        )
    });

    assert!(
        controller.wait_started(Duration::from_secs(1)),
        "test controller should enter the blocking turn_on call"
    );

    let (read_tx, read_rx) = mpsc::channel();
    let read_runtime = runtime.clone();
    let reader_thread = std::thread::spawn(move || {
        let started = Instant::now();
        let snapshots = RuntimeHandle::engine_all_room_snapshots(read_runtime.as_ref());
        read_tx.send((started.elapsed(), snapshots.len())).unwrap();
    });

    let read_result = read_rx.recv_timeout(Duration::from_millis(100));
    controller.release();
    event_thread
        .join()
        .expect("button thread should not panic")
        .expect("button action should complete after release");
    reader_thread
        .join()
        .expect("reader thread should not panic");

    let (elapsed, snapshot_count) =
        read_result.expect("engine snapshots should be readable while controller I/O is in flight");
    assert!(
        elapsed < Duration::from_millis(100),
        "engine snapshot read was blocked by controller I/O for {:?}",
        elapsed
    );
    assert_eq!(snapshot_count, 2);
}

#[test]
fn slow_controller_does_not_drop_button_events_for_other_rooms() {
    let (rooms, devices) =
        rooms_with_lights(&[("mock-kitchen", "Kitchen"), ("mock-bedroom", "Bedroom")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    // Slow the kitchen room's turn_on to 50ms — fast enough to keep the
    // test snappy but obvious if calls are serialised. Bedroom is fast.
    let kitchen_node = harness.resolve("mock-kitchen");
    let bedroom_node = harness.resolve("mock-bedroom");
    add_button_source(&harness, "button-kitchen", "mock-kitchen");
    add_button_source(&harness, "button-bedroom", "mock-bedroom");
    spy.set_turn_on_delay_for_room(&kitchen_node, Duration::from_millis(50));

    let mut motion = MotionTimerState::new();
    let kitchen_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    let bedroom_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-bedroom",
        ButtonAction::OnPress,
        "button-bedroom",
    );

    assert!(
        kitchen_ingress < Duration::from_millis(100),
        "slow kitchen controller must not block first button ingress, got {:?}",
        kitchen_ingress
    );
    assert!(
        bedroom_ingress < Duration::from_millis(100),
        "slow kitchen controller must not block subsequent bedroom ingress, got {:?}",
        bedroom_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || {
            let calls = spy.turn_on_calls();
            let kitchen_on = calls.iter().any(|(room, _)| room == &kitchen_node);
            let bedroom_on = calls.iter().any(|(room, _)| room == &bedroom_node);
            kitchen_on
                && bedroom_on
                && harness.lights_on("mock-kitchen")
                && harness.lights_on("mock-bedroom")
        },
        "both button events should eventually reach the controller and update engine state",
    );
}

#[test]
fn slow_lights_off_does_not_lose_subsequent_on_press() {
    // Hard-off (`lights_off`) is the only action that drives `turn_off` in
    // the engine — `off` is a soft-off implemented as a low-brightness
    // turn_on. Drive a slow turn_off then an on press and verify both
    // reach the controller and the room ends up on.
    let (rooms, devices) = rooms_with_lights(&[("mock-kitchen", "Kitchen")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    let mut motion = MotionTimerState::new();
    add_button_source(&harness, "button-kitchen", "mock-kitchen");
    spy.set_turn_off_delay(Duration::from_millis(300));

    harness.action("mock-kitchen", "on").unwrap();
    spy.reset();

    let lights_off_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::LightsOff,
        "button-kitchen",
    );
    assert!(
        lights_off_ingress < Duration::from_millis(100),
        "slow lights_off must not block ingress, got {:?}",
        lights_off_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || spy.turn_off_count() == 1 && !harness.lights_on("mock-kitchen"),
        "slow lights_off should still complete through the ingress path",
    );

    let on_ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    assert!(
        on_ingress < Duration::from_millis(100),
        "follow-up on press must not be blocked after a slow lights_off, got {:?}",
        on_ingress
    );

    wait_until(
        Duration::from_secs(2),
        || {
            spy.turn_off_count() == 1
                && spy.turn_on_count() >= 1
                && harness.lights_on("mock-kitchen")
        },
        "slow lights_off should not lose the subsequent on press",
    );
}

#[test]
fn slow_controller_preserves_room_state_across_actions() {
    // Engine state mutations (e.g. preferences) should remain consistent
    // even when the controller is artificially slow — the engine doesn't
    // require a controller ack before updating its own state.
    let (rooms, devices) = rooms_with_lights(&[("mock-kitchen", "Kitchen")]);

    let (harness, spy) = TestHarness::with_spy_controller();
    let harness = harness.with_discovery(rooms, devices);
    harness.sync();

    spy.set_turn_on_delay(Duration::from_millis(60));
    add_button_source(&harness, "button-kitchen", "mock-kitchen");

    harness.set_room_preferences("mock-kitchen", Some(true), None, Some(false));
    let mut motion = MotionTimerState::new();
    let ingress = dispatch_button_event(
        &harness,
        &mut motion,
        "mock-kitchen",
        ButtonAction::OnPress,
        "button-kitchen",
    );
    assert!(
        ingress < Duration::from_millis(100),
        "slow turn_on must not block button ingress, got {:?}",
        ingress
    );

    wait_until(
        Duration::from_secs(2),
        || spy.turn_on_count() >= 1 && harness.lights_on("mock-kitchen"),
        "slow turn_on should still reach the controller and update room state",
    );

    let snap = harness.snapshot("mock-kitchen").unwrap();
    assert!(
        snap.rhythm_enabled,
        "rhythm_enabled preference must persist through a slow turn_on"
    );
    assert!(harness.lights_on("mock-kitchen"));
}
