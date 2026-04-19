//! Home Assistant light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using HA service calls
//! via any `HaTransport` implementation. Controls rooms through area_id targeting.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::debug;
use rhythm_core::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::ColorPreference;
use rhythm_os::canonical::identity::HubKey;
use rhythm_os::state::SharedState;

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
    capability_state: Option<SharedState>,
    capability_hub_key: Option<HubKey>,
}

impl<H: HaTransport> HaLightController<H> {
    /// Create a new HA light controller.
    pub fn new(client: H, registry: Arc<Mutex<HaDeviceRegistry>>) -> Self {
        Self {
            client,
            registry,
            capability_state: None,
            capability_hub_key: None,
        }
    }

    /// Attach shared state so room capabilities can be derived from the
    /// canonical registry at command time.
    pub fn with_capability_source(mut self, state: SharedState, hub_key: HubKey) -> Self {
        self.capability_state = Some(state);
        self.capability_hub_key = Some(hub_key);
        self
    }

    fn adapt_group_command(
        &self,
        room_id: &str,
        command: &LightingCommand,
    ) -> (String, rhythm_devices::AdaptedCommand) {
        let room_label = rhythm_os::controller_helpers::format_room_label(&self.registry, room_id);
        let caps = rhythm_os::controller_helpers::resolve_room_capabilities(
            &self.registry,
            self.capability_state.as_ref(),
            self.capability_hub_key.as_ref(),
            room_id,
        );
        let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
            &caps,
            command,
            ColorPreference::PreferColorTemperature,
        );
        (room_label, adapted)
    }

    fn adapt_device_command(
        &self,
        native_ids: &[String],
        command: &LightingCommand,
    ) -> rhythm_devices::AdaptedCommand {
        let caps = rhythm_os::controller_helpers::resolve_device_capabilities(
            self.capability_state.as_ref(),
            self.capability_hub_key.as_ref(),
            native_ids,
        );
        rhythm_os::controller_helpers::adapt_lighting_command(
            &caps,
            command,
            ColorPreference::PreferColorTemperature,
        )
    }

    fn turn_on_area(
        &self,
        room_id: &str,
        area_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let (room_label, adapted) = self.adapt_group_command(room_id, &command);
        let fade_ms = adapted.transition_ms.unwrap_or(0) as u16;

        let mut data = serde_json::json!({
            "area_id": area_id,
            "brightness_pct": adapted.brightness.unwrap_or(0),
        });

        if let Some((x, y)) = adapted.xy {
            data["xy_color"] = serde_json::json!([x, y]);
        } else if let Some(kelvin) = adapted.kelvin {
            data["color_temp_kelvin"] = serde_json::json!(kelvin);
        }

        if fade_ms > 0 {
            let transition_secs = (fade_ms as f32 / 1000.0).max(0.1);
            data["transition"] = serde_json::json!(transition_secs);
        }

        self.client
            .call_service("light", "turn_on", &data)
            .map_err(|e| {
                log::warn!(target: "cmd", "HA turn_on failed: room={} err={}", room_label, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn on room {}: {}",
                    room_id, e
                ))
            })?;

        if let Some((x, y)) = adapted.xy {
            debug!(target: "cmd",
                "HA turn_on: room={} bri={} xy=({:.3},{:.3}) rgb=({},{},{})",
                room_label, adapted.brightness.unwrap_or(0),
                x, y,
                command.rgb.r, command.rgb.g, command.rgb.b,
            );
        } else if let Some(kelvin) = adapted.kelvin {
            debug!(target: "cmd",
                "HA turn_on: room={} bri={} kelvin={}",
                room_label, adapted.brightness.unwrap_or(0), kelvin
            );
        } else {
            debug!(target: "cmd", "HA turn_on: room={} bri={}", room_label, adapted.brightness.unwrap_or(0));
        }

        Ok(())
    }

    fn turn_on_devices(
        &self,
        native_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let adapted = self.adapt_device_command(native_ids, &command);
        let fade_ms = adapted.transition_ms.unwrap_or(0) as u16;

        let mut data = serde_json::json!({
            "entity_id": native_ids,
            "brightness_pct": adapted.brightness.unwrap_or(0),
        });

        if let Some((x, y)) = adapted.xy {
            data["xy_color"] = serde_json::json!([x, y]);
        } else if let Some(kelvin) = adapted.kelvin {
            data["color_temp_kelvin"] = serde_json::json!(kelvin);
        }

        if fade_ms > 0 {
            let transition_secs = (fade_ms as f32 / 1000.0).max(0.1);
            data["transition"] = serde_json::json!(transition_secs);
        }

        self.client
            .call_service("light", "turn_on", &data)
            .map_err(|e| {
                LightControlError::CommandFailed(format!(
                    "Failed to turn on devices [{}]: {}",
                    native_ids.join(","),
                    e
                ))
            })?;

        debug!(
            target: "cmd",
            "HA turn_on direct: devices={} bri={}",
            native_ids.join(","),
            adapted.brightness.unwrap_or(0)
        );
        Ok(())
    }

    fn turn_off_area(
        &self,
        room_id: &str,
        area_id: &str,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let room_label = rhythm_os::controller_helpers::format_room_label(&self.registry, room_id);
        let mut data = serde_json::json!({
            "area_id": area_id,
        });

        if let Some(transition_ms) = transition_ms.filter(|ms| *ms > 0) {
            let transition_secs = (transition_ms as f32 / 1000.0).max(0.1);
            data["transition"] = serde_json::json!(transition_secs);
        }

        self.client
            .call_service("light", "turn_off", &data)
            .map_err(|e| {
                log::warn!(target: "cmd", "HA turn_off failed: room={} err={}", room_label, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn off room {}: {}",
                    room_id, e
                ))
            })?;

        debug!(
            target: "cmd",
            "HA turn_off: room={} transition_ms={:?}",
            room_label,
            transition_ms
        );

        Ok(())
    }

    fn turn_off_devices(
        &self,
        native_ids: &[String],
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let mut data = serde_json::json!({
            "entity_id": native_ids,
        });

        if let Some(transition_ms) = transition_ms.filter(|ms| *ms > 0) {
            let transition_secs = (transition_ms as f32 / 1000.0).max(0.1);
            data["transition"] = serde_json::json!(transition_secs);
        }

        self.client
            .call_service("light", "turn_off", &data)
            .map_err(|e| {
                LightControlError::CommandFailed(format!(
                    "Failed to turn off devices [{}]: {}",
                    native_ids.join(","),
                    e
                ))
            })?;

        debug!(
            target: "cmd",
            "HA turn_off direct: devices={} transition_ms={:?}",
            native_ids.join(","),
            transition_ms
        );
        Ok(())
    }

    fn any_devices_on(&self, native_ids: &[String]) -> LightControlResult<bool> {
        for entity_id in native_ids {
            match self.client.get_state(entity_id) {
                Ok(state) if state.state == "on" => return Ok(true),
                Ok(_) => {}
                Err(e) => {
                    log::warn!(target: "cmd", "Failed to get state for {}: {}", entity_id, e);
                }
            }
        }
        Ok(false)
    }
}

#[async_trait]
impl<H: HaTransport + 'static> HubLightController for HaLightController<H> {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.turn_on_area(room_id, control_id, command),
            HubDispatchTarget::Devices { native_ids } => self.turn_on_devices(native_ids, command),
        }
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.turn_off_area(room_id, control_id, transition_ms),
            HubDispatchTarget::Devices { native_ids } => {
                self.turn_off_devices(native_ids, transition_ms)
            }
        }
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        self.client.test_connection().unwrap_or(false)
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        match target {
            HubDispatchTarget::Group { room_id, .. } => {
                let light_entities = {
                    let registry = self.registry.lock().map_err(|e| {
                        LightControlError::Internal(format!("Failed to lock registry: {}", e))
                    })?;
                    registry.get_light_entities(room_id)
                };

                if light_entities.is_empty() {
                    return Ok(false);
                }

                self.any_devices_on(&light_entities)
            }
            HubDispatchTarget::Devices { native_ids } => self.any_devices_on(native_ids),
        }
    }

    fn name(&self) -> &str {
        "HomeAssistant"
    }
}

#[async_trait]
impl<H: HaTransport + 'static> LightController for HaLightController<H> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let area_id = rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.turn_on_area(room_id, &area_id, command)
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        let area_id = rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.turn_off_area(room_id, &area_id, transition_ms)
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

        self.any_devices_on(&light_entities)
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
        let registry = Arc::new(Mutex::new(HaDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("living_room", "Living Room", "living_room", &[]);
        let controller = HaLightController::new(spy, registry.clone());
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
        let cmd = LightingCommand::with_transition(50, 3500, 1000);
        block_on(controller.turn_on("living_room", cmd)).unwrap();

        let calls = controller.client.calls();
        let transition = calls[0].data["transition"].as_f64().unwrap();
        // 1000ms -> 1.0 seconds
        assert!((transition - 1.0).abs() < 0.01);
    }

    #[test]
    fn turn_off_sends_area_id() {
        let (controller, _) = make_controller();
        block_on(controller.turn_off("living_room", None)).unwrap();

        let calls = controller.client.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].domain, "light");
        assert_eq!(calls[0].service, "turn_off");
        assert_eq!(calls[0].data["area_id"], "living_room");
    }

    #[test]
    fn turn_off_includes_transition_when_fade_set() {
        let (controller, _) = make_controller();
        block_on(controller.turn_off("living_room", Some(1_500))).unwrap();

        let calls = controller.client.calls();
        let transition = calls[0].data["transition"].as_f64().unwrap();
        assert!((transition - 1.5).abs() < 0.01);
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
    fn turn_on_direct_color_sends_xy_not_kelvin() {
        let (controller, _) = make_controller();
        let rgb = rhythm_core::color::Rgb::new(255, 40, 150);
        let xy = rhythm_core::color::XyColor { x: 0.45, y: 0.25 };
        let cmd = LightingCommand::from_color(5, rgb, xy, Some(500));
        block_on(controller.turn_on("living_room", cmd)).unwrap();

        let calls = controller.client.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0].data.get("xy_color").is_some(),
            "direct color should send xy_color"
        );
        assert!(
            calls[0].data.get("color_temp_kelvin").is_none(),
            "direct color should not send kelvin"
        );
        let xy_arr = calls[0].data["xy_color"].as_array().unwrap();
        assert!((xy_arr[0].as_f64().unwrap() - 0.45).abs() < 0.001);
        assert!((xy_arr[1].as_f64().unwrap() - 0.25).abs() < 0.001);
        assert_eq!(calls[0].data["brightness_pct"], 5);
    }

    #[test]
    fn name_returns_home_assistant() {
        let (controller, _) = make_controller();
        assert_eq!(LightController::name(&controller), "HomeAssistant");
    }
}
