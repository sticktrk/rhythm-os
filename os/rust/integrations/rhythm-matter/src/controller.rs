//! Matter light controller using the typed Matter transport.

use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use log::{debug, warn};
use rhythm_core::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult, LightController,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;
use rhythm_devices::{ColorPreference, DeviceQuirk, LightCapabilities, LightType};

use crate::clusters;
use crate::hub_state::MatterHubData;
use crate::transport::MatterTransport;

/// Type alias for the registry (same as HA and Hue).
pub type MatterDeviceRegistry = rhythm_os::registry::HubDeviceRegistry;

const DISPATCH_INFO_MS: u128 = 250;
const DISPATCH_WARN_MS: u128 = 1000;

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

    /// Resolve the concrete Matter device IDs for a registry-backed room target.
    fn target_device_ids(&self, room_id: &str) -> LightControlResult<Vec<String>> {
        let registry =
            self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;

        let device_ids = registry.get_light_entities(room_id);
        if !device_ids.is_empty() || registry.get_grouped_light_id(room_id).is_some() {
            return Ok(device_ids);
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

    fn device_metadata(
        &self,
        device_id: &str,
        node_id: u64,
    ) -> (LightCapabilities, Vec<DeviceQuirk>) {
        let cached_caps = self
            .hub_data
            .device_caps
            .lock()
            .ok()
            .and_then(|device_caps| device_caps.get(device_id).cloned());
        let cached_quirks = self
            .hub_data
            .device_quirks
            .lock()
            .ok()
            .and_then(|device_quirks| device_quirks.get(device_id).cloned());

        if let (Some(caps), Some(quirks)) = (cached_caps, cached_quirks) {
            return (caps, quirks);
        }

        match self.transport.probe_light(node_id) {
            Ok(device) => {
                let caps = crate::commissioning::build_device_capabilities(&device);
                let quirks = crate::commissioning::build_device_quirks(&device);
                let probed_id =
                    crate::lifecycle::format_device_id(device.node_id, device.light_endpoint);

                self.cache_device_metadata(device_id, &probed_id, &caps, &quirks);
                if let Err(error) = crate::capture::persist_device_capture(
                    &self.hub_data,
                    &device,
                    "on_demand_probe",
                ) {
                    warn!(
                        target: "cmd",
                        "Matter: failed to persist on-demand probe capture for {}: {}",
                        probed_id,
                        error
                    );
                }

                debug!(
                    target: "cmd",
                    "Matter: probed device metadata on demand for {}",
                    device_id
                );
                (caps, quirks)
            }
            Err(error) => {
                warn!(
                    target: "cmd",
                    "Matter: failed to probe metadata for {} (node {}): {}; using extended-color fallback",
                    device_id,
                    node_id,
                    error
                );
                (
                    LightCapabilities::defaults_for(LightType::ExtendedColor),
                    Vec::new(),
                )
            }
        }
    }

    fn cache_device_metadata(
        &self,
        requested_id: &str,
        probed_id: &str,
        caps: &LightCapabilities,
        quirks: &[DeviceQuirk],
    ) {
        if let Ok(mut device_caps) = self.hub_data.device_caps.lock() {
            device_caps.insert(requested_id.to_string(), caps.clone());
            if requested_id != probed_id {
                device_caps.insert(probed_id.to_string(), caps.clone());
            }
        }

        if let Ok(mut device_quirks) = self.hub_data.device_quirks.lock() {
            device_quirks.insert(requested_id.to_string(), quirks.to_vec());
            if requested_id != probed_id {
                device_quirks.insert(probed_id.to_string(), quirks.to_vec());
            }
        }
    }

    fn color_preference(quirks: &[DeviceQuirk]) -> ColorPreference {
        if quirks
            .iter()
            .any(|quirk| matches!(quirk, DeviceQuirk::NeedsXyNotCt))
        {
            ColorPreference::PreferXy
        } else {
            ColorPreference::PreferColorTemperature
        }
    }

    fn needs_explicit_on(quirks: &[DeviceQuirk]) -> bool {
        quirks
            .iter()
            .any(|quirk| matches!(quirk, DeviceQuirk::NeedsExplicitOn))
    }

    fn maybe_throttle(throttle_ms: Option<u32>) {
        if let Some(throttle_ms) = throttle_ms.filter(|ms| *ms > 0) {
            std::thread::sleep(Duration::from_millis(throttle_ms as u64));
        }
    }

    fn turn_on_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let started = Instant::now();
        let mut successful_devices = 0usize;
        let mut partial_devices = 0usize;
        let mut failed_devices = 0usize;

        for device_id in device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                failed_devices += 1;
                continue;
            };

            let (caps, quirks) = self.device_metadata(device_id, node_id);
            let adapted = rhythm_os::controller_helpers::adapt_lighting_command(
                &caps,
                &command,
                Self::color_preference(&quirks),
            );
            let throttle_ms = quirks.iter().find_map(|quirk| match quirk {
                DeviceQuirk::CommandThrottleMs(ms) => Some(*ms),
                _ => None,
            });
            let needs_explicit_on = adapted.on && Self::needs_explicit_on(&quirks);

            let mut command_successes = 0usize;
            let mut command_failures = 0usize;
            let mut already_sent_on = false;

            if needs_explicit_on {
                if let Err(e) = self.transport.set_on_off(node_id, endpoint, true) {
                    warn!(
                        target: "cmd",
                        "Matter: explicit on command failed for node {}: {}",
                        node_id,
                        e
                    );
                    command_failures += 1;
                } else {
                    command_successes += 1;
                    already_sent_on = true;
                }
                Self::maybe_throttle(throttle_ms);
            }

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
                    command_failures += 1;
                } else {
                    command_successes += 1;
                }
                Self::maybe_throttle(throttle_ms);
            } else if adapted.on && !already_sent_on {
                if let Err(e) = self.transport.set_on_off(node_id, endpoint, true) {
                    warn!(
                        target: "cmd",
                        "Matter: on command failed for node {}: {}",
                        node_id,
                        e
                    );
                    command_failures += 1;
                } else {
                    command_successes += 1;
                }
                Self::maybe_throttle(throttle_ms);
            }

            if let Some((hue, saturation)) = adapted.hue_saturation {
                if let Err(e) = self.transport.set_hue_saturation(
                    node_id,
                    endpoint,
                    hue,
                    saturation,
                    adapted.transition_ms,
                ) {
                    warn!(
                        target: "cmd",
                        "Matter: hue/saturation command failed for node {}: {}",
                        node_id,
                        e
                    );
                    command_failures += 1;
                } else {
                    command_successes += 1;
                }
                Self::maybe_throttle(throttle_ms);
            } else if let Some((x, y)) = adapted.xy {
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
                    command_failures += 1;
                } else {
                    command_successes += 1;
                }
                Self::maybe_throttle(throttle_ms);
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
                    command_failures += 1;
                } else {
                    command_successes += 1;
                }
                Self::maybe_throttle(throttle_ms);
            }

            match (command_successes, command_failures) {
                (0, _) => failed_devices += 1,
                (_, 0) => successful_devices += 1,
                _ => {
                    partial_devices += 1;
                    warn!(
                        target: "cmd",
                        "Matter turn_on partial for device {}: successes={} failures={}",
                        device_id,
                        command_successes,
                        command_failures
                    );
                }
            }
        }

        if successful_devices == 0 && partial_devices == 0 && !device_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_on failed for target {} ({} target devices)",
                target_label, failed_devices,
            )));
        }

        if successful_devices == 0 && partial_devices > 0 {
            return Err(LightControlError::CommandFailed(format!(
                "Matter turn_on partially failed for target {} ({} partial, {} failed)",
                target_label, partial_devices, failed_devices,
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if partial_devices > 0 || failed_devices > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_on_partial",
                target = %target_label,
                latency_ms,
                ok = successful_devices,
                partial = partial_devices,
                failed = failed_devices,
                device_count = device_ids.len(),
                "Matter turn_on partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_on",
                target = %target_label,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                device_count = device_ids.len(),
                "Matter turn_on slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_turn_on",
                target = %target_label,
                latency_ms,
                brightness = command.brightness,
                kelvin = command.kelvin,
                direct_color = command.is_direct_color,
                device_count = device_ids.len(),
                "Matter turn_on"
            );
        } else {
            debug!(
                target: "cmd",
                "Matter turn_on: target={} bri={} kelvin={} devices={} latency_ms={}",
                target_label,
                command.brightness,
                command.kelvin,
                device_ids.len(),
                latency_ms
            );
        }

        Ok(())
    }

    fn turn_off_devices(
        &self,
        target_label: &str,
        device_ids: &[String],
    ) -> LightControlResult<()> {
        let started = Instant::now();
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

        let latency_ms = started.elapsed().as_millis();
        if failed_devices > 0 {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_off_partial",
                target = %target_label,
                latency_ms,
                ok = successful_devices,
                failed = failed_devices,
                device_count = device_ids.len(),
                "Matter turn_off partial"
            );
        } else if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "matter_turn_off",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter turn_off slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "matter_turn_off",
                target = %target_label,
                latency_ms,
                device_count = device_ids.len(),
                "Matter turn_off"
            );
        } else {
            debug!(
                target: "cmd",
                "Matter turn_off: target={} devices={} latency_ms={}",
                target_label,
                device_ids.len(),
                latency_ms
            );
        }

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
    use anyhow::Result;
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
            capture_dir: std::sync::OnceLock::new(),
            registry: registry.clone(),
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::new()),
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
    fn turn_on_sends_brightness_and_color_temperature_commands_by_default() {
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
            RecordedOperation::SetColorTemperature {
                node_id: 42,
                endpoint: 1,
                ..
            }
        ));
        assert!(matches!(
            operations[3],
            RecordedOperation::SetColorTemperature {
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
                | RecordedOperation::SetHueSaturation { node_id, .. }
                | RecordedOperation::SetXy { node_id, .. }
                | RecordedOperation::ReadOnOff { node_id, .. } => *node_id,
            })
            .collect();
        assert!(node_ids.contains(&42));
        assert!(node_ids.contains(&43));
    }

    #[test]
    fn turn_on_direct_device_target_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            LightingCommand::new(50, 3000),
        ))
        .unwrap();

        assert!(
            spy.operations().iter().any(|operation| matches!(
                operation,
                RecordedOperation::SetBrightness { node_id: 42, .. }
            )),
            "expected direct device control to address node 42",
        );
    }

    #[test]
    fn turn_on_direct_device_id_without_registry_room_errors() {
        let (controller, _, _) = make_controller();

        let error =
            block_on(controller.turn_on("matter-42", LightingCommand::new(50, 3000))).unwrap_err();
        assert!(matches!(error, LightControlError::RoomNotFound(_)));
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
    fn turn_off_direct_device_target_succeeds() {
        let (controller, spy, _) = make_controller();

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            None,
        ))
        .unwrap();

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
    fn any_lights_on_direct_device_target_reads_state() {
        let (controller, spy, _) = make_controller();
        spy.set_on_off_state(42, true);

        let result = block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            }),
        )
        .unwrap();
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
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::new()),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::new()),
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

    #[test]
    fn turn_on_probes_missing_caps_before_sending_color_command() {
        let (controller, spy, _) = make_controller();
        spy.set_probe_device(crate::transport::CommissionedDevice {
            node_id: 42,
            vendor_name: "Vendor".to_string(),
            product_name: "Lamp".to_string(),
            vendor_id: 1,
            product_id: 2,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![crate::transport::MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
        });

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["matter-42".to_string()],
            },
            LightingCommand::new(50, 3000),
        ))
        .unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
        assert!(!spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));

        let cached = controller.hub_data.device_caps.lock().unwrap();
        let caps = cached
            .get("matter-42")
            .expect("expected probed caps to be cached");
        assert!(caps.supports_color_temp());
        assert!(!caps.supports_xy_color());
    }

    #[test]
    fn turn_on_quirked_device_prefers_xy() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsXyNotCt],
            )])),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        block_on(controller.turn_on("r1", LightingCommand::new(50, 3000))).unwrap();

        assert!(spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
        assert!(!spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetColorTemperature { node_id: 42, .. }
        )));
    }

    #[test]
    fn turn_on_direct_color_prefers_hue_saturation_when_supported() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities {
                    color_modes: vec![
                        rhythm_devices::ColorMode::HueSaturation,
                        rhythm_devices::ColorMode::Xy,
                        rhythm_devices::ColorMode::ColorTemperature,
                    ],
                    ..LightCapabilities::defaults_for(LightType::ExtendedColor)
                },
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                Vec::new(),
            )])),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        let command = LightingCommand::from_color(
            50,
            rhythm_core::Rgb::new(255, 0, 0),
            rhythm_core::XyColor { x: 0.64, y: 0.33 },
            Some(400),
        );
        block_on(controller.turn_on("r1", command)).unwrap();

        assert!(spy.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetHueSaturation { node_id: 42, .. }
        )));
        assert!(!spy
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
    }

    #[test]
    fn turn_on_quirked_device_sends_explicit_on_before_brightness() {
        let spy = Arc::new(SpyTransport::new());
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::Dimmable),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsExplicitOn],
            )])),
            event_tx: tx,
        });
        let controller = MatterLightController::new(spy.clone(), hub_data);

        block_on(controller.turn_on("r1", LightingCommand::new(50, 3000))).unwrap();

        assert_eq!(
            spy.operations(),
            vec![
                RecordedOperation::SetOnOff {
                    node_id: 42,
                    endpoint: 1,
                    on: true,
                },
                RecordedOperation::SetBrightness {
                    node_id: 42,
                    endpoint: 1,
                    level: clusters::brightness_to_level(50),
                    transition_ms: None,
                },
            ]
        );
    }

    #[test]
    fn partial_device_failure_returns_command_failed() {
        struct PartialFailureTransport {
            operations: Mutex<Vec<RecordedOperation>>,
        }

        impl PartialFailureTransport {
            fn operations(&self) -> Vec<RecordedOperation> {
                self.operations.lock().unwrap().clone()
            }
        }

        impl MatterTransport for PartialFailureTransport {
            fn commission_light(
                &self,
                _request: &crate::transport::MatterCommissionRequest,
            ) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
                Ok(())
            }

            fn list_devices(&self) -> Result<Vec<crate::transport::MatterDeviceInfo>> {
                Ok(Vec::new())
            }

            fn probe_light(&self, _node_id: u64) -> Result<crate::transport::CommissionedDevice> {
                unreachable!()
            }

            fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetOnOff {
                        node_id,
                        endpoint,
                        on,
                    });
                Ok(())
            }

            fn set_brightness(
                &self,
                node_id: u64,
                endpoint: u16,
                level: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetBrightness {
                        node_id,
                        endpoint,
                        level,
                        transition_ms,
                    });
                Ok(())
            }

            fn set_color_temperature(
                &self,
                node_id: u64,
                endpoint: u16,
                kelvin: u16,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetColorTemperature {
                        node_id,
                        endpoint,
                        kelvin,
                        transition_ms,
                    });
                Ok(())
            }

            fn set_xy(
                &self,
                node_id: u64,
                endpoint: u16,
                x: f32,
                y: f32,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetXy {
                        node_id,
                        endpoint,
                        x,
                        y,
                        transition_ms,
                    });
                anyhow::bail!("xy failed");
            }

            fn set_hue_saturation(
                &self,
                node_id: u64,
                endpoint: u16,
                hue: u8,
                saturation: u8,
                transition_ms: Option<u32>,
            ) -> Result<()> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::SetHueSaturation {
                        node_id,
                        endpoint,
                        hue,
                        saturation,
                        transition_ms,
                    });
                anyhow::bail!("hue/saturation failed");
            }

            fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
                self.operations
                    .lock()
                    .unwrap()
                    .push(RecordedOperation::ReadOnOff { node_id, endpoint });
                Ok(false)
            }
        }

        let transport = Arc::new(PartialFailureTransport {
            operations: Mutex::new(Vec::new()),
        });
        let registry = Arc::new(Mutex::new(MatterDeviceRegistry::with_options(true)));
        registry
            .lock()
            .unwrap()
            .upsert_room("r1", "Room", "r1", &["matter-42".to_string()]);
        registry
            .lock()
            .unwrap()
            .set_area_lights("r1", vec!["matter-42".to_string()]);

        let (tx, _rx) = std::sync::mpsc::channel();
        let hub_data = Arc::new(crate::hub_state::MatterHubData {
            #[cfg(feature = "desktop")]
            transport: std::sync::OnceLock::new(),
            capture_dir: std::sync::OnceLock::new(),
            registry,
            fabric_id: "test".to_string(),
            commissioned: std::sync::Mutex::new(Vec::new()),
            next_node_id: std::sync::atomic::AtomicU64::new(100),
            device_caps: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                LightCapabilities::defaults_for(LightType::ExtendedColor),
            )])),
            device_quirks: std::sync::Mutex::new(std::collections::HashMap::from([(
                "matter-42".to_string(),
                vec![DeviceQuirk::NeedsXyNotCt],
            )])),
            event_tx: tx,
        });
        let controller = MatterLightController::new(transport.clone(), hub_data);

        let result = block_on(controller.turn_on("r1", LightingCommand::new(50, 3000)));
        assert!(matches!(result, Err(LightControlError::CommandFailed(_))));
        assert!(transport.operations().iter().any(|operation| matches!(
            operation,
            RecordedOperation::SetBrightness { node_id: 42, .. }
        )));
        assert!(transport
            .operations()
            .iter()
            .any(|operation| matches!(operation, RecordedOperation::SetXy { node_id: 42, .. })));
    }
}
