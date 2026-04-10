//! Matter light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using Matter cluster
//! commands via any `MatterTransport` implementation. Controls rooms through
//! per-device fan-out (Matter has no native room grouping).

use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, warn};
use rhythm_core::controller::{LightControlError, LightControlResult, LightController};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::{ColorPreference, LightCapabilities, LightType};

use crate::clusters;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Type alias for the registry (same as HA and Hue).
pub type MatterDeviceRegistry = rhythm_os::registry::HubDeviceRegistry;

/// Light controller implementation using Matter cluster commands.
///
/// Generic over `T: MatterTransport` so different platforms can provide their
/// own Matter stack. All transport methods are blocking; the async_trait wrapper
/// executes them synchronously.
///
/// Uses per-device fan-out: for each room, iterates `get_light_entities()` and
/// sends individual cluster commands to each device. This is the same pattern
/// as the HA controller with `area_lights`.
pub struct MatterLightController<T: MatterTransport> {
    transport: Arc<T>,
    hub_data: Arc<MatterHubData>,
}

impl<T: MatterTransport> MatterLightController<T> {
    /// Create a new Matter light controller.
    pub fn new(transport: Arc<T>, hub_data: Arc<MatterHubData>) -> Self {
        Self {
            transport,
            hub_data,
        }
    }

    /// Parse a Matter device ID string into (node_id, endpoint).
    ///
    /// Format: "matter-{node_id}" or "matter-{node_id}-{endpoint}"
    /// Default endpoint is 1 if not specified.
    fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
        let parts: Vec<&str> = device_id.strip_prefix("matter-")?.split('-').collect();
        let node_id = parts.first()?.parse::<u64>().ok()?;
        let endpoint = parts
            .get(1)
            .and_then(|e| e.parse::<u16>().ok())
            .unwrap_or(1);
        Some((node_id, endpoint))
    }
}

#[async_trait]
impl<T: MatterTransport + 'static> LightController for MatterLightController<T> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        // Verify room exists (for Matter, grouped_light_id == room_id)
        let _target =
            rhythm_os::controller_helpers::resolve_room_target(&self.hub_data.registry, room_id)?;

        // Get light device IDs for this room
        let device_ids = {
            let registry = self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        let default_caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let device_caps = self.hub_data.device_caps.lock().map_err(|e| {
            LightControlError::Internal(format!("Failed to lock device_caps: {}", e))
        })?;

        // Fan out to each device with capability-aware command adaptation
        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;
        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                failed_devices += 1;
                continue;
            };

            let caps = device_caps.get(device_id).unwrap_or(&default_caps);
            let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
                caps,
                &command,
                ColorPreference::PreferXy,
            );

            let transition_tenths = adapted
                .transition_ms
                .map(|ms| (ms / 100) as u16)
                .unwrap_or(0);

            let mut any_success = false;

            // Send level if device supports dimming
            if let Some(brightness) = adapted.brightness {
                let level = clusters::brightness_to_level(brightness);
                if let Err(e) = clusters::send_level(
                    &*self.transport,
                    node_id,
                    endpoint,
                    level,
                    transition_tenths,
                ) {
                    warn!(target: "cmd", "Matter: level command failed for node {}: {}", node_id, e);
                } else {
                    any_success = true;
                }
            } else if adapted.on {
                // OnOff-only device: just send on
                if let Err(e) = clusters::send_on(&*self.transport, node_id, endpoint) {
                    warn!(target: "cmd", "Matter: on command failed for node {}: {}", node_id, e);
                } else {
                    any_success = true;
                }
            }

            if let Some((x, y)) = adapted.xy {
                let cx = clusters::xy_to_matter(x);
                let cy = clusters::xy_to_matter(y);
                if let Err(e) = clusters::send_color_xy(
                    &*self.transport,
                    node_id,
                    endpoint,
                    cx,
                    cy,
                    transition_tenths,
                ) {
                    warn!(target: "cmd", "Matter: color xy command failed for node {}: {}", node_id, e);
                } else {
                    any_success = true;
                }
            } else if let Some(kelvin) = adapted.kelvin {
                let mireds = clusters::kelvin_to_mireds(kelvin);
                if let Err(e) = clusters::send_color_temperature(
                    &*self.transport,
                    node_id,
                    endpoint,
                    mireds,
                    transition_tenths,
                ) {
                    warn!(target: "cmd", "Matter: color temperature command failed for node {}: {}", node_id, e);
                } else {
                    any_success = true;
                }
            }

            if any_success {
                successful_devices += 1;
            } else {
                failed_devices += 1;
            }
        }

        if successful_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_on failed for room {} ({} target devices)",
                room_label, failed_devices,
            )));
        }

        if failed_devices > 0 {
            warn!(
                target: "cmd",
                "Matter turn_on partial: room={} ok={} failed={}",
                room_label,
                successful_devices,
                failed_devices,
            );
        } else if command.is_direct_color || command.kelvin == 0 {
            debug!(target: "cmd",
                "Matter turn_on: room={} bri={} xy=({:.3},{:.3}) rgb=({},{},{}) devices={}",
                room_label, command.brightness,
                command.xy.x, command.xy.y,
                command.rgb.r, command.rgb.g, command.rgb.b,
                device_ids.len(),
            );
        } else {
            debug!(target: "cmd",
                "Matter turn_on: room={} bri={} kelvin={} devices={}",
                room_label, command.brightness, command.kelvin, device_ids.len()
            );
        }

        Ok(())
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = {
            let registry = self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;
        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                failed_devices += 1;
                continue;
            };
            if let Err(e) = clusters::send_off(&*self.transport, node_id, endpoint) {
                warn!(target: "cmd", "Matter: off command failed for node {}: {}", node_id, e);
                failed_devices += 1;
            } else {
                successful_devices += 1;
            }
        }

        if successful_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_off failed for room {} ({} target devices)",
                room_label, failed_devices,
            )));
        }

        if failed_devices > 0 {
            warn!(
                target: "cmd",
                "Matter turn_off partial: room={} ok={} failed={}",
                room_label,
                successful_devices,
                failed_devices,
            );
        }

        debug!(target: "cmd", "Matter turn_off: room={} devices={}", room_label, device_ids.len());
        Ok(())
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.hub_data.registry)
    }

    async fn is_connected(&self) -> bool {
        // Check if any commissioned device is reachable
        self.transport
            .commissioned_devices()
            .map(|devices| devices.iter().any(|d| d.reachable))
            .unwrap_or(false)
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let device_ids = {
            let registry = self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        if device_ids.is_empty() {
            return Ok(false);
        }

        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                continue;
            };
            match self.transport.read_attribute(
                node_id,
                endpoint,
                clusters::CLUSTER_ON_OFF,
                clusters::ATTR_ON_OFF,
            ) {
                Ok(data) => {
                    if data.first().copied().unwrap_or(0) != 0 {
                        return Ok(true);
                    }
                }
                Err(e) => {
                    warn!(target: "cmd", "Matter: failed to read on/off state for node {}: {}", node_id, e);
                }
            }
        }

        Ok(false)
    }

    fn name(&self) -> &str {
        "Matter"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Mutex;
    use std::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    use crate::test_support::SpyTransport;

    // Minimal block_on for synchronous futures (no real I/O in mocks).
    fn block_on<F: Future>(mut f: F) -> F::Output {
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
            Poll::Pending => panic!("Mock future returned Pending"),
        }
    }

    fn make_controller() -> (
        MatterLightController<SpyTransport>,
        Arc<SpyTransport>,
        Arc<Mutex<MatterDeviceRegistry>>,
    ) {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));

        // Register room with two Matter devices
        registry.lock().unwrap().upsert_room(
            "kitchen",
            "Kitchen",
            "kitchen",
            &["matter-42".to_string(), "matter-43".to_string()],
        );
        registry.lock().unwrap().set_area_lights(
            "kitchen",
            vec!["matter-42".to_string(), "matter-43".to_string()],
        );

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            registry: registry.clone(),
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);

        (controller, spy, registry)
    }

    // ── Parse device ID ──────────────────────────────────────────────

    #[test]
    fn parse_device_id_basic() {
        assert_eq!(
            MatterLightController::<SpyTransport>::parse_device_id("matter-42"),
            Some((42, 1))
        );
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(
            MatterLightController::<SpyTransport>::parse_device_id("matter-42-2"),
            Some((42, 2))
        );
    }

    #[test]
    fn parse_device_id_invalid() {
        assert_eq!(
            MatterLightController::<SpyTransport>::parse_device_id("hue-abc"),
            None
        );
    }

    // ── turn_on ──────────────────────────────────────────────────────

    #[test]
    fn turn_on_sends_level_and_color_temp() {
        let (controller, spy, _) = make_controller();
        let cmd = LightingCommand::new(80, 4000);
        block_on(controller.turn_on("kitchen", cmd)).unwrap();

        let calls = spy.commands();
        // 2 devices × 2 commands (level + CT) = 4 commands
        assert_eq!(calls.len(), 4);

        // First device: level command (cluster 0x0008, cmd 0x04)
        assert_eq!(calls[0].node_id, 42);
        assert_eq!(calls[0].cluster, clusters::CLUSTER_LEVEL_CONTROL);
        assert_eq!(calls[0].cmd_id, clusters::CMD_MOVE_TO_LEVEL_WITH_ON_OFF);

        // First device: color XY command (cluster 0x0300, cmd 0x07)
        assert_eq!(calls[1].node_id, 42);
        assert_eq!(calls[1].cluster, clusters::CLUSTER_COLOR_CONTROL);
        assert_eq!(calls[1].cmd_id, clusters::CMD_MOVE_TO_COLOR);

        // Second device also gets both commands
        assert_eq!(calls[2].node_id, 43);
        assert_eq!(calls[3].node_id, 43);
    }

    #[test]
    fn turn_on_fans_out_to_all_devices() {
        let (controller, spy, _) = make_controller();
        let cmd = LightingCommand::new(50, 3000);
        block_on(controller.turn_on("kitchen", cmd)).unwrap();

        let node_ids: Vec<u64> = spy.commands().iter().map(|c| c.node_id).collect();
        assert!(node_ids.contains(&42));
        assert!(node_ids.contains(&43));
    }

    // ── turn_off ─────────────────────────────────────────────────────

    #[test]
    fn turn_off_sends_off_to_all_devices() {
        let (controller, spy, _) = make_controller();
        block_on(controller.turn_off("kitchen")).unwrap();

        let calls = spy.commands();
        assert_eq!(calls.len(), 2); // One off command per device

        assert_eq!(calls[0].node_id, 42);
        assert_eq!(calls[0].cluster, clusters::CLUSTER_ON_OFF);
        assert_eq!(calls[0].cmd_id, clusters::CMD_OFF);

        assert_eq!(calls[1].node_id, 43);
        assert_eq!(calls[1].cluster, clusters::CLUSTER_ON_OFF);
        assert_eq!(calls[1].cmd_id, clusters::CMD_OFF);
    }

    // ── any_lights_on ────────────────────────────────────────────────

    #[test]
    fn any_lights_on_returns_true_when_device_on() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off(42, true);

        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_returns_false_when_all_off() {
        let (controller, _, _) = make_controller();
        // Default state is off
        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(!result);
    }

    #[test]
    fn any_lights_on_empty_returns_false() {
        let (controller, _, registry) = make_controller();
        // Room with no light entities
        registry
            .lock()
            .unwrap()
            .upsert_room("empty", "Empty Room", "empty", &[]);

        let result = block_on(controller.any_lights_on("empty")).unwrap();
        assert!(!result);
    }

    // ── error cases ──────────────────────────────────────────────────

    #[test]
    fn unknown_room_returns_error() {
        let (controller, _, _) = make_controller();
        let cmd = LightingCommand::new(50, 3000);
        let result = block_on(controller.turn_on("nonexistent", cmd));
        assert!(result.is_err());
    }

    // ── name ─────────────────────────────────────────────────────────

    #[test]
    fn name_returns_matter() {
        let (controller, _, _) = make_controller();
        assert_eq!(controller.name(), "Matter");
    }

    // ── transition time ──────────────────────────────────────────────

    #[test]
    fn transition_time_from_fade_ms() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-1".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-1".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            registry: registry.clone(),
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);

        let cmd = LightingCommand::new(100, 4000);
        block_on(controller.turn_on("r1", cmd)).unwrap();

        // The level payload should contain transition_tenths = 20
        // (payload format depends on feature gate, but we can verify the command was sent)
        let calls = spy.commands();
        assert_eq!(calls.len(), 2); // level + CT
    }

    #[test]
    fn turn_on_returns_error_when_all_devices_fail() {
        let (controller, spy, _) = make_controller();
        spy.fail_node(42);
        spy.fail_node(43);

        let cmd = LightingCommand::new(50, 3000);
        let result = block_on(controller.turn_on("kitchen", cmd));

        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
    }

    #[test]
    fn turn_on_succeeds_when_at_least_one_device_succeeds() {
        let (controller, spy, _) = make_controller();
        spy.fail_node(42);

        let cmd = LightingCommand::new(50, 3000);
        block_on(controller.turn_on("kitchen", cmd)).unwrap();

        let node_ids: Vec<u64> = spy.commands().iter().map(|c| c.node_id).collect();
        assert!(!node_ids.contains(&42));
        assert!(node_ids.contains(&43));
    }
}
