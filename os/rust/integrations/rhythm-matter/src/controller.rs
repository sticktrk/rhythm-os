//! Matter light controller generic over transport.
//!
//! Implements `rhythm_core::controller::LightController` using Matter cluster
//! commands via any `MatterTransport` implementation. Controls rooms through
//! per-device fan-out (Matter has no native room grouping).

use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use log::{info, warn};
use rhythm_core::controller::{LightControlError, LightControlResult, LightController};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;

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
    /// Shared atomic for fade duration (ms). Updated via settings API.
    fade_ms: Arc<AtomicU16>,
}

impl<T: MatterTransport> MatterLightController<T> {
    /// Create a new Matter light controller.
    pub fn new(
        transport: Arc<T>,
        hub_data: Arc<MatterHubData>,
        fade_ms: Arc<AtomicU16>,
    ) -> Self {
        Self {
            transport,
            hub_data,
            fade_ms,
        }
    }

    /// Parse a Matter device ID string into (node_id, endpoint).
    ///
    /// Format: "matter-{node_id}" or "matter-{node_id}-{endpoint}"
    /// Default endpoint is 1 if not specified.
    fn parse_device_id(device_id: &str) -> Option<(u64, u16)> {
        let parts: Vec<&str> = device_id.strip_prefix("matter-")?.split('-').collect();
        let node_id = parts.first()?.parse::<u64>().ok()?;
        let endpoint = parts.get(1).and_then(|e| e.parse::<u16>().ok()).unwrap_or(1);
        Some((node_id, endpoint))
    }
}

#[async_trait]
impl<T: MatterTransport + 'static> LightController for MatterLightController<T> {
    async fn turn_on(&self, room_id: &str, command: LightingCommand) -> LightControlResult<()> {
        // Verify room exists (for Matter, grouped_light_id == room_id)
        let _target =
            rhythm_os::controller_helpers::resolve_room_target(&self.hub_data.registry, room_id)?;

        let fade_ms = self.fade_ms.load(Ordering::Relaxed);
        let transition_tenths = fade_ms / 100;
        let level = clusters::brightness_to_level(command.brightness);
        let mireds = clusters::kelvin_to_mireds(command.kelvin);

        // Get light device IDs for this room
        let device_ids = {
            let registry = self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        // Fan out to each device
        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                warn!(target: "cmd", "Matter: invalid device ID format: {}", device_id);
                continue;
            };

            // Send level (with on/off) + color temperature
            if let Err(e) =
                clusters::send_level(&*self.transport, node_id, endpoint, level, transition_tenths)
            {
                warn!(target: "cmd", "Matter: level command failed for node {}: {}", node_id, e);
            }

            if let Err(e) = clusters::send_color_temperature(
                &*self.transport,
                node_id,
                endpoint,
                mireds,
                transition_tenths,
            ) {
                warn!(target: "cmd", "Matter: color temp command failed for node {}: {}", node_id, e);
            }
        }

        info!(target: "cmd",
            "Matter turn_on: room={} bri={} kelvin={} devices={}",
            room_id, command.brightness, command.kelvin, device_ids.len()
        );

        Ok(())
    }

    async fn turn_off(&self, room_id: &str) -> LightControlResult<()> {
        let device_ids = {
            let registry = self.hub_data.registry.lock().map_err(|e| {
                LightControlError::Internal(format!("Failed to lock registry: {}", e))
            })?;
            registry.get_light_entities(room_id)
        };

        for device_id in &device_ids {
            let Some((node_id, endpoint)) = Self::parse_device_id(device_id) else {
                continue;
            };
            if let Err(e) = clusters::send_off(&*self.transport, node_id, endpoint) {
                warn!(target: "cmd", "Matter: off command failed for node {}: {}", node_id, e);
            }
        }

        info!(target: "cmd", "Matter turn_off: room={} devices={}", room_id, device_ids.len());
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

    #[test]
    fn parse_device_id_basic() {
        assert_eq!(
            MatterLightController::<crate::test_support::NoOpTransport>::parse_device_id(
                "matter-42"
            ),
            Some((42, 1))
        );
    }

    #[test]
    fn parse_device_id_with_endpoint() {
        assert_eq!(
            MatterLightController::<crate::test_support::NoOpTransport>::parse_device_id(
                "matter-42-2"
            ),
            Some((42, 2))
        );
    }

    #[test]
    fn parse_device_id_invalid() {
        assert_eq!(
            MatterLightController::<crate::test_support::NoOpTransport>::parse_device_id(
                "hue-abc"
            ),
            None
        );
    }
}
