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
use super::types::{HueBleColor, HueBleCommand, HueBleDevice, HueBleState};

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
        let mut commands = Vec::new();
        for id in ids {
            let device = match self.device(id) {
                Ok(device) => device,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            let command = command(&device);
            commands.push((device, command));
        }
        if !commands.is_empty() {
            match self.transport.apply_commands(&commands) {
                Ok(()) => {
                    for (device, command) in &commands {
                        let state = Self::state_after_command(device.last_state, command);
                        if let Err(error) = self.store.cache_state(&device.id, state) {
                            warn!(
                                target: "cmd",
                                "Hue BLE could not cache commanded state for {}: {error:#}",
                                device.display_name()
                            );
                        }
                    }
                }
                Err(error) => errors.push(format!("Hue BLE batch: {error:#}")),
            }
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(LightControlError::CommandFailed(errors.join("; ")))
        }
    }

    fn state_after_command(previous: Option<HueBleState>, command: &HueBleCommand) -> HueBleState {
        let mut state = previous.unwrap_or_default();
        if let Some(on) = command.on {
            state.on = Some(on);
        }
        if let Some(brightness) = command.brightness {
            state.brightness = Some(brightness);
        }
        if let Some(color) = command.color {
            state.color = Some(color);
        }
        if let Some(effect) = command.effect {
            state.effect = Some(effect);
        }
        state
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

    fn read_cached_any_on(&self, ids: &[String]) -> LightControlResult<bool> {
        let mut unknown = Vec::new();
        for id in ids {
            let device = self.device(id)?;
            match device.last_state.and_then(|state| state.on) {
                Some(true) => return Ok(true),
                Some(false) => {}
                None => unknown.push(device.display_name()),
            }
        }
        if unknown.is_empty() {
            Ok(false)
        } else {
            Err(LightControlError::ConnectionError(format!(
                "Hue BLE cached power state is unavailable for {}",
                unknown.join(", ")
            )))
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

    async fn any_lights_on_target_for_periodic(
        &self,
        target: &HubDispatchTarget,
    ) -> LightControlResult<bool> {
        let ids = self.native_ids(target)?;
        self.read_cached_any_on(&ids)
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
    use crate::ble::types::{
        HueBleCapabilities, HueBlePairingOutcome, HueBlePairingRequest, HueBleState,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    #[derive(Default)]
    struct RecordingBatchTransport {
        batches: Mutex<Vec<Vec<(String, HueBleCommand)>>>,
        single_command_calls: AtomicUsize,
        live_read_calls: AtomicUsize,
    }

    impl HueBleTransport for RecordingBatchTransport {
        fn is_available(&self) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn pair_lights(
            &self,
            _request: &HueBlePairingRequest,
            _record_bond_intent: &(dyn Fn(&str) -> anyhow::Result<()> + Send + Sync),
        ) -> anyhow::Result<HueBlePairingOutcome> {
            unreachable!()
        }

        fn apply_command(
            &self,
            _device: &HueBleDevice,
            _command: &HueBleCommand,
        ) -> anyhow::Result<()> {
            self.single_command_calls.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }

        fn apply_commands(&self, commands: &[(HueBleDevice, HueBleCommand)]) -> anyhow::Result<()> {
            self.batches.lock().unwrap().push(
                commands
                    .iter()
                    .map(|(device, command)| (device.id.clone(), *command))
                    .collect(),
            );
            Ok(())
        }

        fn read_state(&self, _device: &HueBleDevice) -> anyhow::Result<HueBleState> {
            self.live_read_calls.fetch_add(1, Ordering::SeqCst);
            anyhow::bail!("live reads are disabled in this test")
        }

        fn has_local_bond(&self, _device: &HueBleDevice) -> anyhow::Result<bool> {
            Ok(true)
        }

        fn local_hue_bond_addresses(&self) -> anyhow::Result<Vec<String>> {
            Ok(Vec::new())
        }

        fn inspect_local_bond(&self, _address: &str) -> anyhow::Result<HueBleDevice> {
            anyhow::bail!("not used")
        }

        fn validate_pairing_handoff(&self, _device: &HueBleDevice) -> anyhow::Result<()> {
            Ok(())
        }

        fn prepare_pairing_handoff(&self, _device: &HueBleDevice) -> anyhow::Result<Instant> {
            Ok(Instant::now())
        }

        fn remove_local_bond(
            &self,
            _device: &HueBleDevice,
            _handoff_valid_until: Option<Instant>,
        ) -> anyhow::Result<()> {
            Ok(())
        }
    }

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

    fn test_device(id: &str, on: bool) -> HueBleDevice {
        let mut device = color_device("LCA013");
        device.id = id.to_string();
        device.eui64 = id.to_string();
        device.address = if on {
            "00:11:22:33:44:55".to_string()
        } else {
            "00:11:22:33:44:66".to_string()
        };
        device.last_state = Some(HueBleState {
            on: Some(on),
            brightness: Some(10),
            ..Default::default()
        });
        device
    }

    fn test_controller(
        devices: &[HueBleDevice],
    ) -> (
        HueBleLightController,
        Arc<RecordingBatchTransport>,
        Arc<HueBleDeviceStore>,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-hue-controller-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let store = Arc::new(HueBleDeviceStore::load(&dir).unwrap());
        for device in devices {
            store.upsert(device.clone()).unwrap();
        }
        let transport = Arc::new(RecordingBatchTransport::default());
        let controller = HueBleLightController::new(
            transport.clone(),
            store.clone(),
            Arc::new(Mutex::new(rhythm_os::registry::HubDeviceRegistry::new())),
        );
        (controller, transport, store, dir)
    }

    #[test]
    fn logical_group_uses_one_transport_batch_and_updates_observed_state() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", false),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, store, dir) = test_controller(&devices);

        controller
            .apply_to_ids(&ids, |_| HueBleCommand {
                on: Some(true),
                brightness: Some(127),
                ..Default::default()
            })
            .unwrap();

        let batches = transport.batches.lock().unwrap();
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].len(), 2);
        assert_eq!(transport.single_command_calls.load(Ordering::SeqCst), 0);
        drop(batches);
        for id in &ids {
            let state = store.get(id).unwrap().last_state.unwrap();
            assert_eq!(state.on, Some(true));
            assert_eq!(state.brightness, Some(127));
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn background_power_query_uses_passive_cache_without_live_ble_read() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", true),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, _store, dir) = test_controller(&devices);

        let any_on = futures::executor::block_on(
            controller
                .any_lights_on_target_for_periodic(&HubDispatchTarget::Devices { native_ids: ids }),
        )
        .unwrap();

        assert!(any_on);
        assert_eq!(transport.live_read_calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(dir).unwrap();
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
