//! Home Assistant light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using HA service calls
//! via any `HaTransport` implementation. Controls rooms through area_id targeting.

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::info;
use rhythm_core::controller::{LightControlError, LightControlResult, LightController};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;

use crate::registry::HaDeviceRegistry;
use crate::transport::HaTransport;

/// Light controller implementation using Home Assistant service calls.
///
/// Generic over `H: HaTransport` so different platforms can provide their
/// own HTTP implementation. All transport methods are blocking; the
/// async_trait wrapper just executes them synchronously.
pub struct HaLightController<H: HaTransport> {
    client: H,
    registry: Arc<Mutex<HaDeviceRegistry>>,
    /// Shared atomic for fade duration (ms). Updated via settings API.
    fade_ms: Arc<AtomicU16>,
}

impl<H: HaTransport> HaLightController<H> {
    /// Create a new HA light controller.
    pub fn new(client: H, registry: Arc<Mutex<HaDeviceRegistry>>, fade_ms: Arc<AtomicU16>) -> Self {
        Self {
            client,
            registry,
            fade_ms,
        }
    }
}

#[async_trait]
impl<H: HaTransport + 'static> LightController for HaLightController<H> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        // Verify room exists in registry (for HA, target == room_id == area_id)
        let area_id = rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;

        let fade_ms = self.fade_ms.load(Ordering::Relaxed);

        // Build service data targeting area_id
        // Note: HA 2026.3 removed `color_temp` (mireds) — use `color_temp_kelvin` instead
        let mut data = serde_json::json!({
            "area_id": area_id,
            "brightness_pct": command.brightness,
            "color_temp_kelvin": command.kelvin,
        });

        // Add transition if fade is set (HA uses seconds)
        if fade_ms > 0 {
            let transition_secs = (fade_ms as f32 / 1000.0).max(0.1);
            data["transition"] = serde_json::json!(transition_secs);
        }

        self.client
            .call_service("light", "turn_on", &data)
            .map_err(|e| {
                log::warn!(target: "cmd", "HA turn_on failed: room={} err={}", room_id, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn on room {}: {}",
                    room_id, e
                ))
            })?;

        info!(target: "cmd",
            "HA turn_on: room={} bri={} kelvin={}",
            room_id, command.brightness, command.kelvin
        );

        Ok(())
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        let data = serde_json::json!({
            "area_id": room_id,
        });

        self.client
            .call_service("light", "turn_off", &data)
            .map_err(|e| {
                log::warn!(target: "cmd", "HA turn_off failed: room={} err={}", room_id, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn off room {}: {}",
                    room_id, e
                ))
            })?;

        info!(target: "cmd", "HA turn_off: room={}", room_id);

        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        self.client.test_connection().unwrap_or(false)
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        // Check light entities for this area
        let light_entities = {
            let registry = self.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        if light_entities.is_empty() {
            return Ok(false);
        }

        // Check each light entity state
        for entity_id in &light_entities {
            match self.client.get_state(entity_id) {
                Ok(state) => {
                    if state.state == "on" {
                        return Ok(true);
                    }
                }
                Err(e) => {
                    log::warn!(target: "cmd", "Failed to get state for {}: {}", entity_id, e);
                }
            }
        }

        Ok(false)
    }

    fn name(&self) -> &str {
        "HomeAssistant"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use crate::test_support::SpyHaTransport;
    use rhythm_core::room::RoomSource;

    // Minimal block_on for synchronous futures (no real I/O in mocks).
    fn block_on<F: Future>(mut f: F) -> F::Output {
        // SAFETY: We never move the future after pinning.
        let mut f = unsafe { Pin::new_unchecked(&mut f) };
        let raw = RawWaker::new(
            std::ptr::null(),
            &RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {}),
        );
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {}),
            )
        }
        let waker = unsafe { Waker::from_raw(raw) };
        let mut cx = Context::from_waker(&waker);
        match f.as_mut().poll(&mut cx) {
            Poll::Ready(val) => val,
            Poll::Pending => panic!("Mock future returned Pending — should be impossible"),
        }
    }

    fn make_controller() -> (
        HaLightController<SpyHaTransport>,
        Arc<Mutex<HaDeviceRegistry>>,
    ) {
        let spy = SpyHaTransport::new();
        let registry = Arc::new(Mutex::new(HaDeviceRegistry::with_options(
            RoomSource::HomeAssistant,
            true,
        )));
        registry
            .lock()
            .unwrap()
            .upsert_room("living_room", "Living Room", "living_room", &[]);
        let fade_ms = Arc::new(AtomicU16::new(1000));
        let controller = HaLightController::new(spy, registry.clone(), fade_ms);
        (controller, registry)
    }

    #[test]
    fn turn_on_sends_area_and_kelvin() {
        let (controller, _) = make_controller();
        let cmd = LightingCommand::new(80, 4000);
        block_on(controller.turn_on("living_room", cmd)).unwrap();

        let calls = controller.client.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].domain, "light");
        assert_eq!(calls[0].service, "turn_on");
        assert_eq!(calls[0].data["area_id"], "living_room");
        assert_eq!(calls[0].data["brightness_pct"], 80);
        assert_eq!(calls[0].data["color_temp_kelvin"], 4000);
    }

    #[test]
    fn kelvin_passed_directly() {
        let (controller, _) = make_controller();

        let cmd = LightingCommand::new(100, 2700);
        block_on(controller.turn_on("living_room", cmd)).unwrap();

        let calls = controller.client.calls();
        assert_eq!(calls[0].data["color_temp_kelvin"], 2700);
    }

    #[test]
    fn transition_included_when_fade_set() {
        let (controller, _) = make_controller();
        // fade_ms is 1000 from make_controller
        let cmd = LightingCommand::new(50, 3500);
        block_on(controller.turn_on("living_room", cmd)).unwrap();

        let calls = controller.client.calls();
        let transition = calls[0].data["transition"].as_f64().unwrap();
        // 1000ms -> 1.0 seconds
        assert!((transition - 1.0).abs() < 0.01);
    }

    #[test]
    fn turn_off_sends_area_id() {
        let (controller, _) = make_controller();
        block_on(controller.turn_off("living_room")).unwrap();

        let calls = controller.client.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].domain, "light");
        assert_eq!(calls[0].service, "turn_off");
        assert_eq!(calls[0].data["area_id"], "living_room");
    }

    #[test]
    fn any_lights_on_checks_entities() {
        let (controller, registry) = make_controller();
        // Register light entities for the room
        registry.lock().unwrap().set_area_lights(
            "living_room",
            vec![
                "light.living_room_1".to_string(),
                "light.living_room_2".to_string(),
            ],
        );
        // Set one light to "on"
        controller
            .client
            .set_entity_state("light.living_room_1", "on");

        let result = block_on(controller.any_lights_on("living_room")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_empty_entities() {
        let (controller, _) = make_controller();
        // No light entities registered — should return false
        let result = block_on(controller.any_lights_on("living_room")).unwrap();
        assert!(!result);
    }

    #[test]
    fn name_returns_home_assistant() {
        let (controller, _) = make_controller();
        assert_eq!(controller.name(), "HomeAssistant");
    }
}
