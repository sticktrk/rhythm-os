use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rhythm_core::composite_controller::{HubDispatchPolicy, HubDispatchTimeoutScope};
use rhythm_core::{
    CompositeController, HubDispatchTarget, HubLightController, LightControlError,
    LightControlResult, LightController, LightingCommand, Room,
};

struct SlowHubController {
    started: (Mutex<bool>, Condvar),
    release: (Mutex<bool>, Condvar),
    turn_on_calls: AtomicUsize,
    turn_off_calls: AtomicUsize,
}

struct FlashFallbackController {
    was_on: bool,
    turn_on_calls: AtomicUsize,
    turn_off_calls: AtomicUsize,
}

impl FlashFallbackController {
    fn new(was_on: bool) -> Self {
        Self {
            was_on,
            turn_on_calls: AtomicUsize::new(0),
            turn_off_calls: AtomicUsize::new(0),
        }
    }
}

impl SlowHubController {
    fn new() -> Self {
        Self {
            started: (Mutex::new(false), Condvar::new()),
            release: (Mutex::new(false), Condvar::new()),
            turn_on_calls: AtomicUsize::new(0),
            turn_off_calls: AtomicUsize::new(0),
        }
    }

    fn wait_started(&self) {
        let (lock, cvar) = &self.started;
        let started = lock.lock().unwrap();
        let _guard = cvar
            .wait_timeout_while(started, Duration::from_secs(5), |started| !*started)
            .unwrap();
    }

    fn release(&self) {
        let (lock, cvar) = &self.release;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }

    fn turn_on_count(&self) -> usize {
        self.turn_on_calls.load(Ordering::SeqCst)
    }

    fn turn_off_count(&self) -> usize {
        self.turn_off_calls.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl HubLightController for SlowHubController {
    async fn turn_on_target(
        &self,
        _target: &HubDispatchTarget,
        _command: LightingCommand,
    ) -> LightControlResult<()> {
        self.turn_on_calls.fetch_add(1, Ordering::SeqCst);
        {
            let (lock, cvar) = &self.started;
            *lock.lock().unwrap() = true;
            cvar.notify_all();
        }
        let (lock, cvar) = &self.release;
        let mut release = lock.lock().unwrap();
        while !*release {
            release = cvar.wait(release).unwrap();
        }
        Ok(())
    }

    async fn turn_off_target(
        &self,
        _target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.turn_off_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(Vec::new())
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on_target(&self, _target: &HubDispatchTarget) -> LightControlResult<bool> {
        Ok(false)
    }

    fn name(&self) -> &str {
        "slow-hub"
    }
}

#[async_trait]
impl HubLightController for FlashFallbackController {
    async fn turn_on_target(
        &self,
        _target: &HubDispatchTarget,
        _command: LightingCommand,
    ) -> LightControlResult<()> {
        self.turn_on_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn turn_off_target(
        &self,
        _target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        self.turn_off_calls.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        Ok(Vec::new())
    }

    async fn is_connected(&self) -> bool {
        true
    }

    async fn any_lights_on_target(&self, _target: &HubDispatchTarget) -> LightControlResult<bool> {
        Ok(self.was_on)
    }

    fn name(&self) -> &str {
        "flash-fallback"
    }
}

fn route_room(composite: &CompositeController, hub_key: &str, room_id: &str) {
    composite.update_routing(HashMap::from([(
        room_id.to_string(),
        vec![(
            hub_key.to_string(),
            HubDispatchTarget::Group {
                room_id: room_id.to_string(),
                control_id: room_id.to_string(),
            },
        )],
    )]));
}

fn slow_hub_policy(dispatch_timeout: Duration, timeout_cooldown: Duration) -> HubDispatchPolicy {
    HubDispatchPolicy {
        max_in_flight: 2,
        rate_limit: None,
        split_device_targets: false,
        dispatch_timeout,
        timeout_cooldown,
        timeout_scope: HubDispatchTimeoutScope::Target,
        query_timeout: Duration::from_secs(1),
    }
}

fn wait_until(timeout: Duration, predicate: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    predicate()
}

/// A slow in-flight command must never block or reject later commands:
/// they are accepted, coalesce latest-wins, and drain after release.
#[tokio::test(flavor = "multi_thread")]
async fn slow_inflight_command_coalesces_followups_and_never_rejects() {
    let controller = Arc::new(SlowHubController::new());
    let composite = CompositeController::new();
    composite.register_controller_with_policy(
        "matter@local",
        controller.clone(),
        slow_hub_policy(Duration::from_secs(10), Duration::from_millis(50)),
    );
    route_room(&composite, "matter@local", "kitchen");

    composite
        .turn_on("kitchen", LightingCommand::new(80, 4000))
        .await
        .unwrap();
    controller.wait_started();

    // Both accepted while the first dispatch is stuck; they coalesce into
    // one trailing dispatch instead of overflowing a queue.
    composite
        .turn_on("kitchen", LightingCommand::new(70, 3500))
        .await
        .unwrap();
    composite
        .turn_on("kitchen", LightingCommand::new(60, 3200))
        .await
        .unwrap();

    controller.release();
    assert!(wait_until(Duration::from_secs(5), || {
        controller.turn_on_count() == 2
    }));

    composite.turn_off("kitchen", Some(120)).await.unwrap();
    assert!(wait_until(Duration::from_secs(5), || {
        controller.turn_off_count() == 1
    }));

    assert_eq!(controller.turn_on_count(), 2);
    assert_eq!(controller.turn_off_count(), 1);
}

/// A dispatch timeout cools the target down (enqueue rejected with a
/// Timeout error), then commands flow again once the cooldown expires.
#[tokio::test(flavor = "multi_thread")]
async fn dispatch_timeout_cools_down_then_recovers_after_slow_command_finishes() {
    let controller = Arc::new(SlowHubController::new());
    let composite = CompositeController::new();
    composite.register_controller_with_policy(
        "hue@bridge",
        controller.clone(),
        slow_hub_policy(Duration::from_millis(40), Duration::from_millis(80)),
    );
    route_room(&composite, "hue@bridge", "kitchen");

    // Accepted (dispatch is fire-and-forget), then times out and cools down.
    composite
        .turn_on("kitchen", LightingCommand::new(80, 4000))
        .await
        .unwrap();
    controller.wait_started();

    assert!(wait_until(Duration::from_secs(5), || {
        let retry = futures::executor::block_on(
            composite.turn_on("kitchen", LightingCommand::new(70, 3500)),
        );
        matches!(retry, Err(LightControlError::Timeout(_)))
    }));

    controller.release();

    // After the cooldown expires the target accepts and dispatches again.
    assert!(wait_until(Duration::from_secs(5), || {
        futures::executor::block_on(composite.turn_on("kitchen", LightingCommand::new(60, 3200)))
            .is_ok()
    }));
    assert!(wait_until(Duration::from_secs(5), || {
        controller.turn_on_count() == 2
    }));
    controller.release();
}

#[tokio::test]
async fn default_flash_target_restores_original_off_state_after_identify_pulse() {
    let controller = FlashFallbackController::new(false);
    let target = HubDispatchTarget::Devices {
        native_ids: vec!["bulb-a".to_string()],
    };

    controller.flash_target(&target).await.unwrap();

    assert_eq!(controller.turn_on_calls.load(Ordering::SeqCst), 1);
    assert_eq!(controller.turn_off_calls.load(Ordering::SeqCst), 2);
}
