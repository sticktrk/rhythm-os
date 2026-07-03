use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rhythm_core::composite_controller::{
    HubDispatchCompletion, HubDispatchPolicy, HubDispatchTimeoutScope,
};
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
            .wait_timeout_while(started, Duration::from_millis(500), |started| !*started)
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

fn enqueue_policy() -> HubDispatchPolicy {
    HubDispatchPolicy {
        queue_capacity: 1,
        completion: HubDispatchCompletion::Enqueue,
        requires_staggering: false,
        min_dispatch_spacing: Duration::ZERO,
        dispatch_timeout: Duration::from_millis(250),
        timeout_cooldown: Duration::from_millis(50),
        timeout_scope: HubDispatchTimeoutScope::Target,
    }
}

fn wait_policy() -> HubDispatchPolicy {
    HubDispatchPolicy {
        queue_capacity: 2,
        completion: HubDispatchCompletion::Wait,
        requires_staggering: false,
        min_dispatch_spacing: Duration::ZERO,
        dispatch_timeout: Duration::from_millis(40),
        timeout_cooldown: Duration::from_millis(80),
        timeout_scope: HubDispatchTimeoutScope::Target,
    }
}

#[tokio::test]
async fn enqueue_policy_reports_full_queue_while_slow_light_command_is_inflight() {
    let controller = Arc::new(SlowHubController::new());
    let composite = CompositeController::new();
    composite.register_controller_with_policy("matter@local", controller.clone(), enqueue_policy());
    route_room(&composite, "matter@local", "kitchen");

    composite
        .turn_on("kitchen", LightingCommand::new(80, 4000))
        .await
        .unwrap();
    controller.wait_started();

    composite
        .turn_on("kitchen", LightingCommand::new(70, 3500))
        .await
        .unwrap();
    let error = composite
        .turn_on("kitchen", LightingCommand::new(60, 3200))
        .await
        .unwrap_err();
    assert!(matches!(error, LightControlError::CommandFailed(_)));

    controller.release();
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && controller.turn_on_count() < 2 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    composite.turn_off("kitchen", Some(120)).await.unwrap();

    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && controller.turn_off_count() == 0 {
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    assert!(controller.turn_on_count() >= 2);
    assert_eq!(controller.turn_off_count(), 1);
}

#[tokio::test]
async fn wait_policy_timeout_cools_down_then_recovers_after_slow_light_command_finishes() {
    let controller = Arc::new(SlowHubController::new());
    let composite = CompositeController::new();
    composite.register_controller_with_policy("hue@bridge", controller.clone(), wait_policy());
    route_room(&composite, "hue@bridge", "kitchen");

    let first = composite
        .turn_on("kitchen", LightingCommand::new(80, 4000))
        .await;
    assert!(first.is_err());
    controller.wait_started();

    let retry = composite
        .turn_on("kitchen", LightingCommand::new(70, 3500))
        .await;
    assert!(retry.is_err());

    controller.release();
    tokio::time::sleep(Duration::from_millis(100)).await;

    composite
        .turn_on("kitchen", LightingCommand::new(60, 3200))
        .await
        .unwrap();

    assert_eq!(controller.turn_on_count(), 2);
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
