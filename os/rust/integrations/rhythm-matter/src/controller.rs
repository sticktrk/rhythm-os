//! Matter light controller using the typed Matter transport.

use std::sync::Arc;

use async_trait::async_trait;
use log::{debug, warn};
use rhythm_core::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::{ColorPreference, LightCapabilities, LightType};

use crate::clusters;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Type alias for the registry (same as HA and Hue).
pub type MatterDeviceRegistry = rhythm_os::registry::HubDeviceRegistry;

/// Light controller implementation using typed Matter light operations.
pub struct MatterLightController {
    transport: Arc<dyn MatterTransport>,
    hub_data: Arc<MatterHubData>,
}

impl MatterLightController {
    /// Create a new Matter light controller.
    pub fn new(transport: Arc<dyn MatterTransport>, hub_data: Arc<MatterHubData>) -> Self {
        Self {
            transport,
            hub_data,
        }
    }

    /// Parse a Matter device ID string into `(node_id, endpoint)`.
    ///
    /// Format: `matter-{node_id}` or `matter-{node_id}-{endpoint}`.
    fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
        let parts: Vec<&str> = device_id.strip_prefix("matter-")?.split('-').collect();
        let node_id = parts.first()?.parse::<u64>().ok()?;
        let endpoint = parts
            .get(1)
            .and_then(|endpoint| endpoint.parse::<u16>().ok())
            .unwrap_or(1);
        Some((node_id, endpoint))
    }

    /// Resolve the concrete Matter device IDs for a room or direct device target.
    ///
    /// Assigned devices are stored in the hub registry under a synthetic
    /// per-device room, while newly paired roomless devices may arrive here as a
    /// raw `matter-{node}` identifier through the composite controller's
    /// single-hub fallback routing. Accept both forms.
    fn target_device_ids(&self, room_id: &str) -> LightControlResult<Vec<String>> {
        let registry =
            self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;

        let device_ids = registry.get_light_entities(room_id);
        if !device_ids.is_empty() || registry.get_grouped_light_id(room_id).is_some() {
            return Ok(device_ids);
        }

        if Self::parse_device_id(room_id).is_some() {
            return Ok(vec![room_id.to_string()]);
        }

        Err(LightControlError::RoomNotFound(format!(
            "No target for room {}",
            room_id
        )))
    }

    fn target_device_ids_for_target(
        &self,
        target: &HubDispatchTarget,
    ) -> LightControlResult<Vec<String>> {
        match target {
            HubDispatchTarget::Group { room_id, .. } => self.target_device_ids(room_id),
            HubDispatchTarget::Devices { native_ids } => Ok(native_ids.clone()),
        }
    }

    fn turn_on_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let default_caps = LightCapabilities::defaults_for(LightType::ExtendedColor);
        let device_caps = self.hub_data.device_caps.lock().map_err(|e| {
            LightControlError::Internal(format!("Failed to lock device capabilities: {}", e))
        })?;

        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
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

            let mut any_success = false;

            if let Some(brightness) = adapted.brightness {
                let level = clusters::brightness_to_level(brightness);
                if let Err(e) =
                    self.transport
                        .set_brightness(node_id, endpoint, level, adapted.transition_ms)
                {
                    warn!(
                        target: "cmd",
                        "Matter: brightness command failed for node {}: {}",
                        node_id,
                        e
                    );
                } else {
                    any_success = true;
                }
            } else if adapted.on {
                if let Err(e) = self.transport.set_on_off(node_id, endpoint, true) {
                    warn!(
                        target: "cmd",
                        "Matter: on command failed for node {}: {}",
                        node_id,
                        e
                    );
                } else {
                    any_success = true;
                }
            }

            if let Some((x, y)) = adapted.xy {
                if let Err(e) =
                    self.transport
                        .set_xy(node_id, endpoint, x, y, adapted.transition_ms)
                {
                    warn!(
                        target: "cmd",
                        "Matter: xy command failed for node {}: {}",
                        node_id,
                        e
                    );
                } else {
                    any_success = true;
                }
            } else if let Some(kelvin) = adapted.kelvin {
                if let Err(e) = self.transport.set_color_temperature(
                    node_id,
                    endpoint,
                    kelvin,
                    adapted.transition_ms,
                ) {
                    warn!(
                        target: "cmd",
                        "Matter: color temperature command failed for node {}: {}",
                        node_id,
                        e
                    );
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
                "Matter turn_on failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        if failed_devices > 0 {
            warn!(
                target: "cmd",
                "Matter turn_on partial: target={} ok={} failed={}",
                target_label,
                successful_devices,
                failed_devices,
            );
        } else if command.is_direct_color || command.kelvin == 0 {
            debug!(
                target: "cmd",
                "Matter turn_on: target={} bri={} xy=({:.3},{:.3}) rgb=({},{},{}) devices={}",
                target_label,
                command.brightness,
                command.xy.x,
                command.xy.y,
                command.rgb.r,
                command.rgb.g,
                command.rgb.b,
                device_ids.len(),
            );
        } else {
            debug!(
                target: "cmd",
                "Matter turn_on: target={} bri={} kelvin={} devices={}",
                target_label,
                command.brightness,
                command.kelvin,
                device_ids.len()
            );
        }

        Ok(())
    }

    fn turn_off_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        let mut successful_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                failed_devices += 1;
                continue;
            };

            if let Err(e) = self.transport.set_on_off(node_id, endpoint, false) {
                warn!(target: "cmd", "Matter: off command failed for node {}: {}", node_id, e);
                failed_devices += 1;
            } else {
                successful_devices += 1;
            }
        }

        if successful_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_off failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        if failed_devices > 0 {
            warn!(
                target: "cmd",
                "Matter turn_off partial: target={} ok={} failed={}",
                target_label,
                successful_devices,
                failed_devices,
            );
        }

        debug!(target: "cmd", "Matter turn_off: target={} devices={}", target_label, device_ids.len());
        Ok(())
    }
}

#[async_trait]
impl HubLightController for MatterLightController {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        self.turn_on_devices(&target_label, &device_ids, command)
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        _transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let target_label = target.label();
        let device_ids = self.target_device_ids_for_target(target)?;
        self.turn_off_devices(&target_label, &device_ids)
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.hub_data.registry)
    }

    async fn is_connected(&self) -> bool {
        self.transport.list_devices().is_ok()
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        let device_ids = self.target_device_ids_for_target(target)?;

        if device_ids.is_empty() {
            return Ok(false);
        }

        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                continue;
            };
            match self.transport.read_on_off(node_id, endpoint) {
                Ok(true) => return Ok(true),
                Ok(false) => {}
                Err(e) => {
                    warn!(
                        target: "cmd",
                        "Matter: failed to read on/off state for node {}: {}",
                        node_id,
                        e
                    );
                }
            }
        }

        Ok(false)
    }

    fn name(&self) -> &str {
        "Matter"
    }
}

#[async_trait]
impl LightController for MatterLightController {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = self.target_device_ids(room_id)?;
        self.turn_on_devices(&room_label, &device_ids, command)
    }

    async fn turn_off(&self, room_id: &str, _transition_ms: Option<u32>) -> LightControlResult<()> {
        let room_label =
            rhythm_os::controller_helpers::format_room_label(&self.hub_data.registry, room_id);
        let device_ids = self.target_device_ids(room_id)?;
        self.turn_off_devices(&room_label, &device_ids)
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.hub_data.registry)
    }

    async fn is_connected(&self) -> bool {
        self.transport.list_devices().is_ok()
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let device_ids = self.target_device_ids(room_id)?;
        self.any_lights_on_target(&HubDispatchTarget::Devices {
            native_ids: device_ids,
        })
        .await
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

    use crate::test_support::{RecordedOperation, SpyTransport};

    fn block_on<F: Future>(mut future: F) -> F::Output {
        fn raw_waker() -> RawWaker {
            RawWaker::new(
                std::ptr::null(),
                &RawWakerVTable::new(|_| raw_waker(), |_| {}, |_| {}, |_| {}),
            )
        }

        // SAFETY: The future is not moved after pinning.
        let mut future = unsafe { Pin::new_unchecked(&mut future) };
        // SAFETY: The RawWaker above never dereferences the data pointer.
        let waker = unsafe { Waker::from_raw(raw_waker()) };
        let mut cx = Context::from_waker(&waker);
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => value,
            Poll::Pending => panic!("Mock future returned Pending"),
        }
    }

    fn make_controller() -> (
        MatterLightController,
        Arc<SpyTransport>,
        Arc<Mutex<MatterDeviceRegistry>>,
    ) {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));

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
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);
        (controller, spy, registry)
    }

    #[test]
    fn parse_device_id_basic() {
        assert_eq!(
            MatterLightController::parse_device_id("matter-42"),
            Some((42, 1))
        );
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(
            MatterLightController::parse_device_id("matter-42-2"),
            Some((42, 2))
        );
    }

    #[test]
    fn parse_device_id_invalid() {
        assert_eq!(MatterLightController::parse_device_id("hue-abc"), None);
    }

    #[test]
    fn turn_on_sends_brightness_and_xy_commands() {
        let (controller, spy, _) = make_controller();
        let command = LightingCommand::new(80, 4000);

        block_on(controller.turn_on("kitchen", command)).unwrap();

        let operations = spy.operations();
        assert_eq!(operations.len(), 4);

        assert_eq!(
            operations[0],
            RecordedOperation::SetBrightness {
                node_id: 42,
                endpoint: 1,
                level: clusters::brightness_to_level(80),
                transition_ms: None,
            }
        );
        assert!(matches!(
            operations[1],
            RecordedOperation::SetXy {
                node_id: 42,
                endpoint: 1,
                ..
            }
        ));
        assert!(matches!(
            operations[3],
            RecordedOperation::SetXy {
                node_id: 43,
                endpoint: 1,
                ..
            }
        ));
    }

    #[test]
    fn turn_on_fans_out_to_all_devices() {
        let (controller, spy, _) = make_controller();
        let command = LightingCommand::new(50, 3000);

        block_on(controller.turn_on("kitchen", command)).unwrap();

        let node_ids: Vec<u64> = spy
            .operations()
            .iter()
            .map(|operation| match operation {
                RecordedOperation::SetOnOff { node_id, .. }
                | RecordedOperation::SetBrightness { node_id, .. }
                | RecordedOperation::SetColorTemperature { node_id, .. }
                | RecordedOperation::SetXy { node_id, .. }
                | RecordedOperation::ReadOnOff { node_id, .. } => *node_id,
            })
            .collect();
        assert!(node_ids.contains(&42));
        assert!(node_ids.contains(&43));
    }

    #[test]
    fn turn_on_direct_device_id_without_registry_room_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_on("matter-42", LightingCommand::new(50, 3000))).unwrap();

        assert!(
            spy.operations().iter().any(|operation| matches!(
                operation,
                RecordedOperation::SetBrightness { node_id: 42, .. }
            )),
            "expected direct device control to address node 42",
        );
    }

    #[test]
    fn turn_off_sends_off_to_all_devices() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_off("kitchen", None)).unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: false,
                },
                RecordedOperation::SetOnOff {
                    node_id: 43,
                    endpoint: 1,
                    on: false,
                },
            ]
        );
    }

    #[test]
    fn turn_off_direct_device_id_without_registry_room_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_off("matter-42", None)).unwrap();

        assert_eq!(
            spy.operations(),
            vec![RecordedOperation::SetOnOff {
                node_id: 42,
                endpoint: 1,
                on: false,
            }]
        );
    }

    #[test]
    fn any_lights_on_returns_true_when_device_on() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off_state(42, true);

        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_returns_false_when_all_off() {
        let (controller, _, _) = make_controller();
        let result = block_on(controller.any_lights_on("kitchen")).unwrap();
        assert!(!result);
    }

    #[test]
    fn any_lights_on_direct_device_id_reads_state() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off_state(42, true);

        let result = block_on(controller.any_lights_on("matter-42")).unwrap();
        assert!(result);
    }

    #[test]
    fn any_lights_on_empty_returns_false() {
        let (controller, _, registry) = make_controller();
        registry
            .lock()
            .unwrap()
            .upsert_room("empty", "Empty Room", "empty", &[]);

        let result = block_on(controller.any_lights_on("empty")).unwrap();
        assert!(!result);
    }

    #[test]
    fn unknown_room_returns_error() {
        let (controller, _, _) = make_controller();
        let result = block_on(controller.turn_on("nonexistent", LightingCommand::new(50, 3000)));
        assert!(result.is_err());
    }

    #[test]
    fn name_returns_matter() {
        let (controller, _, _) = make_controller();
        assert_eq!(LightController::name(&controller), "Matter");
    }

    #[test]
    fn transition_time_is_forwarded() {
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
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            event_tx: tx,
        });

        let controller = MatterLightController::new(spy.clone(), hub_data);
        let command = LightingCommand::with_transition(50, 3000, 1200);

        block_on(controller.turn_on("r1", command)).unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness {
                transition_ms: Some(1200),
                ..
            }
        )));
    }

    #[test]
    fn failing_all_devices_returns_command_failed() {
        let (controller, spy, _) = make_controller();
        spy.fail_node(42);
        spy.fail_node(43);

        let result = block_on(controller.turn_on("kitchen", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
    }
}
