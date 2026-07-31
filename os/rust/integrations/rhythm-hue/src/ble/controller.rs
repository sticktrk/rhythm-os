//! Standard Rhythm light controller backed by Hue's direct BLE protocol.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use log::warn;
use rhythm_core::controller::{
    HubDispatchTarget, HubLightController, LightControlError, LightControlResult,
};
use rhythm_core::lighting::LightingCommand;
use rhythm_core::room::Room;

use super::protocol::{
    HUE_DEFAULT_MAX_MIRED, HUE_DEFAULT_MIN_MIRED, HUE_MAX_BRIGHTNESS, HUE_MIN_BRIGHTNESS,
};
use super::store::HueBleDeviceStore;
use super::transport::HueBleTransport;
use super::types::{HueBleColor, HueBleCommand, HueBleDevice};

pub struct HueBleLightController {
    transport: Arc<dyn HueBleTransport>,
    store: Arc<HueBleDeviceStore>,
    registry: Arc<Mutex<rhythm_os::registry::HubDeviceRegistry>>,
}

impl HueBleLightController {
    pub fn new(
        transport: Arc<dyn HueBleTransport>,
        store: Arc<HueBleDeviceStore>,
        registry: Arc<Mutex<rhythm_os::registry::HubDeviceRegistry>>,
    ) -> Self {
        Self {
            transport,
            store,
            registry,
        }
    }

    fn native_ids(&self, target: &HubDispatchTarget) -> LightControlResult<Vec<String>> {
        match target {
            HubDispatchTarget::Devices { native_ids } => Ok(native_ids.clone()),
            HubDispatchTarget::Group { room_id, .. } => {
                let registry = self.registry.lock().map_err(|_| {
                    LightControlError::Internal("Hue BLE registry lock poisoned".to_string())
                })?;
                let ids = registry.get_light_entities(room_id);
                if ids.is_empty() {
                    return Err(LightControlError::RoomNotFound(format!(
                        "Hue BLE room has no bulbs: {room_id}"
                    )));
                }
                Ok(ids)
            }
        }
    }

    fn device(&self, id: &str) -> LightControlResult<HueBleDevice> {
        self.store
            .get(id)
            .ok_or_else(|| LightControlError::CommandFailed(format!("Unknown Hue BLE bulb: {id}")))
    }

    fn command_for(device: &HueBleDevice, command: &LightingCommand) -> HueBleCommand {
        let brightness = ((f32::from(command.brightness.clamp(1, 100)) / 100.0)
            * f32::from(HUE_MAX_BRIGHTNESS))
        .round()
        .clamp(f32::from(HUE_MIN_BRIGHTNESS), f32::from(HUE_MAX_BRIGHTNESS))
            as u8;
        let color = if command.is_direct_color && device.capabilities.xy_color {
            Some(HueBleColor::Xy {
                x: command.xy.x,
                y: command.xy.y,
            })
        } else if !command.is_direct_color && device.capabilities.color_temperature {
            let min = device
                .capabilities
                .min_mired
                .unwrap_or(HUE_DEFAULT_MIN_MIRED);
            let max = device
                .capabilities
                .max_mired
                .unwrap_or(HUE_DEFAULT_MAX_MIRED);
            let requested_mired = if command.kelvin == 0 {
                max
            } else {
                (1_000_000u32 / u32::from(command.kelvin)).clamp(1, u32::from(u16::MAX)) as u16
            };
            Some(HueBleColor::ColorTemperature {
                mired: requested_mired.clamp(min, max),
            })
        } else if device.capabilities.xy_color {
            Some(HueBleColor::Xy {
                x: command.xy.x,
                y: command.xy.y,
            })
        } else {
            None
        };
        HueBleCommand {
            on: Some(true),
            brightness: device.capabilities.dimming.then_some(brightness),
            color,
            ..Default::default()
        }
    }

    fn apply_to_ids(
        &self,
        ids: &[String],
        command: impl Fn(&HueBleDevice) -> HueBleCommand,
    ) -> LightControlResult<()> {
        if ids.is_empty() {
            return Err(LightControlError::CommandFailed(
                "Hue BLE target has no bulbs".to_string(),
            ));
        }
        let mut errors = Vec::new();
        for id in ids {
            let device = match self.device(id) {
                Ok(device) => device,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            if let Err(error) = self.transport.apply_command(&device, &command(&device)) {
                errors.push(format!("{}: {error:#}", device.display_name()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(errors.join("; ")))
        }
    }

    fn read_any_on(&self, ids: &[String]) -> LightControlResult<bool> {
        let mut successful_read = false;
        let mut errors = Vec::new();
        for id in ids {
            let device = self.device(id)?;
            match self.transport.read_state(&device) {
                Ok(state) => {
                    successful_read = true;
                    let _ = self.store.cache_state(id, state);
                    if state.on == Some(true) {
                        return Ok(true);
                    }
                }
                Err(error) => errors.push(format!("{}: {error:#}", device.display_name())),
            }
        }
        if successful_read || ids.is_empty() {
            Ok(false)
        } else {
            Err(LightControlError::CommandFailed(errors.join("; ")))
        }
    }
}

#[async_trait]
impl HubLightController for HueBleLightController {
    async fn turn_on_target(
        &self,
        target: &HubDispatchTarget,
        command: LightingCommand,
    ) -> LightControlResult<()> {
        if command.transition_ms.is_some_and(|duration| duration > 0) {
            // Hue's direct BLE TLVs have no duration field. The bulb applies
            // its own short native fade; host-timed stepping would introduce
            // jitter and contention across rooms.
            tracing::debug!(
                target: "cmd",
                "Hue BLE ignores requested transition_ms={:?}",
                command.transition_ms
            );
        }
        let ids = self.native_ids(target)?;
        self.apply_to_ids(&ids, |device| Self::command_for(device, &command))
    }

    async fn turn_off_target(
        &self,
        target: &HubDispatchTarget,
        transition_ms: Option<u32>,
    ) -> LightControlResult<()> {
        if transition_ms.is_some_and(|duration| duration > 0) {
            tracing::debug!(
                target: "cmd",
                "Hue BLE ignores requested off transition_ms={transition_ms:?}"
            );
        }
        let ids = self.native_ids(target)?;
        self.apply_to_ids(&ids, |_| HueBleCommand {
            on: Some(false),
            ..Default::default()
        })
    }

    async fn get_rooms(&self) -> LightControlResult<Vec<Room>> {
        rhythm_os::controller_helpers::rooms_from_registry(&self.registry)
    }

    async fn is_connected(&self) -> bool {
        self.transport.is_available().unwrap_or_else(|error| {
            warn!(target: "cmd", "Hue BLE adapter check failed: {error:#}");
            false
        })
    }

    async fn any_lights_on_target(&self, target: &HubDispatchTarget) -> LightControlResult<bool> {
        let ids = self.native_ids(target)?;
        self.read_any_on(&ids)
    }

    async fn flash_target(&self, target: &HubDispatchTarget) -> LightControlResult<()> {
        let ids = self.native_ids(target)?;
        let mut errors = Vec::new();
        for id in ids {
            let device = self.device(&id)?;
            if let Err(error) = self.transport.identify(&device) {
                errors.push(format!("{}: {error:#}", device.display_name()));
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(errors.join("; ")))
        }
    }

    fn name(&self) -> &str {
        "HueBLE"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::types::HueBleCapabilities;

    fn color_device(model: &str) -> HueBleDevice {
        HueBleDevice {
            id: "hue-ble-test".to_string(),
            address: "00:11:22:33:44:55".to_string(),
            address_type: "random".to_string(),
            eui64: "0017880100000001".to_string(),
            name: "Hue lamp".to_string(),
            manufacturer: "Signify Netherlands B.V.".to_string(),
            model: model.to_string(),
            firmware: String::new(),
            capabilities: HueBleCapabilities::from_gatt(
                "Signify Netherlands B.V.",
                model,
                true,
                true,
                true,
                true,
            ),
            paired_at_epoch_secs: 0,
            last_state: None,
        }
    }

    #[test]
    fn lca013_commands_reach_the_full_20k_temperature_range() {
        let device = color_device("LCA013");

        let coolest =
            HueBleLightController::command_for(&device, &LightingCommand::new(80, 20_000));
        assert_eq!(
            coolest.color,
            Some(HueBleColor::ColorTemperature { mired: 50 })
        );

        let warmest = HueBleLightController::command_for(&device, &LightingCommand::new(80, 1_000));
        assert_eq!(
            warmest.color,
            Some(HueBleColor::ColorTemperature { mired: 1000 })
        );
    }

    #[test]
    fn unknown_hue_models_remain_safely_in_the_legacy_range() {
        let device = color_device("UNKNOWN");
        let command =
            HueBleLightController::command_for(&device, &LightingCommand::new(80, 20_000));
        assert_eq!(
            command.color,
            Some(HueBleColor::ColorTemperature {
                mired: HUE_DEFAULT_MIN_MIRED
            })
        );
    }

    #[test]
    fn direct_color_does_not_force_ct_only_bulbs_to_the_warmest_white() {
        let mut device = color_device("UNKNOWN");
        device.capabilities.xy_color = false;
        let direct = LightingCommand::from_color(
            80,
            rhythm_core::Rgb::new(255, 0, 0),
            rhythm_core::XyColor::new(0.7, 0.3),
            None,
        );
        let command = HueBleLightController::command_for(&device, &direct);
        assert_eq!(command.color, None);
    }
}
