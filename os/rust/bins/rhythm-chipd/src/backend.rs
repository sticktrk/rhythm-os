use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};

use rhythm_matter::chip_rpc::ChipInitControllerResponse;
use rhythm_matter::transport::{CommissionedDevice, MatterCommissionRequest};

use crate::service::CommissioningState;

mod chip_ffi;

pub trait ChipControllerBackend {
    fn init_controller(
        &mut self,
        state: &CommissioningState,
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse>;

    fn commission_light(&mut self, request: &MatterCommissionRequest)
        -> Result<CommissionedDevice>;
    fn probe_light(&mut self, node_id: u64) -> Result<CommissionedDevice>;
    fn decommission_device(&mut self, node_id: u64, force: bool) -> Result<()>;
    fn set_on_off(&mut self, node_id: u64, endpoint: u16, on: bool) -> Result<()>;
    fn set_brightness(
        &mut self,
        node_id: u64,
        endpoint: u16,
        level: u8,
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
    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool>;
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
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse> {
        let controller = chip_ffi::ChipFfiController::initialize(state, existing_devices)
            .context("initializing direct CHIP controller bridge")?;
        self.controller = Some(controller);
        Ok(ChipInitControllerResponse {
            fabric_id: state.fabric_id.clone(),
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

    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.controller_mut()?.read_on_off(node_id, endpoint)
    }
}

#[derive(Default)]
pub struct FakeChipBackend {
    state: Option<CommissioningState>,
    devices: BTreeMap<u64, CommissionedDevice>,
    on_off: HashMap<(u64, u16), bool>,
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
        existing_devices: &[CommissionedDevice],
    ) -> Result<ChipInitControllerResponse> {
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
        Ok(ChipInitControllerResponse {
            fabric_id: state.fabric_id.clone(),
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

    fn set_brightness(
        &mut self,
        node_id: u64,
        _endpoint: u16,
        _level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        let _ = self.require_device_mut(node_id)?;
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

    fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.require_device(node_id)?;
        Ok(*self.on_off.get(&(node_id, endpoint)).unwrap_or(&false))
    }
}
