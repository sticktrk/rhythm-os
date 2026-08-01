//! Standard Rhythm light controller backed by Hue's direct BLE protocol.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
use super::transport::{HueBleCommandTimeout, HueBleTransport};
use super::types::{HueBleColor, HueBleCommand, HueBleDevice};

// The composite dispatcher gives a physical command ten seconds. Start every
// target together and keep each real BLE future inside that outer boundary.
const COMMAND_DISPATCH_TIMEOUT: Duration = Duration::from_secs(9);

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
        let mut timed_out = false;
        let mut commands = Vec::new();
        let mut seen = HashSet::new();
        for id in ids {
            if !seen.insert(id.clone()) {
                continue;
            }
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
            let mut observation_guards = Vec::with_capacity(commands.len());
            let mut guarded_commands = Vec::with_capacity(commands.len());
            for (device, command) in commands {
                let display_name = format!("{} ({})", device.display_name(), device.id);
                match self.store.begin_command_observation(&device.id) {
                    Ok(guard) => {
                        observation_guards.push(guard);
                        guarded_commands.push((device, command));
                    }
                    Err(error) => errors.push(format!(
                        "{display_name}: observation invalidation failed; command was not dispatched: {error:#}"
                    )),
                }
            }
            if !guarded_commands.is_empty() {
                let labels = guarded_commands
                    .iter()
                    .map(|(device, _)| {
                        (
                            device.id.clone(),
                            format!("{} ({})", device.display_name(), device.id),
                        )
                    })
                    .collect::<HashMap<_, _>>();
                let mut pending = labels.keys().cloned().collect::<HashSet<_>>();
                let batch_result = self.transport.apply_commands_until(
                    &guarded_commands,
                    Instant::now() + COMMAND_DISPATCH_TIMEOUT,
                );
                drop(observation_guards);

                match batch_result {
                    Err(error) => {
                        timed_out |= error.is::<HueBleCommandTimeout>();
                        errors.push(format!("Hue BLE dispatch: {error:#}"));
                    }
                    Ok(outcomes) => {
                        for (device_id, outcome) in outcomes {
                            let Some(display_name) = labels.get(&device_id) else {
                                errors.push(format!(
                                    "Hue BLE transport returned an unexpected outcome for {device_id}"
                                ));
                                continue;
                            };
                            if !pending.remove(&device_id) {
                                errors.push(format!(
                                    "Hue BLE transport returned duplicate outcomes for {display_name}"
                                ));
                                continue;
                            }
                            if let Err(error) = outcome {
                                timed_out |= error.is::<HueBleCommandTimeout>();
                                errors.push(format!("{display_name}: {error:#}"));
                            }
                        }
                        for device_id in pending {
                            errors.push(format!(
                                "Hue BLE transport omitted an outcome for {}",
                                labels[&device_id]
                            ));
                        }
                    }
                }
            }
        }
        if errors.is_empty() {
            Ok(())
        } else if timed_out {
            Err(LightControlError::Timeout(errors.join("; ")))
        } else {
            Err(LightControlError::CommandFailed(errors.join("; ")))
        }
    }

    fn read_any_on(&self, ids: &[String]) -> LightControlResult<bool> {
        let mut observed_off = 0;
        let mut errors = Vec::new();
        for id in ids {
            let (device, token) = match self.store.begin_state_observation(id) {
                Ok(observation) => observation,
                Err(error) => {
                    errors.push(error.to_string());
                    continue;
                }
            };
            match self.transport.read_state(&device) {
                Ok(state) => match self.store.record_state_observation(id, token, state) {
                    Ok(true) if state.on == Some(true) => return Ok(true),
                    Ok(true) if state.on == Some(false) => observed_off += 1,
                    Ok(true) => errors.push(format!(
                        "{}: physical power state was unavailable",
                        device.display_name()
                    )),
                    Ok(false) => errors.push(format!(
                        "{}: physical state changed during observation",
                        device.display_name()
                    )),
                    Err(error) => errors.push(format!("{}: {error:#}", device.display_name())),
                },
                Err(error) => errors.push(format!("{}: {error:#}", device.display_name())),
            }
        }
        if ids.is_empty() || observed_off == ids.len() {
            Ok(false)
        } else {
            Err(LightControlError::ConnectionError(errors.join("; ")))
        }
    }

    fn read_cached_any_on(&self, ids: &[String]) -> LightControlResult<bool> {
        let mut unknown = Vec::new();
        for id in ids {
            let device = match self.device(id) {
                Ok(device) => device,
                Err(error) => {
                    unknown.push(error.to_string());
                    continue;
                }
            };
            let observation = match self.store.fresh_state_observation(id) {
                Ok(observation) => observation,
                Err(error) => {
                    unknown.push(format!("{}: {error:#}", device.display_name()));
                    continue;
                }
            };
            match observation.and_then(|state| state.on) {
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
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Instant;

    static NEXT_TEST_DIRECTORY: AtomicUsize = AtomicUsize::new(0);

    struct TestTransport {
        expected_batch_commands: usize,
        started_commands: Mutex<Vec<String>>,
        command_failures: Mutex<HashSet<String>>,
        command_timeouts: Mutex<HashSet<String>>,
        live_states: Mutex<HashMap<String, HueBleState>>,
        live_failures: Mutex<HashSet<String>>,
        live_read_hook: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
        live_read_calls: AtomicUsize,
    }

    impl TestTransport {
        fn new(expected_batch_commands: usize) -> Self {
            Self {
                expected_batch_commands,
                started_commands: Mutex::new(Vec::new()),
                command_failures: Mutex::new(HashSet::new()),
                command_timeouts: Mutex::new(HashSet::new()),
                live_states: Mutex::new(HashMap::new()),
                live_failures: Mutex::new(HashSet::new()),
                live_read_hook: Mutex::new(None),
                live_read_calls: AtomicUsize::new(0),
            }
        }

        fn fail_command_for(&self, id: &str) {
            self.command_failures.lock().unwrap().insert(id.to_string());
        }

        fn time_out_command_for(&self, id: &str) {
            self.command_timeouts.lock().unwrap().insert(id.to_string());
        }

        fn set_live_state(&self, id: &str, state: HueBleState) {
            self.live_states
                .lock()
                .unwrap()
                .insert(id.to_string(), state);
        }

        fn fail_live_read_for(&self, id: &str) {
            self.live_failures.lock().unwrap().insert(id.to_string());
        }

        fn set_live_read_hook(&self, hook: Arc<dyn Fn() + Send + Sync>) {
            *self.live_read_hook.lock().unwrap() = Some(hook);
        }
    }

    impl HueBleTransport for TestTransport {
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
            device: &HueBleDevice,
            _command: &HueBleCommand,
        ) -> anyhow::Result<()> {
            self.started_commands
                .lock()
                .unwrap()
                .push(device.id.clone());
            if self.command_failures.lock().unwrap().contains(&device.id) {
                anyhow::bail!("injected command failure");
            }
            Ok(())
        }

        fn apply_commands_until(
            &self,
            commands: &[(HueBleDevice, HueBleCommand)],
            deadline: Instant,
        ) -> anyhow::Result<Vec<(String, anyhow::Result<()>)>> {
            if deadline <= Instant::now() {
                return Err(anyhow::Error::new(HueBleCommandTimeout));
            }
            if commands.len() != self.expected_batch_commands {
                anyhow::bail!(
                    "received {} commands, expected {}",
                    commands.len(),
                    self.expected_batch_commands
                );
            }
            self.started_commands
                .lock()
                .unwrap()
                .extend(commands.iter().map(|(device, _)| device.id.clone()));
            let command_timeouts = self.command_timeouts.lock().unwrap();
            let command_failures = self.command_failures.lock().unwrap();
            Ok(commands
                .iter()
                .map(|(device, _)| {
                    let result = if command_timeouts.contains(&device.id) {
                        Err(anyhow::Error::new(HueBleCommandTimeout))
                    } else if command_failures.contains(&device.id) {
                        Err(anyhow::anyhow!("injected command failure"))
                    } else {
                        Ok(())
                    };
                    (device.id.clone(), result)
                })
                .collect())
        }

        fn read_state(&self, device: &HueBleDevice) -> anyhow::Result<HueBleState> {
            self.live_read_calls.fetch_add(1, Ordering::SeqCst);
            if let Some(hook) = self.live_read_hook.lock().unwrap().clone() {
                hook();
            }
            if self.live_failures.lock().unwrap().contains(&device.id) {
                anyhow::bail!("injected live read failure");
            }
            self.live_states
                .lock()
                .unwrap()
                .get(&device.id)
                .copied()
                .ok_or_else(|| anyhow::anyhow!("live read is not configured"))
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
        expected_batch_commands: usize,
    ) -> (
        HueBleLightController,
        Arc<TestTransport>,
        Arc<HueBleDeviceStore>,
        std::path::PathBuf,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "rhythm-hue-controller-{}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos(),
            NEXT_TEST_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        let store = Arc::new(HueBleDeviceStore::load(&dir).unwrap());
        for device in devices {
            store.upsert(device.clone()).unwrap();
        }
        let transport = Arc::new(TestTransport::new(expected_batch_commands));
        let controller = HueBleLightController::new(
            transport.clone(),
            store.clone(),
            Arc::new(Mutex::new(rhythm_os::registry::HubDeviceRegistry::new())),
        );
        (controller, transport, store, dir)
    }

    #[test]
    fn logical_group_preserves_keyed_outcomes_without_promoting_command_state() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", false),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, store, dir) = test_controller(&devices, 2);
        for device in &devices {
            let (_, token) = store.begin_state_observation(&device.id).unwrap();
            assert!(store
                .record_state_observation(&device.id, token, device.last_state.unwrap())
                .unwrap());
        }

        controller
            .apply_to_ids(&ids, |_| HueBleCommand {
                on: Some(true),
                brightness: Some(127),
                ..Default::default()
            })
            .unwrap();

        let mut started = transport.started_commands.lock().unwrap().clone();
        started.sort();
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(started, expected);
        for id in &ids {
            let state = store.get(id).unwrap().last_state.unwrap();
            assert_eq!(state.on, Some(false));
            assert_eq!(state.brightness, Some(10));
            assert_eq!(store.fresh_state_observation(id).unwrap(), None);
        }

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn partial_failure_still_attempts_every_bulb() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", false),
            test_device("bulb-three", false),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, _store, dir) = test_controller(&devices, 3);
        transport.fail_command_for("bulb-two");

        let error = controller
            .apply_to_ids(&ids, |_| HueBleCommand {
                on: Some(true),
                ..Default::default()
            })
            .unwrap_err();

        let mut started = transport.started_commands.lock().unwrap().clone();
        started.sort();
        let mut expected = ids.clone();
        expected.sort();
        assert_eq!(started, expected);
        assert!(error.to_string().contains("bulb-two"));
        assert!(error.to_string().contains("injected command failure"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn per_bulb_deadline_surfaces_as_controller_timeout() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", false),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, _store, dir) = test_controller(&devices, 2);
        transport.time_out_command_for("bulb-one");

        let result = controller.apply_to_ids(&ids, |_| HueBleCommand {
            on: Some(true),
            ..Default::default()
        });

        assert!(matches!(result, Err(LightControlError::Timeout(_))));
        assert_eq!(transport.started_commands.lock().unwrap().len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn background_power_query_uses_fresh_physical_cache_without_live_ble_read() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", true),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, store, dir) = test_controller(&devices, 0);
        for device in &devices {
            let (_, token) = store.begin_state_observation(&device.id).unwrap();
            assert!(store
                .record_state_observation(&device.id, token, device.last_state.unwrap())
                .unwrap());
        }

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
    fn persisted_state_is_not_a_fresh_physical_observation() {
        let devices = vec![test_device("bulb-one", true)];
        let ids = vec![devices[0].id.clone()];
        let (controller, transport, _store, dir) = test_controller(&devices, 0);

        let result = futures::executor::block_on(
            controller
                .any_lights_on_target_for_periodic(&HubDispatchTarget::Devices { native_ids: ids }),
        );

        assert!(matches!(result, Err(LightControlError::ConnectionError(_))));
        assert_eq!(transport.live_read_calls.load(Ordering::SeqCst), 0);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn fresh_cached_true_wins_even_when_an_earlier_target_is_unknown() {
        let devices = vec![test_device("bulb-on", true)];
        let ids = vec!["missing-bulb".to_string(), "bulb-on".to_string()];
        let (controller, transport, store, dir) = test_controller(&devices, 0);
        let (_, token) = store.begin_state_observation("bulb-on").unwrap();
        assert!(store
            .record_state_observation("bulb-on", token, devices[0].last_state.unwrap())
            .unwrap());

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
    fn live_false_plus_failure_is_indeterminate() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", false),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, _store, dir) = test_controller(&devices, 0);
        transport.set_live_state("bulb-one", devices[0].last_state.unwrap());
        transport.fail_live_read_for("bulb-two");

        let result = futures::executor::block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices { native_ids: ids }),
        );

        assert!(matches!(result, Err(LightControlError::ConnectionError(_))));
        assert_eq!(transport.live_read_calls.load(Ordering::SeqCst), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_true_wins_over_another_bulbs_failure() {
        let devices = vec![
            test_device("bulb-one", false),
            test_device("bulb-two", true),
        ];
        let ids = devices
            .iter()
            .map(|device| device.id.clone())
            .collect::<Vec<_>>();
        let (controller, transport, _store, dir) = test_controller(&devices, 0);
        transport.fail_live_read_for("bulb-one");
        transport.set_live_state("bulb-two", devices[1].last_state.unwrap());

        let any_on = futures::executor::block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices { native_ids: ids }),
        )
        .unwrap();

        assert!(any_on);
        assert_eq!(transport.live_read_calls.load(Ordering::SeqCst), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn live_true_overlapping_a_command_is_indeterminate() {
        let devices = vec![test_device("bulb-one", true)];
        let ids = vec![devices[0].id.clone()];
        let (controller, transport, store, dir) = test_controller(&devices, 0);
        transport.set_live_state("bulb-one", devices[0].last_state.unwrap());
        let hook_store = Arc::clone(&store);
        transport.set_live_read_hook(Arc::new(move || {
            let guard = hook_store.begin_command_observation("bulb-one").unwrap();
            drop(guard);
        }));

        let result = futures::executor::block_on(
            controller.any_lights_on_target(&HubDispatchTarget::Devices { native_ids: ids }),
        );

        assert!(matches!(result, Err(LightControlError::ConnectionError(_))));
        assert_eq!(store.fresh_state_observation("bulb-one").unwrap(), None);
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
