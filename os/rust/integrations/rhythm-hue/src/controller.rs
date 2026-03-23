//! Hue light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using the Hue V2 API
//! via any `HueTransport` implementation. Controls rooms through grouped_light
//! resources with color_temperature.mirek (no xy conversion needed).

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::info;
use rhythm_core::controller::{LightControlError, LightControlResult, LightController};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;

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
    /// Shared atomic for Hue dynamics fade duration (ms). Updated via settings API.
    fade_ms: Arc<AtomicU16>,
}

impl<H: HueTransport> HueLightController<H> {
    /// Create a new Hue light controller.
    ///
    /// # Arguments
    ///
    /// * `client` - Transport implementation for Hue bridge communication
    /// * `username` - Hue application key (from push-link pairing)
    /// * `registry` - Shared device registry for room -> grouped_light lookup
    /// * `fade_ms` - Shared atomic for dynamics fade duration (ms)
    pub fn new(
        client: H,
        username: String,
        registry: Arc<Mutex<HueDeviceRegistry>>,
        fade_ms: Arc<AtomicU16>,
    ) -> Self {
        Self {
            client,
            username,
            registry,
            fade_ms,
        }
    }

    /// Eagerly establish the TLS connection to the Hue bridge so the first
    /// button press doesn't pay the 1-3s handshake cost.
    pub fn warmup_tls(&self) -> anyhow::Result<()> {
        self.client.warmup_tls()
    }
}

#[async_trait]
impl<H: HueTransport + 'static> LightController for HueLightController<H> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;

        let dynamics = self.fade_ms.load(Ordering::Relaxed);
        self.client
            .set_grouped_light(
                &self.username,
                &grouped_light_id,
                true,
                Some(command.brightness),
                Some(command.kelvin),
                if dynamics > 0 { Some(dynamics) } else { None },
            )
            .map_err(|e| {
                log::warn!(target: "cmd", "Hue turn_on failed: room={} err={}", room_id, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn on room {}: {}",
                    room_id, e
                ))
            })?;

        info!(target: "cmd",
            "Hue turn_on: room={} grouped_light={} bri={} kelvin={}",
            room_id, grouped_light_id, command.brightness, command.kelvin
        );

        Ok(())
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;

        self.client
            .set_grouped_light(&self.username, &grouped_light_id, false, None, None, None)
            .map_err(|e| {
                log::warn!(target: "cmd", "Hue turn_off failed: room={} err={}", room_id, e);
                LightControlError::CommandFailed(format!(
                    "Failed to turn off room {}: {}",
                    room_id, e
                ))
            })?;

        info!(target: "cmd",
            "Hue turn_off: room={} grouped_light={}",
            room_id, grouped_light_id
        );

        Ok(())
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

        self.client
            .is_grouped_light_on(&self.username, &grouped_light_id)
            .map_err(|e| {
                LightControlError::CommandFailed(format!(
                    "Failed to check lights for room {}: {}",
                    room_id, e
                ))
            })
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
        let fade_ms = Arc::new(AtomicU16::new(500));
        let controller =
            HueLightController::new(spy, "testuser".to_string(), registry.clone(), fade_ms);
        (controller, registry)
    }

    #[test]
    fn turn_on_sends_correct_params() {
        let (controller, _) = make_spy_controller();
        let cmd = LightingCommand::new(80, 4000);
        block_on(controller.turn_on("room1", cmd)).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
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
        block_on(controller.turn_off("room1")).unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                brightness,
                kelvin,
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

        let rooms = block_on(controller.get_rooms()).unwrap();
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
    fn no_dynamics_when_fade_zero() {
        let spy = SpyHueTransport::new();
        let registry = Arc::new(Mutex::new(HueDeviceRegistry::default()));
        registry
            .lock()
            .unwrap()
            .upsert_room("room1", "Living Room", "gl1", &[]);
        let fade_ms = Arc::new(AtomicU16::new(0));
        let controller = HueLightController::new(spy, "testuser".to_string(), registry, fade_ms);

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
    fn name_returns_hue_v2() {
        let (controller, _) = make_spy_controller();
        assert_eq!(controller.name(), "HueV2");
    }
}
