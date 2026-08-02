//! Hue light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using the Hue V2 API
//! via any `HueTransport` implementation. Controls rooms through grouped_light
//! resources with color_temperature.mirek (no xy conversion needed).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

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
use crate::sse_liveness::HueSseLiveness;
use crate::transport::HueTransport;

const DISPATCH_INFO_MS: u128 = 250;
const DISPATCH_WARN_MS: u128 = 1000;

/// Light controller implementation using Hue V2 API.
///
/// Generic over `H: HueTransport` so different platforms can provide their
/// own HTTP/TLS implementation. All transport methods are blocking; the
/// async_trait wrapper just executes them synchronously.
pub struct HueLightController<H: HueTransport> {
    client: H,
    username: String,
    registry: Arc<Mutex<HueDeviceRegistry>>,
    light_resource_ids: Mutex<HashMap<String, String>>,
    capability_state: Option<SharedState>,
    capability_hub_key: Option<HubKey>,
    sse_liveness: Option<Arc<HueSseLiveness>>,
    external_topology_transaction_lock: Option<Arc<Mutex<()>>>,
    controller_operation_lock: Option<Arc<Mutex<()>>>,
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
            light_resource_ids: Mutex::new(HashMap::new()),
            capability_state: None,
            capability_hub_key: None,
            sse_liveness: None,
            external_topology_transaction_lock: None,
            controller_operation_lock: None,
        }
    }

    /// Attach shared state so room capabilities can be derived from the
    /// canonical registry at command time.
    pub fn with_capability_source(mut self, state: SharedState, hub_key: HubKey) -> Self {
        self.external_topology_transaction_lock = state
            .lock()
            .ok()
            .map(|state| state.external_topology_transaction_lock.clone());
        self.capability_state = Some(state);
        self.capability_hub_key = Some(hub_key);
        self
    }

    /// Attach the per-bridge SSE activity tracker used to verify that
    /// successful light writes are followed by event-stream traffic.
    pub fn with_sse_liveness(mut self, sse_liveness: Arc<HueSseLiveness>) -> Self {
        self.sse_liveness = Some(sse_liveness);
        self
    }

    /// Serialize ordinary light writes with Hue room/scene ownership changes.
    pub fn with_controller_operation_lock(mut self, lock: Arc<Mutex<()>>) -> Self {
        self.controller_operation_lock = Some(lock);
        self
    }

    fn lock_controller_operations(
        &self,
    ) -> LightControlResult<Option<std::sync::MutexGuard<'_, ()>>> {
        self.controller_operation_lock
            .as_ref()
            .map(|lock| {
                lock.lock().map_err(|_| {
                    LightControlError::CommandFailed(
                        "Hue controller operations are temporarily unavailable".to_string(),
                    )
                })
            })
            .transpose()
    }

    fn lock_topology_transaction(
        &self,
    ) -> LightControlResult<Option<std::sync::MutexGuard<'_, ()>>> {
        self.external_topology_transaction_lock
            .as_ref()
            .map(|lock| {
                lock.lock().map_err(|_| {
                    LightControlError::CommandFailed(
                        "Hue topology is temporarily unavailable".to_string(),
                    )
                })
            })
            .transpose()
    }

    fn ensure_authority_ready(&self) -> LightControlResult<()> {
        let Some(state) = self.capability_state.as_ref() else {
            return Ok(());
        };
        let Some(key) = self.capability_hub_key.as_ref() else {
            return Ok(());
        };
        let ready = state
            .lock()
            .map_err(|_| {
                LightControlError::CommandFailed(
                    "Hue authority state is temporarily unavailable".to_string(),
                )
            })?
            .external_controller_authority_is_ready(key);
        if !ready {
            return Err(LightControlError::CommandFailed(
                "Hue controller authority is not ready".to_string(),
            ));
        }
        Ok(())
    }

    fn lock_authoritative_operation(
        &self,
    ) -> LightControlResult<(
        Option<std::sync::MutexGuard<'_, ()>>,
        Option<std::sync::MutexGuard<'_, ()>>,
    )> {
        // The topology transaction is the authority hand-off barrier. Recheck
        // readiness only after crossing it so a command queued behind release
        // cannot write to a bridge Rhythm has just restored to the user.
        let topology = self.lock_topology_transaction()?;
        self.ensure_authority_ready()?;
        let operation = self.lock_controller_operations()?;
        Ok((topology, operation))
    }

    fn tracked_light_write(
        &self,
        write: impl FnOnce() -> anyhow::Result<()>,
    ) -> anyhow::Result<()> {
        let token = self
            .sse_liveness
            .as_ref()
            .and_then(|liveness| liveness.begin_expected_activity());
        let result = write();
        if result.is_err() {
            if let Some(liveness) = self.sse_liveness.as_ref() {
                liveness.cancel_expected_activity(token);
            }
        }
        result
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

    fn device_context(
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

    fn send_group_turn_on(
        &self,
        room_id: &str,
        grouped_light_id: &str,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let (room_label, _caps, adapted) = self.room_context(room_id, &command);
        let dynamics = adapted.transition_ms.map(|ms| ms as u16);
        let started = Instant::now();

        if let Err(e) = self.tracked_light_write(|| {
            self.client.set_grouped_light(
                &self.username,
                grouped_light_id,
                true,
                adapted.brightness,
                adapted.kelvin,
                adapted.xy,
                dynamics.filter(|ms| *ms > 0),
            )
        }) {
            tracing::warn!(
                target: "cmd",
                event = "hue_turn_on_failed",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms = started.elapsed().as_millis(),
                error = %e,
                "Hue turn_on failed"
            );
            return Err(LightControlError::CommandFailed(format!(
                "Failed to turn on room {}: {}",
                room_id, e
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "hue_turn_on",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms,
                brightness = adapted.brightness.unwrap_or(0),
                kelvin = ?adapted.kelvin,
                xy = ?adapted.xy,
                transition_ms = ?adapted.transition_ms,
                direct_color = command.is_direct_color,
                "Hue turn_on slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "hue_turn_on",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms,
                brightness = adapted.brightness.unwrap_or(0),
                kelvin = ?adapted.kelvin,
                xy = ?adapted.xy,
                transition_ms = ?adapted.transition_ms,
                direct_color = command.is_direct_color,
                "Hue turn_on"
            );
        } else {
            debug!(
                target: "cmd",
                "Hue turn_on: room={} bri={} kelvin={:?} xy={:?} transition_ms={:?} latency_ms={}",
                room_label,
                adapted.brightness.unwrap_or(0),
                adapted.kelvin,
                adapted.xy,
                adapted.transition_ms,
                latency_ms
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
        let started = Instant::now();

        if let Err(e) = self.tracked_light_write(|| {
            self.client.set_grouped_light(
                &self.username,
                grouped_light_id,
                false,
                None,
                None,
                None,
                fade_ms,
            )
        }) {
            tracing::warn!(
                target: "cmd",
                event = "hue_turn_off_failed",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms = started.elapsed().as_millis(),
                error = %e,
                "Hue turn_off failed"
            );
            return Err(LightControlError::CommandFailed(format!(
                "Failed to turn off room {}: {}",
                room_id, e
            )));
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "hue_turn_off",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms,
                transition_ms = ?transition_ms,
                "Hue turn_off slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "hue_turn_off",
                room_id = %room_id,
                room = %room_label,
                control_id = %grouped_light_id,
                latency_ms,
                transition_ms = ?transition_ms,
                "Hue turn_off"
            );
        } else {
            debug!(
                target: "cmd",
                "Hue turn_off: room={} transition_ms={:?} latency_ms={}",
                room_label,
                transition_ms,
                latency_ms
            );
        }

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
        // Core keeps a one-device synthetic registry room for generic
        // standalone-light addressing. Its room and control IDs are both the
        // Hue device resource ID, not a Hue grouped_light resource. Treat that
        // exact scaffold as direct control so we resolve and write the bulb's
        // light service instead of sending an invalid grouped-light request.
        if native_ids.len() == 1 && room_id == native_ids[0] && grouped_light_id == native_ids[0] {
            return None;
        }
        Some((room_id, grouped_light_id))
    }

    /// Canonical Hue endpoints are device resource IDs because Hue rooms own
    /// devices. Direct control, however, targets the device's `light` service
    /// resource. Resolve that service lazily so grouped-room matching can keep
    /// using device IDs while standalone bulbs use the correct V2 endpoint.
    fn light_resource_id(&self, device_id: &str) -> String {
        if let Some(light_id) = self
            .light_resource_ids
            .lock()
            .ok()
            .and_then(|cache| cache.get(device_id).cloned())
        {
            return light_id;
        }

        let resource_path = format!("device/{device_id}");
        let light_id = self
            .client
            .get_resources(&self.username, &resource_path)
            .ok()
            .and_then(|response| {
                response
                    .pointer("/data/0/services")
                    .and_then(|services| services.as_array())
                    .and_then(|services| {
                        services.iter().find_map(|service| {
                            (service.get("rtype").and_then(|value| value.as_str()) == Some("light"))
                                .then(|| service.get("rid").and_then(|value| value.as_str()))
                                .flatten()
                        })
                    })
                    .map(str::to_string)
            });

        let Some(light_id) = light_id else {
            // Backward/forward compatibility: some callers may already supply
            // a light resource ID, in which case device/{id} has no result.
            return device_id.to_string();
        };

        if let Ok(mut cache) = self.light_resource_ids.lock() {
            cache.insert(device_id.to_string(), light_id.clone());
        }
        light_id
    }

    fn device_any_lights_on(&self, native_ids: &[String]) -> LightControlResult<bool> {
        if let Some((room_id, grouped_light_id)) = self.group_target_for_devices(native_ids) {
            return self.group_any_lights_on(&room_id, &grouped_light_id);
        }

        for native_id in native_ids {
            let light_id = self.light_resource_id(native_id);
            let is_on = self
                .client
                .is_light_on(&self.username, &light_id)
                .map_err(|e| {
                    LightControlError::CommandFailed(format!(
                        "Failed to check Hue light {} for device {}: {}",
                        light_id, native_id, e
                    ))
                })?;
            if is_on {
                return Ok(true);
            }
        }
        Ok(false)
    }

    fn send_direct_devices_turn_on(
        &self,
        native_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        if native_ids.is_empty() {
            return Err(LightControlError::CommandFailed(
                "Hue turn_on target has no light IDs".to_string(),
            ));
        }

        let adapted = self.device_context(native_ids, &command);
        let dynamics = adapted.transition_ms.map(|ms| ms as u16);
        let label = native_ids.join(",");
        let started = Instant::now();

        for native_id in native_ids {
            let light_id = self.light_resource_id(native_id);
            if let Err(e) = self.tracked_light_write(|| {
                self.client.set_light(
                    &self.username,
                    &light_id,
                    true,
                    adapted.brightness,
                    adapted.kelvin,
                    adapted.xy,
                    dynamics.filter(|ms| *ms > 0),
                )
            }) {
                tracing::warn!(
                    target: "cmd",
                    event = "hue_light_turn_on_failed",
                    light_id = %light_id,
                    device_id = %native_id,
                    target = %label,
                    latency_ms = started.elapsed().as_millis(),
                    error = %e,
                    "Hue light turn_on failed"
                );
                return Err(LightControlError::CommandFailed(format!(
                    "Failed to turn on Hue light {} for device {}: {}",
                    light_id, native_id, e
                )));
            }
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "hue_light_turn_on",
                target = %label,
                latency_ms,
                brightness = adapted.brightness.unwrap_or(0),
                kelvin = ?adapted.kelvin,
                xy = ?adapted.xy,
                transition_ms = ?adapted.transition_ms,
                direct_color = command.is_direct_color,
                "Hue light turn_on slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "hue_light_turn_on",
                target = %label,
                latency_ms,
                brightness = adapted.brightness.unwrap_or(0),
                kelvin = ?adapted.kelvin,
                xy = ?adapted.xy,
                transition_ms = ?adapted.transition_ms,
                direct_color = command.is_direct_color,
                "Hue light turn_on"
            );
        } else {
            debug!(
                target: "cmd",
                "Hue light turn_on: target={} bri={} kelvin={:?} xy={:?} transition_ms={:?} latency_ms={}",
                label,
                adapted.brightness.unwrap_or(0),
                adapted.kelvin,
                adapted.xy,
                adapted.transition_ms,
                latency_ms
            );
        }

        Ok(())
    }

    fn send_direct_devices_turn_off(
        &self,
        native_ids: &[String],
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        if native_ids.is_empty() {
            return Err(LightControlError::CommandFailed(
                "Hue turn_off target has no light IDs".to_string(),
            ));
        }

        let fade_ms = transition_ms
            .map(|ms| u16::try_from(ms).unwrap_or(u16::MAX))
            .filter(|ms| *ms > 0);
        let label = native_ids.join(",");
        let started = Instant::now();

        for native_id in native_ids {
            let light_id = self.light_resource_id(native_id);
            if let Err(e) = self.tracked_light_write(|| {
                self.client
                    .set_light(&self.username, &light_id, false, None, None, None, fade_ms)
            }) {
                tracing::warn!(
                    target: "cmd",
                    event = "hue_light_turn_off_failed",
                    light_id = %light_id,
                    device_id = %native_id,
                    target = %label,
                    latency_ms = started.elapsed().as_millis(),
                    error = %e,
                    "Hue light turn_off failed"
                );
                return Err(LightControlError::CommandFailed(format!(
                    "Failed to turn off Hue light {} for device {}: {}",
                    light_id, native_id, e
                )));
            }
        }

        let latency_ms = started.elapsed().as_millis();
        if latency_ms >= DISPATCH_WARN_MS {
            tracing::warn!(
                target: "cmd",
                event = "hue_light_turn_off",
                target = %label,
                latency_ms,
                transition_ms = ?transition_ms,
                "Hue light turn_off slow"
            );
        } else if latency_ms >= DISPATCH_INFO_MS {
            tracing::info!(
                target: "cmd",
                event = "hue_light_turn_off",
                target = %label,
                latency_ms,
                transition_ms = ?transition_ms,
                "Hue light turn_off"
            );
        } else {
            debug!(
                target: "cmd",
                "Hue light turn_off: target={} transition_ms={:?} latency_ms={}",
                label,
                transition_ms,
                latency_ms
            );
        }

        Ok(())
    }

    fn send_devices_turn_on(
        &self,
        native_ids: &[String],
        command: LightingCommand,
    ) -> LightControlResult<()> {
        if let Some((room_id, grouped_light_id)) = self.group_target_for_devices(native_ids) {
            return self.send_group_turn_on(&room_id, &grouped_light_id, command);
        }

        self.send_direct_devices_turn_on(native_ids, command)
    }

    fn send_devices_turn_off(
        &self,
        native_ids: &[String],
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        if let Some((room_id, grouped_light_id)) = self.group_target_for_devices(native_ids) {
            return self.send_group_turn_off(&room_id, &grouped_light_id, transition_ms);
        }

        self.send_direct_devices_turn_off(native_ids, transition_ms)
    }
}

#[async_trait]
impl<H: HueTransport + 'static> HubLightController for HueLightController<H> {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.send_group_turn_on(room_id, control_id, command),
            HubDispatchTarget::Devices { native_ids } => {
                self.send_devices_turn_on(native_ids, command)
            }
        }
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.send_group_turn_off(room_id, control_id, transition_ms),
            HubDispatchTarget::Devices { native_ids } => {
                self.send_devices_turn_off(native_ids, transition_ms)
            }
        }
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        if self.ensure_authority_ready().is_err() {
            return false;
        }
        self.client.test_connection(&self.username).unwrap_or(false)
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        match target {
            HubDispatchTarget::Group {
                room_id,
                control_id,
            } => self.group_any_lights_on(room_id, control_id),
            HubDispatchTarget::Devices { native_ids } => self.device_any_lights_on(native_ids),
        }
    }

    async fn flash_target(&self, target: &HubDispatchTarget) -> LightControlResult<()> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        // Use the Hue V2 native identify action on each light
        // resource. This is the canonical mechanism Hue exposes for physical
        // identification — unlike the default on/off flash, it works for any
        // single light without requiring a grouped_light wrapping the exact
        // device set, and it does not perturb persisted on/brightness state.
        let native_ids = match target {
            HubDispatchTarget::Devices { native_ids } => native_ids.clone(),
            HubDispatchTarget::Group { room_id, .. } => {
                let registry = self.registry.lock().map_err(|_| {
                    LightControlError::CommandFailed(
                        "Hue registry lock poisoned during identify".to_string(),
                    )
                })?;
                registry.get_light_entities(room_id)
            }
        };

        if native_ids.is_empty() {
            return Err(LightControlError::CommandFailed(format!(
                "Hue identify: no light resources resolved for target {}",
                target.label()
            )));
        }

        for native_id in &native_ids {
            let light_id = self.light_resource_id(native_id);
            self.client
                .identify_light(&self.username, &light_id)
                .map_err(|e| {
                    LightControlError::CommandFailed(format!(
                        "Hue identify_light failed for {} (device {}): {}",
                        light_id, native_id, e
                    ))
                })?;
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "HueV2"
    }
}

#[async_trait]
impl<H: HueTransport + 'static> LightController for HueLightController<H> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.send_group_turn_on(room_id, &grouped_light_id, command)
    }

    async fn turn_off(&self, room_id: &str, transition_ms: Option<u32>) -> LightControlResult<()> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
        let grouped_light_id =
            rhythm_os::controller_helpers::resolve_room_target(&self.registry, room_id)?;
        self.send_group_turn_off(room_id, &grouped_light_id, transition_ms)
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        if self.ensure_authority_ready().is_err() {
            return false;
        }
        self.client.test_connection(&self.username).unwrap_or(false)
    }

    async fn any_lights_on(&self, room_id: &str) -> LightControlResult<bool> {
        let (_topology, _operation) = self.lock_authoritative_operation()?;
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

    use rhythm_os::hub::HubType;
    use rhythm_os::state::AppState;

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
    fn topology_barrier_precedes_authority_recheck() {
        let state = Arc::new(Mutex::new(AppState::default()));
        let key = HubKey::new(HubType::new(HubType::HUE), "192.0.2.1");
        state
            .lock()
            .unwrap()
            .mark_external_controller_authority_pending(&key);

        let (controller, _) = make_spy_controller();
        let controller = controller.with_capability_source(state.clone(), key);
        let topology_lock = state
            .lock()
            .unwrap()
            .external_topology_transaction_lock
            .clone();
        let poison = std::thread::spawn(move || {
            let _guard = topology_lock.lock().unwrap();
            panic!("poison topology barrier for lock-order assertion");
        });
        assert!(poison.join().is_err());

        let error = block_on(controller.turn_on_target(
            &HubDispatchTarget::Group {
                room_id: "room1".to_string(),
                control_id: "gl1".to_string(),
            },
            LightingCommand::new(80, 4000),
        ))
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("Hue topology is temporarily unavailable"));
        assert_eq!(controller.client.set_grouped_light_count(), 0);
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
    fn successful_light_write_expects_sse_activity() {
        let (controller, _) = make_spy_controller();
        let liveness = Arc::new(HueSseLiveness::default());
        let controller = controller.with_sse_liveness(liveness.clone());

        block_on(controller.turn_on("room1", LightingCommand::new(80, 4000))).unwrap();

        assert_eq!(liveness.pending_count(), 1);
        liveness.observe_sse_activity();
        assert_eq!(liveness.pending_count(), 0);
    }

    #[test]
    fn failed_light_write_cancels_sse_expectation() {
        let (controller, _) = make_spy_controller();
        let liveness = Arc::new(HueSseLiveness::default());
        controller.client.set_should_fail(true);
        let controller = controller.with_sse_liveness(liveness.clone());

        assert!(block_on(controller.turn_on("room1", LightingCommand::new(80, 4000))).is_err());

        assert_eq!(liveness.pending_count(), 0);
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
    fn any_lights_on_device_target_without_match_checks_direct_lights() {
        let (controller, _) = make_spy_controller();
        controller.client.set_is_on(true);

        let result = block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices {
                native_ids: vec!["orphan".to_string()],
            }),
        )
        .unwrap();

        assert!(result);
        let calls = controller.client.calls();
        assert!(!calls
            .iter()
            .any(|call| matches!(call, HueTransportCall::IsGroupedLightOn { .. })));
        assert!(calls.iter().any(|call| matches!(
            call,
            HueTransportCall::IsLightOn { light_id } if light_id == "orphan"
        )));
    }

    #[test]
    fn turn_on_device_target_uses_matching_grouped_light() {
        let (controller, registry) = make_spy_controller();
        registry.lock().unwrap().upsert_room(
            "room_devices",
            "Device-backed room",
            "gl-devices",
            &["dev1".to_string()],
        );

        let cmd = LightingCommand::with_transition(80, 4000, 500);
        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["dev1".to_string()],
            },
            cmd,
        ))
        .unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                ..
            } => {
                assert_eq!(grouped_light_id, "gl-devices");
                assert!(*on);
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn turn_on_device_target_without_match_uses_direct_light() {
        let (controller, _) = make_spy_controller();

        let cmd = LightingCommand::with_transition(80, 4000, 500);
        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["orphan".to_string()],
            },
            cmd,
        ))
        .unwrap();

        assert_eq!(controller.client.set_grouped_light_count(), 0);
        let calls = controller.client.set_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetLight {
                light_id,
                on,
                brightness,
                kelvin,
                fade_ms,
                ..
            } => {
                assert_eq!(light_id, "orphan");
                assert!(*on);
                assert_eq!(*brightness, Some(80));
                assert_eq!(*kelvin, Some(4000));
                assert_eq!(*fade_ms, Some(500));
            }
            other => panic!("Expected SetLight, got {:?}", other),
        }
    }

    #[test]
    fn standalone_device_ignores_synthetic_registry_group_and_uses_direct_light() {
        let (controller, registry) = make_spy_controller();
        registry.lock().unwrap().upsert_room(
            "standalone-device",
            "Standalone lamp",
            "standalone-device",
            &["standalone-device".to_string()],
        );

        block_on(controller.turn_on_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["standalone-device".to_string()],
            },
            LightingCommand::with_transition(65, 3500, 250),
        ))
        .unwrap();

        assert_eq!(controller.client.set_grouped_light_count(), 0);
        let calls = controller.client.set_light_calls();
        assert_eq!(calls.len(), 1);
        assert!(matches!(
            &calls[0],
            HueTransportCall::SetLight { light_id, .. } if light_id == "standalone-device"
        ));
    }

    #[test]
    fn direct_device_target_resolves_and_caches_light_service_id() {
        let (controller, _) = make_spy_controller();
        controller.client.set_resource_response(
            "device/orphan-device",
            serde_json::json!({
                "data": [{
                    "id": "orphan-device",
                    "services": [
                        {"rtype": "zigbee_connectivity", "rid": "zigbee-1"},
                        {"rtype": "light", "rid": "light-service-1"}
                    ]
                }]
            }),
        );

        let target = HubDispatchTarget::Devices {
            native_ids: vec!["orphan-device".to_string()],
        };
        block_on(controller.turn_on_target(&target, LightingCommand::new(80, 4000))).unwrap();
        block_on(controller.turn_off_target(&target, None)).unwrap();

        let light_ids: Vec<String> = controller
            .client
            .set_light_calls()
            .into_iter()
            .filter_map(|call| match call {
                HueTransportCall::SetLight { light_id, .. } => Some(light_id),
                _ => None,
            })
            .collect();
        assert_eq!(light_ids, vec!["light-service-1", "light-service-1"]);
        assert_eq!(
            controller
                .client
                .calls()
                .into_iter()
                .filter(|call| matches!(
                    call,
                    HueTransportCall::GetResources { resource_type }
                        if resource_type == "device/orphan-device"
                ))
                .count(),
            1,
            "the device-to-light mapping should be reused after the first lookup"
        );
    }

    #[test]
    fn flash_target_single_light_uses_native_identify() {
        // Regression for https://github.com/sticktrk/rhythm-os/issues/55:
        // flashing a single Hue light from /api/devices/canonical/:id/flash
        // used to fail with "Hue device-addressed dispatch is not implemented"
        // because the default flash routed through device-addressed turn_on/off
        // which only succeeds when native_ids match a complete grouped_light.
        let (controller, _) = make_spy_controller();

        block_on(controller.flash_target(&HubDispatchTarget::Devices {
            native_ids: vec!["light-uuid-1".to_string()],
        }))
        .unwrap();

        let calls = controller.client.calls();
        let identify_calls: Vec<_> = calls
            .iter()
            .filter(|c| matches!(c, HueTransportCall::IdentifyLight { .. }))
            .collect();
        assert_eq!(identify_calls.len(), 1, "expected one identify call");
        match &identify_calls[0] {
            HueTransportCall::IdentifyLight { light_id } => {
                assert_eq!(light_id, "light-uuid-1");
            }
            _ => unreachable!(),
        }
        // Native identify must not toggle on/off — that was the broken path.
        assert_eq!(controller.client.set_grouped_light_count(), 0);
    }

    #[test]
    fn flash_target_group_identifies_each_member_light() {
        let (controller, registry) = make_spy_controller();
        registry.lock().unwrap().upsert_room(
            "room_group",
            "Living Room",
            "gl-group",
            &["light-a".to_string(), "light-b".to_string()],
        );

        block_on(controller.flash_target(&HubDispatchTarget::Group {
            room_id: "room_group".to_string(),
            control_id: "gl-group".to_string(),
        }))
        .unwrap();

        let identify_ids: Vec<String> = controller
            .client
            .calls()
            .into_iter()
            .filter_map(|c| match c {
                HueTransportCall::IdentifyLight { light_id } => Some(light_id),
                _ => None,
            })
            .collect();
        assert_eq!(identify_ids.len(), 2);
        assert!(identify_ids.contains(&"light-a".to_string()));
        assert!(identify_ids.contains(&"light-b".to_string()));
    }

    #[test]
    fn turn_off_device_target_uses_matching_grouped_light() {
        let (controller, registry) = make_spy_controller();
        registry.lock().unwrap().upsert_room(
            "room_devices",
            "Device-backed room",
            "gl-devices",
            &["dev1".to_string()],
        );

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["dev1".to_string()],
            },
            Some(80),
        ))
        .unwrap();

        let calls = controller.client.set_grouped_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetGroupedLight {
                grouped_light_id,
                on,
                fade_ms,
                ..
            } => {
                assert_eq!(grouped_light_id, "gl-devices");
                assert!(!*on);
                assert_eq!(*fade_ms, Some(80));
            }
            other => panic!("Expected SetGroupedLight, got {:?}", other),
        }
    }

    #[test]
    fn turn_off_device_target_without_match_uses_direct_light() {
        let (controller, _) = make_spy_controller();

        block_on(controller.turn_off_target(
            &HubDispatchTarget::Devices {
                native_ids: vec!["orphan".to_string()],
            },
            Some(80),
        ))
        .unwrap();

        assert_eq!(controller.client.set_grouped_light_count(), 0);
        let calls = controller.client.set_light_calls();
        assert_eq!(calls.len(), 1);
        match &calls[0] {
            HueTransportCall::SetLight {
                light_id,
                on,
                fade_ms,
                ..
            } => {
                assert_eq!(light_id, "orphan");
                assert!(!*on);
                assert_eq!(*fade_ms, Some(80));
            }
            other => panic!("Expected SetLight, got {:?}", other),
        }
    }

    #[test]
    fn direct_device_target_transport_failures_include_device_context() {
        let (controller, _) = make_spy_controller();
        controller.client.set_should_fail(true);
        let target = HubDispatchTarget::Devices {
            native_ids: vec!["orphan".to_string()],
        };

        let read_error = block_on(controller.any_lights_on_target(&target)).unwrap_err();
        assert!(read_error
            .to_string()
            .contains("Failed to check Hue light orphan for device orphan"));

        let on_error = block_on(controller.turn_on_target(&target, LightingCommand::new(80, 4000)))
            .unwrap_err();
        assert!(on_error
            .to_string()
            .contains("Failed to turn on Hue light orphan for device orphan"));

        let off_error = block_on(controller.turn_off_target(&target, None)).unwrap_err();
        assert!(off_error
            .to_string()
            .contains("Failed to turn off Hue light orphan for device orphan"));

        let identify_error = block_on(controller.flash_target(&target)).unwrap_err();
        assert!(identify_error
            .to_string()
            .contains("Hue identify_light failed for orphan (device orphan)"));
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
