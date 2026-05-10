//! Test support — no-op and typed spy Matter transports.

use std::collections::{HashMap, HashSet};
use std::sync::Mutex;

use anyhow::Result;

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterDeviceInfo, MatterTransport,
};

/// A recorded typed controller operation for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedOperation {
    SetOnOff {
        node_id: u64,
        endpoint: u16,
        on: bool,
    },
    IdentifyLight {
        node_id: u64,
        endpoint: u16,
        duration_secs: u16,
    },
    SetBrightness {
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    },
    SetColorTemperature {
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    },
    SetXy {
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    },
    SetHueSaturation {
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    },
    ReadOnOff {
        node_id: u64,
        endpoint: u16,
    },
}

/// No-op transport that accepts all typed operations.
pub struct NoOpTransport;

impl MatterTransport for NoOpTransport {
    fn commission_light(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        Ok(CommissionedDevice {
            node_id: request.node_id,
            vendor_name: "Test".to_string(),
            product_name: "Light".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
        })
    }

    fn decommission_device(&self, _node_id: u64, _force: bool) -> Result<()> {
        Ok(())
    }

    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        Ok(Vec::new())
    }

    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
        Ok(CommissionedDevice {
            node_id,
            vendor_name: "Test".to_string(),
            product_name: "Light".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
        })
    }

    fn set_on_off(&self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
        Ok(())
    }

    fn identify_light(&self, _node_id: u64, _endpoint: u16, _duration_secs: u16) -> Result<()> {
        Ok(())
    }

    fn set_brightness(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_color_temperature(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _kelvin: u16,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_xy(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _x: f32,
        _y: f32,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_hue_saturation(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _hue: u8,
        _saturation: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn read_on_off(&self, _node_id: u64, _endpoint: u16) -> Result<bool> {
        Ok(false)
    }
}

/// Spy transport that records typed operations and configurable results.
pub struct SpyTransport {
    operations: Mutex<Vec<RecordedOperation>>,
    devices: Mutex<Vec<MatterDeviceInfo>>,
    probes: Mutex<HashMap<u64, CommissionedDevice>>,
    on_off_state: Mutex<HashMap<u64, bool>>,
    failing_nodes: Mutex<HashSet<u64>>,
    commission_result: Mutex<Result<CommissionedDevice>>,
    commission_requests: Mutex<Vec<MatterCommissionRequest>>,
    decommissioned: Mutex<Vec<(u64, bool)>>,
}

impl SpyTransport {
    pub fn new() -> Self {
        Self {
            operations: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            probes: Mutex::new(HashMap::new()),
            on_off_state: Mutex::new(HashMap::new()),
            failing_nodes: Mutex::new(HashSet::new()),
            commission_result: Mutex::new(Ok(default_device(99))),
            commission_requests: Mutex::new(Vec::new()),
            decommissioned: Mutex::new(Vec::new()),
        }
    }

    pub fn operations(&self) -> Vec<RecordedOperation> {
        self.operations.lock().unwrap().clone()
    }

    pub fn add_device(&self, node_id: u64, vendor: &str, product: &str) {
        self.devices.lock().unwrap().push(MatterDeviceInfo {
            node_id,
            vendor_name: vendor.to_string(),
            product_name: product.to_string(),
            reachable: true,
        });
        self.probes
            .lock()
            .unwrap()
            .entry(node_id)
            .or_insert_with(|| CommissionedDevice {
                node_id,
                vendor_name: vendor.to_string(),
                product_name: product.to_string(),
                vendor_id: 0,
                product_id: 0,
                serial_number: None,
                light_endpoint: 1,
                color_modes: vec![MatterColorMode::ColorTemperature],
                min_kelvin: Some(2700),
                max_kelvin: Some(6500),
            });
    }

    pub fn set_probe_device(&self, device: CommissionedDevice) {
        self.probes.lock().unwrap().insert(device.node_id, device);
    }

    pub fn set_on_off_state(&self, node_id: u64, is_on: bool) {
        self.on_off_state.lock().unwrap().insert(node_id, is_on);
    }

    pub fn fail_node(&self, node_id: u64) {
        self.failing_nodes.lock().unwrap().insert(node_id);
    }

    pub fn set_commission_result(&self, result: Result<CommissionedDevice>) {
        *self.commission_result.lock().unwrap() = result;
    }

    pub fn commission_requests(&self) -> Vec<MatterCommissionRequest> {
        self.commission_requests.lock().unwrap().clone()
    }

    pub fn decommissioned(&self) -> Vec<(u64, bool)> {
        self.decommissioned.lock().unwrap().clone()
    }

    fn should_fail(&self, node_id: u64) -> bool {
        self.failing_nodes.lock().unwrap().contains(&node_id)
    }

    fn record(&self, operation: RecordedOperation) {
        self.operations.lock().unwrap().push(operation);
    }
}

impl Default for SpyTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MatterTransport for SpyTransport {
    fn commission_light(&self, request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        self.commission_requests
            .lock()
            .unwrap()
            .push(request.clone());
        let result = self.commission_result.lock().unwrap();
        match &*result {
            Ok(device) => {
                let mut commissioned = device.clone();
                commissioned.node_id = request.node_id;
                self.devices.lock().unwrap().push(MatterDeviceInfo {
                    node_id: request.node_id,
                    vendor_name: commissioned.vendor_name.clone(),
                    product_name: commissioned.product_name.clone(),
                    reachable: true,
                });
                self.probes
                    .lock()
                    .unwrap()
                    .insert(request.node_id, commissioned.clone());
                Ok(commissioned)
            }
            Err(e) => Err(anyhow::anyhow!(e.to_string())),
        }
    }

    fn decommission_device(&self, node_id: u64, force: bool) -> Result<()> {
        self.decommissioned.lock().unwrap().push((node_id, force));
        self.devices
            .lock()
            .unwrap()
            .retain(|device| device.node_id != node_id);
        self.probes.lock().unwrap().remove(&node_id);
        Ok(())
    }

    fn list_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        Ok(self.devices.lock().unwrap().clone())
    }

    fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
        self.probes
            .lock()
            .unwrap()
            .get(&node_id)
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("device {} not found", node_id))
    }

    fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::SetOnOff {
            node_id,
            endpoint,
            on,
        });
        self.on_off_state.lock().unwrap().insert(node_id, on);
        Ok(())
    }

    fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::IdentifyLight {
            node_id,
            endpoint,
            duration_secs,
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
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::SetBrightness {
            node_id,
            endpoint,
            level,
            transition_ms,
        });
        self.on_off_state.lock().unwrap().insert(node_id, level > 0);
        Ok(())
    }

    fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::SetColorTemperature {
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
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::SetXy {
            node_id,
            endpoint,
            x,
            y,
            transition_ms,
        });
        Ok(())
    }

    fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::SetHueSaturation {
            node_id,
            endpoint,
            hue,
            saturation,
            transition_ms,
        });
        Ok(())
    }

    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        self.record(RecordedOperation::ReadOnOff { node_id, endpoint });
        Ok(self
            .on_off_state
            .lock()
            .unwrap()
            .get(&node_id)
            .copied()
            .unwrap_or(false))
    }
}

fn default_device(node_id: u64) -> CommissionedDevice {
    CommissionedDevice {
        node_id,
        vendor_name: "Test".to_string(),
        product_name: "Commissioned Light".to_string(),
        vendor_id: 0,
        product_id: 0,
        serial_number: None,
        light_endpoint: 1,
        color_modes: vec![MatterColorMode::ColorTemperature],
        min_kelvin: Some(2700),
        max_kelvin: Some(6500),
    }
}
