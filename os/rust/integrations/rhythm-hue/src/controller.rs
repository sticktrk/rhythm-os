//! Hue light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using the Hue V2 API
//! via any `HueTransport` implementation. Controls rooms through grouped_light
//! resources with color_temperature.mirek (no xy conversion needed).

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

use crate::registry::HueDeviceRegistry;
use crate::transport::HueTransport;

/// Light controller implementation using Hue V2 API.
///
/// Generic over `H: HueTransport` so different platforms can provide their
/// own HTTP/TLS implementation. All transport methods are blocking; the
/// async_trait wrapper just executes them synchronously.
pub struct HueLightController<H: HueTransport> {
    client: H,
    username: String,
    registry: Arc<Mutex<HueDeviceRegistry>>,
    capability_state: Option<SharedState>,
    capability_hub_key: Option<HubKey>,
}

impl<H: HueTransport> HueLightController<H> {
    /// Create a new Hue light controller.
    ///
    /// # Arguments
    ///
    /// * `client` - Transport implementation for Hue bridge communication
    /// * `username` - Hue application key (from push-link pairing)
    /// * `registry` - Shared device registry for room -> grouped_light lookup
    pub fn new(client: H, username: String, registry: Arc<Mutex<HueDeviceRegistry>>) -> Self {
        Self {
            client,
            username,
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

    /// Eagerly establish the TLS connection to the Hue bridge so the first
    /// button press doesn't pay the 1-3s handshake cost.
    pub fn warmup_tls(&self) -> anyhow::Result<()> {
        self.client.warmup_tls()
    }

    fn room_context(
        &self,
        room_id: &str,
        command: &LightingCommand,
    ) -> (
        String,
        rhythm_devices::LightCapabilities,
        rhythm_devices::AdaptedCommand,
    ) {
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
        (room_label, caps, adapted)
    }

    fn unsupported_device_dispatch(&self, target: &HubDispatchTarget) -> LightControlError {
        LightControlError::CommandFailed(format!(
            "Hue device-addressed dispatch is not implemented for target {}",
            target.label()
        ))
    }

    fn send_group_turn_on(
        &self,
        room_id: &str,
        grouped_light_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let (room_label, _caps, adapted) = self.room_context(room_id, &command);
        let dynamics = adapted.transition_ms.map(|ms| ms as u16);

        self.client
            .set_grouped_light(
                &self.username,
                grouped_light_id,
                true,
                adapted.brightness,
                adapted.kelvin,
                adapted.xy,
                dynamics.filter(|ms| *ms > 0),
            )
            .map_err(|e| {
                log::warn!(target: "cmd", "Hue turn_on failed: room={} err={}", room_label, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn on room {}: {}",
                    room_id, e
                ))
            })?;

        if let Some((x, y)) = adapted.xy {
            debug!(target: "cmd",
                "Hue turn_on: room={} bri={} transition_ms={:?} xy=({:.3},{:.3}) rgb=({},{},{})",
                room_label, adapted.brightness.unwrap_or(0),
                adapted.transition_ms,
                x, y,
                command.rgb.r, command.rgb.g, command.rgb.b,
            );
        } else if let Some(kelvin) = adapted.kelvin {
            debug!(target: "cmd",
                "Hue turn_on: room={} bri={} kelvin={} transition_ms={:?}",
                room_label, adapted.brightness.unwrap_or(0), kelvin, adapted.transition_ms
            );
        } else {
            debug!(target: "cmd",
                "Hue turn_on: room={} bri={} transition_ms={:?}",
                room_label, adapted.brightness.unwrap_or(0), adapted.transition_ms
            );
        }

        Ok(())
    }

    fn send_group_turn_off(
        &self,
        room_id: &str,
        grouped_light_id: &str,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let room_label = rhythm_os::controller_helpers::format_room_label(&self.registry, room_id);
        let fade_ms = transition_ms
            .map(|ms| u16::try_from(ms).unwrap_or(u16::MAX))
            .filter(|ms| *ms > 0);

        self.client
            .set_grouped_light(
                &self.username,
                grouped_light_id,
                false,
                None,
                None,
                None,
                fade_ms,
            )
            .map_err(|e| {
                log::warn!(target: "cmd", "Hue turn_off failed: room={} err={}", room_label, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn off room {}: {}",
                    room_id, e
                ))
            })?;

        debug!(
            target: "cmd",
            "Hue turn_off: room={} transition_ms={:?}",
            room_label,
            transition_ms
        );

        Ok(())
    }

    fn group_any_lights_on(
        &self,
        room_id: &str,
        grouped_light_id: &str,
    ) -> LightControlResult<bool> {
        self.client
            .is_grouped_light_on(&self.username, grouped_light_id)
            .map_err(|e| {
                LightControlError::CommandFailed(format!(
                    "Failed to check lights for room {}: {}",
                    room_id, e
                ))
            })
    }

    fn group_target_for_devices(&self, native_ids: &[String]) -> Option<(String, String)> {
        let registry = self.registry.lock().ok()?;
        let room_id = registry.find_room_for_exact_light_entities(native_ids)?;
        let grouped_light_id = registry.get_grouped_light_id(&room_id)?;
        Some((room_id, grouped_light_id))
    }

    fn device_any_lights_on(&self, native_ids: &[String]) -> LightControlResult<bool> {
        if let Some((room_id, grouped_light_id)) = self.group_target_for_devices(native_ids) {
            return self.group_any_lights_on(&room_id, &grouped_light_id);
        }

        debug!(
            target: "cmd",
            "Hue any_lights_on skipped for device target [{}]: no exact grouped_light match",
            native_ids.join(",")
        );
        Ok(false)
    }
}

#[async_trait]
impl<H: HueTransport + 'static> HubLightController for HueLightController<H> {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.send_group_turn_on(room_id, control_id, command),
            HubDispatchTarget::Devices { .. } => Err(self.unsupported_device_dispatch(target)),
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
            } => self.send_group_turn_off(room_id, control_id, transition_ms),
            HubDispatchTarget::Devices { .. } => Err(self.unsupported_device_dispatch(target)),
        }
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        self.client.test_connection(&self.username).unwrap_or(false)
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.group_any_lights_on(room_id, control_id),
            HubDispatchTarget::Devices { native_ids } => self.device_any_lights_on(native_ids),
        }
    }

    fn name(&self) -> &str {
        "HueV2"
    }
}

#[async_trait]
impl<H: HueTransport + 'static> LightController for HueLightController<H> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.send_group_turn_on(room_id, &grouped_light_id, command)
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.send_group_turn_off(room_id, &grouped_light_id, transition_ms)
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        self.client.test_connection(&self.username).unwrap_or(false)
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.group_any_lights_on(room_id, &grouped_light_id)
    }

    fn name(&self) -> &str {
        "HueV2"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use crate::test_support::{HueTransportCall, SpyHueTransport};

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

    fn make_spy_controller() -> (
        HueLightController<SpyHueTransport>,
        Arc<Mutex<HueDeviceRegistry>>,
    ) {
        let spy = SpyHueTransport::new();
        let registry = Arc::new(Mutex::new(HueDeviceRegistry::default()));
        registry
            .lock()
            .unwrap()
            .upsert_room("room1", "Living Room", "gl1", &[]);
        let controller = HueLightController::new(spy, "testuser".to_string(), registry.clone());
        (controller, registry)
    }

    #[test]
    fn turn_on_sends_correct_params() {
        let (controller, _) = make_spy_controller();
        let cmd = LightingCommand::with_transition(80, 4000, 500);
        block_on(controller.turn_on("room1", cmd)).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
                xy: _,
                fade_ms,
            } => {
                assert_eq!(grouped_light_id, "gl1");
                assert!(*on);
                assert_eq!(*brightness, Some(80));
                assert_eq!(*kelvin, Some(4000));
                assert_eq!(*fade_ms, Some(500));
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn turn_off_sends_off() {
        let (controller, _) = make_spy_controller();
        block_on(controller.turn_off("room1", None)).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
                xy: _,
                fade_ms,
            } => {
                assert_eq!(grouped_light_id, "gl1");
                assert!(!*on);
                assert_eq!(*brightness, None);
                assert_eq!(*kelvin, None);
                assert_eq!(*fade_ms, None);
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn turn_off_with_transition_sends_fade() {
        let (controller, _) = make_spy_controller();
        block_on(controller.turn_off("room1", Some(1_250))).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight { on, fade_ms, .. } => {
                assert!(!*on);
                assert_eq!(*fade_ms, Some(1_250));
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn unknown_room_returns_error() {
        let (controller, _) = make_spy_controller();
        let cmd = LightingCommand::new(80, 4000);
        let result = block_on(controller.turn_on("nonexistent", cmd));
        assert!(result.is_err());
        match result.unwrap_err() {
            LightControlError::RoomNotFound(_) => {}
            other => panic!("Expected RoomNotFound, got {:?}", other),
        }
    }

    #[test]
    fn get_rooms_matches_registry() {
        let (controller, registry) = make_spy_controller();
        registry
            .lock()
            .unwrap()
            .upsert_room("room2", "Bedroom", "gl2", &[]);

        let rooms = block_on(LightController::get_rooms(&controller)).unwrap();
        assert_eq!(rooms.len(), 2);

        let names: Vec<&str> = rooms.iter().map(|r| r.name.as_str()).collect();
        assert!(names.contains(&"Living Room"));
        assert!(names.contains(&"Bedroom"));
    }

    #[test]
    fn any_lights_on_true() {
        let (controller, _) = make_spy_controller();
        controller.client.set_is_on(true);
        let result = block_on(controller.any_lights_on("room1")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_false() {
        let (controller, _) = make_spy_controller();
        controller.client.set_is_on(false);
        let result = block_on(controller.any_lights_on("room1")).unwrap();
        assert!(!result);
    }

    #[test]
    fn any_lights_on_device_target_uses_matching_grouped_light() {
        let (controller, registry) = make_spy_controller();
        registry.lock().unwrap().upsert_room(
            "room_devices",
            "Device-backed room",
            "gl-devices",
            &["dev1".to_string(), "dev2".to_string()],
        );

        controller.client.set_is_on(true);
        let result = block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["dev2".to_string(), "dev1".to_string()],
            }),
        )
        .unwrap();

        assert!(result);
        let calls = controller.client.calls();
        assert!(calls.iter().any(|call| matches!(
            call,
            HueTransportCall::IsGroupedLightOn { grouped_light_id } if grouped_light_id == "gl-devices"
        )));
    }

    #[test]
    fn any_lights_on_device_target_without_match_returns_false_quietly() {
        let (controller, _) = make_spy_controller();

        let result = block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["orphan".to_string()],
            }),
        )
        .unwrap();

        assert!(!result);
        let calls = controller.client.calls();
        assert!(!calls
            .iter()
            .any(|call| matches!(call, HueTransportCall::IsGroupedLightOn { .. })));
    }

    #[test]
    fn no_dynamics_when_no_transition() {
        let (controller, _) = make_spy_controller();

        // LightingCommand::new sets transition_ms = None
        let cmd = LightingCommand::new(50, 3000);
        block_on(controller.turn_on("room1", cmd)).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight { fade_ms, .. } => {
                assert_eq!(*fade_ms, None);
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn turn_on_direct_color_sends_xy_not_kelvin() {
        let (controller, _) = make_spy_controller();
        let rgb = rhythm_core::color::Rgb::new(255, 40, 150);
        let xy = rhythm_core::color::XyColor { x: 0.45, y: 0.25 };
        let cmd = LightingCommand::from_color(5, rgb, xy, Some(500));
        block_on(controller.turn_on("room1", cmd)).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                kelvin,
                xy,
                brightness,
                ..
            } => {
                assert_eq!(*kelvin, None, "direct color should not send kelvin");
                assert!(xy.is_some(), "direct color should send xy");
                let (x, y) = xy.unwrap();
                assert!((x - 0.45).abs() < 0.001);
                assert!((y - 0.25).abs() < 0.001);
                assert_eq!(*brightness, Some(5));
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn name_returns_hue_v2() {
        let (controller, _) = make_spy_controller();
        assert_eq!(LightController::name(&controller), "HueV2");
    }
}
