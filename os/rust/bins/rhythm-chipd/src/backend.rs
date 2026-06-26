use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};

use rhythm_matter::chip_rpc::ChipInitControllerResponse;
use rhythm_matter::transport::{
    CommissionedDevice, MatterAttributeReport, MatterCommissionRequest, MatterGroup,
    MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
};

use crate::service::CommissioningState;

mod chip_ffi;

pub trait ChipControllerBackend {
    fn init_controller(
        &mut self,
        state: &CommissioningState,
        ble_controller: Option<u16>,
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse>;

    fn commission_light(&mut self, request: &MatterCommissionRequest)
        -> Result<CommissionedDevice>;
    fn probe_light(&mut self, node_id: u64) -> Result<CommissionedDevice>;
    fn decommission_device(&mut self, node_id: u64, force: bool) -> Result<()>;
    fn set_on_off(&mut self, node_id: u64, endpoint: u16, on: bool) -> Result<()>;
    fn configure_group(&mut self, group: &MatterGroup) -> Result<()>;
    fn remove_group(&mut self, group_id: u16, members: &[MatterGroupMember]) -> Result<()>;
    fn set_group_on_off(&mut self, group_id: u16, on: bool) -> Result<()>;
    fn identify_group(&mut self, group_id: u16, duration_secs: u16) -> Result<()>;
    fn set_group_brightness(
        &mut self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_group_color_temperature(
        &mut self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_group_xy(
        &mut self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_group_hue_saturation(
        &mut self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn identify_light(&mut self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()>;
    fn set_brightness(
        &mut self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn run_level_command(
        &mut self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_color_temperature(
        &mut self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_xy(
        &mut self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn set_hue_saturation(
        &mut self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()>;
    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool>;
    fn read_light_capability_snapshot(
        &mut self,
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value>;
    fn read_light_state(&mut self, node_id: u64, endpoint: u16) -> Result<serde_json::Value>;
    fn subscribe_on_off(
        &mut self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()>;
    fn drain_attribute_reports(&mut self) -> Result<Vec<MatterAttributeReport>>;
}

pub fn build_backend_from_env() -> Box<dyn ChipControllerBackend> {
    match std::env::var("RHYTHM_CHIPD_BACKEND").ok().as_deref() {
        Some("fake") => Box::new(FakeChipBackend::default()),
        _ => Box::new(NativeChipBackend::default()),
    }
}

#[derive(Default)]
pub struct NativeChipBackend {
    controller: Option<chip_ffi::ChipFfiController>,
}

impl NativeChipBackend {
    fn controller_mut(&mut self) -> Result<&mut chip_ffi::ChipFfiController> {
        self.controller
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("CHIP controller backend not initialized"))
    }
}

impl ChipControllerBackend for NativeChipBackend {
    fn init_controller(
        &mut self,
        state: &CommissioningState,
        ble_controller: Option<u16>,
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse> {
        let controller =
            chip_ffi::ChipFfiController::initialize(state, ble_controller, existing_devices)
                .context("initializing direct CHIP controller bridge")?;
        let compressed_fabric_id = controller.compressed_fabric_id().map(str::to_string);
        self.controller = Some(controller);
        Ok(ChipInitControllerResponse {
            fabric_id: state.fabric_id.clone(),
            operational_fabric_id: state.operational_fabric_id,
            compressed_fabric_id,
        })
    }

    fn commission_light(
        &mut self,
        request: &MatterCommissionRequest,
    ) -> Result<CommissionedDevice> {
        self.controller_mut()?.commission_light(request)
    }

    fn probe_light(&mut self, node_id: u64) -> Result<CommissionedDevice> {
        self.controller_mut()?.probe_light(node_id)
    }

    fn decommission_device(&mut self, node_id: u64, force: bool) -> Result<()> {
        self.controller_mut()?.decommission_device(node_id, force)
    }

    fn set_on_off(&mut self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        self.controller_mut()?.set_on_off(node_id, endpoint, on)
    }

    fn configure_group(&mut self, group: &MatterGroup) -> Result<()> {
        self.controller_mut()?.configure_group(group)
    }

    fn remove_group(&mut self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        self.controller_mut()?.remove_group(group_id, members)
    }

    fn set_group_on_off(&mut self, group_id: u16, on: bool) -> Result<()> {
        self.controller_mut()?.set_group_on_off(group_id, on)
    }

    fn identify_group(&mut self, group_id: u16, duration_secs: u16) -> Result<()> {
        self.controller_mut()?
            .identify_group(group_id, duration_secs)
    }

    fn set_group_brightness(
        &mut self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut()?
            .set_group_brightness(group_id, level, transition_ms)
    }

    fn set_group_color_temperature(
        &mut self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut()?
            .set_group_color_temperature(group_id, kelvin, transition_ms)
    }

    fn set_group_xy(
        &mut self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut()?
            .set_group_xy(group_id, x, y, transition_ms)
    }

    fn set_group_hue_saturation(
        &mut self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut()?
            .set_group_hue_saturation(group_id, hue, saturation, transition_ms)
    }

    fn identify_light(&mut self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        self.controller_mut()?
            .identify_light(node_id, endpoint, duration_secs)
    }

    fn set_brightness(
        &mut self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut().and_then(|controller| {
            controller.set_brightness(node_id, endpoint, level, transition_ms)
        })
    }

    fn run_level_command(
        &mut self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut().and_then(|controller| {
            controller.run_level_command(
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            )
        })
    }

    fn set_color_temperature(
        &mut self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut().and_then(|controller| {
            controller.set_color_temperature(node_id, endpoint, kelvin, transition_ms)
        })
    }

    fn set_xy(
        &mut self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut()
            .and_then(|controller| controller.set_xy(node_id, endpoint, x, y, transition_ms))
    }

    fn set_hue_saturation(
        &mut self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.controller_mut().and_then(|controller| {
            controller.set_hue_saturation(node_id, endpoint, hue, saturation, transition_ms)
        })
    }

    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.controller_mut()?.read_on_off(node_id, endpoint)
    }

    fn read_light_capability_snapshot(
        &mut self,
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value> {
        self.controller_mut()?
            .read_light_capability_snapshot(node_id, endpoint)
    }

    fn read_light_state(&mut self, node_id: u64, endpoint: u16) -> Result<serde_json::Value> {
        self.controller_mut()?.read_light_state(node_id, endpoint)
    }

    fn subscribe_on_off(
        &mut self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        self.controller_mut()?
            .subscribe_on_off(targets, min_interval_secs, max_interval_secs)
    }

    fn drain_attribute_reports(&mut self) -> Result<Vec<MatterAttributeReport>> {
        self.controller_mut()?.drain_attribute_reports()
    }
}

#[derive(Default)]
pub struct FakeChipBackend {
    state: Option<CommissioningState>,
    devices: BTreeMap<u64, CommissionedDevice>,
    on_off: HashMap<(u64, u16), bool>,
    groups: BTreeMap<u16, MatterGroup>,
}

impl FakeChipBackend {
    fn require_device(&self, node_id: u64) -> Result<&CommissionedDevice> {
        self.devices
            .get(&node_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown fake CHIP node {}", node_id))
    }

    fn require_device_mut(&mut self, node_id: u64) -> Result<&mut CommissionedDevice> {
        self.devices
            .get_mut(&node_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown fake CHIP node {}", node_id))
    }

    fn require_group(&self, group_id: u16) -> Result<&MatterGroup> {
        self.groups
            .get(&group_id)
            .ok_or_else(|| anyhow::anyhow!("Unknown fake CHIP group {}", group_id))
    }

    fn fake_name(setup_payload: &str, suffix: &str) -> String {
        let trimmed = setup_payload.trim();
        let short = trimmed.chars().take(12).collect::<String>();
        format!("{} {}", suffix, short)
    }
}

impl ChipControllerBackend for FakeChipBackend {
    fn init_controller(
        &mut self,
        state: &CommissioningState,
        _ble_controller: Option<u16>,
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse> {
        if state.operational_fabric_id == 0 {
            anyhow::bail!("Fake CHIP operational fabric id must be non-zero");
        }
        if state.ipk_hex.len() != 32 || !state.ipk_hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
            anyhow::bail!("Fake CHIP IPK must be a 16-byte hex string");
        }
        self.state = Some(state.clone());
        self.devices = existing_devices
            .iter()
            .cloned()
            .map(|device| (device.node_id, device))
            .collect();
        self.on_off = existing_devices
            .iter()
            .map(|device| ((device.node_id, device.light_endpoint), false))
            .collect();
        self.groups.clear();
        Ok(ChipInitControllerResponse {
            fabric_id: state.fabric_id.clone(),
            operational_fabric_id: state.operational_fabric_id,
            compressed_fabric_id: Some(format!(
                "{:016X}",
                state.operational_fabric_id ^ 0xA5A5_A5A5_A5A5_A5A5
            )),
        })
    }

    fn commission_light(
        &mut self,
        request: &MatterCommissionRequest,
    ) -> Result<CommissionedDevice> {
        let device = CommissionedDevice {
            node_id: request.node_id,
            vendor_name: Self::fake_name(&request.setup_payload, "FakeVendor"),
            product_name: "Fake Matter Lamp".to_string(),
            vendor_id: 0xFFF1,
            product_id: 0x0001,
            serial_number: Some(format!("fake-{}", request.node_id)),
            light_endpoint: 1,
            color_modes: vec![
                rhythm_matter::transport::MatterColorMode::Xy,
                rhythm_matter::transport::MatterColorMode::ColorTemperature,
            ],
            min_kelvin: Some(2200),
            max_kelvin: Some(6500),
        };
        self.on_off
            .insert((device.node_id, device.light_endpoint), false);
        self.devices.insert(device.node_id, device.clone());
        Ok(device)
    }

    fn probe_light(&mut self, node_id: u64) -> Result<CommissionedDevice> {
        Ok(self.require_device(node_id)?.clone())
    }

    fn decommission_device(&mut self, node_id: u64, _force: bool) -> Result<()> {
        self.devices.remove(&node_id);
        self.on_off
            .retain(|(device_node_id, _), _| *device_node_id != node_id);
        Ok(())
    }

    fn set_on_off(&mut self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        self.require_device(node_id)?;
        self.on_off.insert((node_id, endpoint), on);
        Ok(())
    }

    fn configure_group(&mut self, group: &MatterGroup) -> Result<()> {
        if group.group_id == 0 {
            anyhow::bail!("Invalid fake CHIP group id 0");
        }
        for member in &group.members {
            self.require_device(member.node_id)?;
        }
        self.groups.insert(group.group_id, group.clone());
        Ok(())
    }

    fn remove_group(&mut self, group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
        self.groups.remove(&group_id);
        Ok(())
    }

    fn set_group_on_off(&mut self, group_id: u16, on: bool) -> Result<()> {
        let members = self.require_group(group_id)?.members.clone();
        for member in members {
            self.require_device(member.node_id)?;
            self.on_off.insert((member.node_id, member.endpoint), on);
        }
        Ok(())
    }

    fn identify_group(&mut self, group_id: u16, _duration_secs: u16) -> Result<()> {
        self.require_group(group_id)?;
        Ok(())
    }

    fn set_group_brightness(
        &mut self,
        group_id: u16,
        level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let members = self.require_group(group_id)?.members.clone();
        for member in members {
            self.require_device(member.node_id)?;
            self.on_off
                .insert((member.node_id, member.endpoint), level > 0);
        }
        Ok(())
    }

    fn set_group_color_temperature(
        &mut self,
        group_id: u16,
        _kelvin: u16,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        self.require_group(group_id)?;
        Ok(())
    }

    fn set_group_xy(
        &mut self,
        group_id: u16,
        _x: f32,
        _y: f32,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        self.require_group(group_id)?;
        Ok(())
    }

    fn set_group_hue_saturation(
        &mut self,
        group_id: u16,
        _hue: u8,
        _saturation: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        self.require_group(group_id)?;
        Ok(())
    }

    fn identify_light(&mut self, node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
        self.require_device(node_id)?;
        Ok(())
    }

    fn set_brightness(
        &mut self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = self.require_device_mut(node_id)?;
        self.on_off.insert((node_id, endpoint), level > 0);
        Ok(())
    }

    fn run_level_command(
        &mut self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        _step_mode: Option<MatterLevelStepMode>,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        self.require_device(node_id)?;
        match command {
            MatterLevelCommandVariant::MoveToLevelWithOnOff
            | MatterLevelCommandVariant::StepWithOnOff => {
                self.on_off.insert((node_id, endpoint), level_or_step > 0);
            }
            MatterLevelCommandVariant::MoveToLevel | MatterLevelCommandVariant::Step => {}
        }
        Ok(())
    }

    fn set_color_temperature(
        &mut self,
        node_id: u64,
        _endpoint: u16,
        kelvin: u16,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let device = self.require_device_mut(node_id)?;
        device.min_kelvin = Some(device.min_kelvin.unwrap_or(kelvin).min(kelvin));
        device.max_kelvin = Some(device.max_kelvin.unwrap_or(kelvin).max(kelvin));
        Ok(())
    }

    fn set_xy(
        &mut self,
        node_id: u64,
        _endpoint: u16,
        _x: f32,
        _y: f32,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = self.require_device(node_id)?;
        Ok(())
    }

    fn set_hue_saturation(
        &mut self,
        node_id: u64,
        _endpoint: u16,
        _hue: u8,
        _saturation: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = self.require_device(node_id)?;
        Ok(())
    }

    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.require_device(node_id)?;
        Ok(*self.on_off.get(&(node_id, endpoint)).unwrap_or(&false))
    }

    fn read_light_capability_snapshot(
        &mut self,
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value> {
        let device = self.require_device(node_id)?;
        Ok(serde_json::json!({
            "node_id": node_id,
            "selected_endpoint": endpoint,
            "endpoint_list": [0, device.light_endpoint],
            "server_clusters": [3, 4, 6, 8, 29, 768],
            "client_clusters": [],
            "device_type_list": [
                {"device_type": 0x010C, "revision": 1}
            ],
            "accepted_command_lists": {
                "level_control": [0, 2, 4, 6],
                "color_control": [0, 7, 10],
                "onoff": [0, 1],
            },
            "attribute_lists": {
                "level_control": [0, 65528, 65529, 65531],
                "color_control": [0, 1, 3, 4, 7, 16394, 16395, 65528, 65529, 65531],
                "onoff": [0, 65528, 65529, 65531],
            },
            "level_control": {
                "feature_map": 3,
                "accepted_command_list": [0, 2, 4, 6],
                "attribute_list": [0, 65528, 65529, 65531],
                "current_level": 128,
            },
            "color_control": {
                "feature_map": 25,
                "color_capabilities": 25,
                "accepted_command_list": [0, 7, 10],
                "attribute_list": [0, 1, 3, 4, 7, 16394, 16395, 65528, 65529, 65531],
                "color_temp_physical_min_mireds": device.max_kelvin.map(|kelvin| 1_000_000u32 / kelvin as u32),
                "color_temp_physical_max_mireds": device.min_kelvin.map(|kelvin| 1_000_000u32 / kelvin as u32),
                "current_x": 0,
                "current_y": 0,
                "current_hue": 0,
                "current_saturation": 0,
            },
            "raw_attribute_reads_available": true,
        }))
    }

    fn read_light_state(&mut self, node_id: u64, endpoint: u16) -> Result<serde_json::Value> {
        let on = self.read_on_off(node_id, endpoint)?;
        Ok(serde_json::json!({
            "onoff": {"ok": true, "value": on},
            "current_level": {"ok": true, "value": 128},
            "current_x": {"ok": true, "value": 0},
            "current_y": {"ok": true, "value": 0},
            "current_hue": {"ok": true, "value": 0},
            "current_saturation": {"ok": true, "value": 0},
        }))
    }

    fn subscribe_on_off(
        &mut self,
        targets: &[MatterSubscriptionTarget],
        _min_interval_secs: u16,
        _max_interval_secs: u16,
    ) -> Result<()> {
        for target in targets {
            self.require_device(target.node_id)?;
        }
        Ok(())
    }

    fn drain_attribute_reports(&mut self) -> Result<Vec<MatterAttributeReport>> {
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Mutex;

    use rhythm_matter::transport::{
        MatterColorMode, MatterCommissioningNetwork, MatterCommissioningRendezvous,
        MatterCommissioningWifiCredentials,
    };

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn commissioning_state() -> CommissioningState {
        CommissioningState {
            fabric_id: "fabric-test".to_string(),
            operational_fabric_id: 0x1234,
            ipk_hex: "00112233445566778899aabbccddeeff".to_string(),
            storage_path: PathBuf::from("/tmp/chip-storage/chip.json"),
        }
    }

    fn commissioned_device(node_id: u64, endpoint: u16) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: "Vendor".to_string(),
            product_name: format!("Lamp {}", node_id),
            vendor_id: 1,
            product_id: 2,
            serial_number: Some(format!("serial-{}", node_id)),
            light_endpoint: endpoint,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(5000),
        }
    }

    fn commission_request(node_id: u64, payload: &str) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: payload.to_string(),
            node_id,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Auto,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "Rhythm".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    fn string_error<T>(result: Result<T>) -> String {
        match result {
            Ok(_) => panic!("expected error"),
            Err(error) => error.to_string(),
        }
    }

    #[test]
    fn build_backend_from_env_selects_fake_or_native_backend() {
        let _guard = ENV_LOCK.lock().unwrap();
        std::env::set_var("RHYTHM_CHIPD_BACKEND", "fake");

        let mut fake = build_backend_from_env();
        let response = fake
            .init_controller(&commissioning_state(), Some(0), &[])
            .unwrap();
        assert_eq!(response.fabric_id, "fabric-test");
        assert_eq!(response.operational_fabric_id, 0x1234);

        std::env::remove_var("RHYTHM_CHIPD_BACKEND");
        let mut native = build_backend_from_env();
        assert!(string_error(native.read_on_off(1, 1))
            .contains("CHIP controller backend not initialized"));
    }

    #[test]
    fn fake_backend_initializes_and_validates_commissioning_state() {
        let mut backend = FakeChipBackend::default();
        let existing = vec![commissioned_device(10, 2)];

        let response = backend
            .init_controller(&commissioning_state(), Some(1), &existing)
            .unwrap();
        assert_eq!(response.fabric_id, "fabric-test");
        assert_eq!(response.operational_fabric_id, 0x1234);
        assert_eq!(backend.probe_light(10).unwrap().light_endpoint, 2);
        assert!(!backend.read_on_off(10, 2).unwrap());

        let mut invalid_fabric = commissioning_state();
        invalid_fabric.operational_fabric_id = 0;
        assert_eq!(
            string_error(FakeChipBackend::default().init_controller(&invalid_fabric, None, &[])),
            "Fake CHIP operational fabric id must be non-zero"
        );

        let mut invalid_ipk = commissioning_state();
        invalid_ipk.ipk_hex = "not-hex".to_string();
        assert_eq!(
            string_error(FakeChipBackend::default().init_controller(&invalid_ipk, None, &[])),
            "Fake CHIP IPK must be a 16-byte hex string"
        );
    }

    #[test]
    fn fake_backend_commissions_light_and_tracks_endpoint_state() {
        let mut backend = FakeChipBackend::default();
        backend
            .init_controller(&commissioning_state(), None, &[])
            .unwrap();

        let device = backend
            .commission_light(&commission_request(42, "MT:1234567890abcdef"))
            .unwrap();
        assert_eq!(device.node_id, 42);
        assert_eq!(device.vendor_name, "FakeVendor MT:123456789");
        assert_eq!(device.product_name, "Fake Matter Lamp");
        assert_eq!(device.serial_number.as_deref(), Some("fake-42"));
        assert_eq!(device.light_endpoint, 1);
        assert!(device.color_modes.contains(&MatterColorMode::Xy));
        assert!(device
            .color_modes
            .contains(&MatterColorMode::ColorTemperature));

        assert!(!backend.read_on_off(42, 1).unwrap());
        backend.set_on_off(42, 1, true).unwrap();
        assert!(backend.read_on_off(42, 1).unwrap());

        backend.set_brightness(42, 1, 0, Some(100)).unwrap();
        assert!(!backend.read_on_off(42, 1).unwrap());
        backend
            .run_level_command(
                42,
                1,
                MatterLevelCommandVariant::MoveToLevel,
                254,
                None,
                None,
            )
            .unwrap();
        assert!(
            !backend.read_on_off(42, 1).unwrap(),
            "level commands without OnOff must not flip power"
        );
        backend
            .run_level_command(
                42,
                1,
                MatterLevelCommandVariant::StepWithOnOff,
                4,
                Some(MatterLevelStepMode::Up),
                Some(50),
            )
            .unwrap();
        assert!(backend.read_on_off(42, 1).unwrap());

        backend.set_color_temperature(42, 1, 1800, None).unwrap();
        backend.set_color_temperature(42, 1, 7000, None).unwrap();
        backend.set_xy(42, 1, 0.31, 0.32, Some(250)).unwrap();
        backend.set_hue_saturation(42, 1, 128, 200, None).unwrap();
        backend.identify_light(42, 1, 5).unwrap();
        backend
            .subscribe_on_off(
                &[MatterSubscriptionTarget {
                    node_id: 42,
                    endpoint: 1,
                }],
                1,
                60,
            )
            .unwrap();
        assert!(backend.drain_attribute_reports().unwrap().is_empty());

        let snapshot = backend.read_light_capability_snapshot(42, 1).unwrap();
        assert_eq!(snapshot["node_id"], 42);
        assert_eq!(snapshot["selected_endpoint"], 1);
        assert_eq!(snapshot["raw_attribute_reads_available"], true);
        assert_eq!(
            snapshot["color_control"]["color_temp_physical_min_mireds"],
            142
        );
        assert_eq!(
            snapshot["color_control"]["color_temp_physical_max_mireds"],
            555
        );

        let state = backend.read_light_state(42, 1).unwrap();
        assert_eq!(state["onoff"]["value"], true);

        assert!(string_error(backend.probe_light(999)).contains("Unknown fake CHIP node 999"));
        assert!(string_error(backend.subscribe_on_off(
            &[MatterSubscriptionTarget {
                node_id: 999,
                endpoint: 1,
            }],
            1,
            60,
        ))
        .contains("Unknown fake CHIP node 999"));
    }

    #[test]
    fn fake_backend_applies_group_commands_and_decommissions_devices() {
        let mut backend = FakeChipBackend::default();
        let devices = vec![commissioned_device(1, 1), commissioned_device(2, 3)];
        backend
            .init_controller(&commissioning_state(), None, &devices)
            .unwrap();

        let group = MatterGroup {
            group_id: 7,
            name: "Kitchen".to_string(),
            members: vec![
                MatterGroupMember {
                    node_id: 1,
                    endpoint: 1,
                },
                MatterGroupMember {
                    node_id: 2,
                    endpoint: 3,
                },
            ],
        };

        let mut invalid_group = group.clone();
        invalid_group.group_id = 0;
        assert_eq!(
            string_error(backend.configure_group(&invalid_group)),
            "Invalid fake CHIP group id 0"
        );

        let mut unknown_member_group = group.clone();
        unknown_member_group.members.push(MatterGroupMember {
            node_id: 99,
            endpoint: 1,
        });
        assert!(string_error(backend.configure_group(&unknown_member_group))
            .contains("Unknown fake CHIP node 99"));

        backend.configure_group(&group).unwrap();
        backend.identify_group(7, 3).unwrap();
        backend.set_group_on_off(7, true).unwrap();
        assert!(backend.read_on_off(1, 1).unwrap());
        assert!(backend.read_on_off(2, 3).unwrap());

        backend.set_group_brightness(7, 0, Some(100)).unwrap();
        assert!(!backend.read_on_off(1, 1).unwrap());
        assert!(!backend.read_on_off(2, 3).unwrap());
        backend
            .set_group_color_temperature(7, 3000, Some(100))
            .unwrap();
        backend.set_group_xy(7, 0.1, 0.2, None).unwrap();
        backend.set_group_hue_saturation(7, 20, 200, None).unwrap();

        backend.remove_group(7, &group.members).unwrap();
        assert!(string_error(backend.identify_group(7, 3)).contains("Unknown fake CHIP group 7"));

        backend.decommission_device(1, true).unwrap();
        assert!(string_error(backend.probe_light(1)).contains("Unknown fake CHIP node 1"));
        assert!(string_error(backend.read_on_off(1, 1)).contains("Unknown fake CHIP node 1"));
    }

    #[test]
    fn native_backend_reports_uninitialized_for_operations() {
        let mut backend = NativeChipBackend::default();
        let request = commission_request(1, "MT:code");
        let group = MatterGroup {
            group_id: 1,
            name: "Test".to_string(),
            members: vec![MatterGroupMember {
                node_id: 1,
                endpoint: 1,
            }],
        };
        let targets = vec![MatterSubscriptionTarget {
            node_id: 1,
            endpoint: 1,
        }];

        for error in [
            string_error(backend.commission_light(&request)),
            string_error(backend.probe_light(1)),
            string_error(backend.decommission_device(1, false)),
            string_error(backend.set_on_off(1, 1, true)),
            string_error(backend.configure_group(&group)),
            string_error(backend.remove_group(1, &group.members)),
            string_error(backend.set_group_on_off(1, true)),
            string_error(backend.identify_group(1, 1)),
            string_error(backend.set_group_brightness(1, 1, None)),
            string_error(backend.set_group_color_temperature(1, 2700, None)),
            string_error(backend.set_group_xy(1, 0.1, 0.2, None)),
            string_error(backend.set_group_hue_saturation(1, 1, 1, None)),
            string_error(backend.identify_light(1, 1, 1)),
            string_error(backend.set_brightness(1, 1, 1, None)),
            string_error(backend.run_level_command(
                1,
                1,
                MatterLevelCommandVariant::Step,
                1,
                Some(MatterLevelStepMode::Down),
                None,
            )),
            string_error(backend.set_color_temperature(1, 1, 2700, None)),
            string_error(backend.set_xy(1, 1, 0.1, 0.2, None)),
            string_error(backend.set_hue_saturation(1, 1, 1, 1, None)),
            string_error(backend.read_on_off(1, 1)),
            string_error(backend.read_light_capability_snapshot(1, 1)),
            string_error(backend.read_light_state(1, 1)),
            string_error(backend.subscribe_on_off(&targets, 1, 60)),
            string_error(backend.drain_attribute_reports()),
        ] {
            assert!(
                error.contains("CHIP controller backend not initialized"),
                "unexpected error: {}",
                error
            );
        }
    }
}
