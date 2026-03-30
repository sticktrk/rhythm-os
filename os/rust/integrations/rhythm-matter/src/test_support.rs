//! Test support — no-op and spy transport implementations.

use std::sync::Mutex;

use anyhow::Result;

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterDeviceInfo, MatterTransport, SubscribeSpec,
};

/// A recorded cluster command for test assertions.
#[derive(Debug, Clone)]
pub struct RecordedCommand {
    pub node_id: u64,
    pub endpoint: u16,
    pub cluster: u16,
    pub cmd_id: u8,
    pub payload: Vec<u8>,
}

/// No-op transport that accepts all commands (for unit tests).
pub struct NoOpTransport;

impl MatterTransport for NoOpTransport {
    fn commission(&self, _setup_code: &str) -> Result<CommissionedDevice> {
        Ok(CommissionedDevice {
            node_id: 1,
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

    fn send_cluster_cmd(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _cluster: u16,
        _cmd_id: u8,
        _payload: &[u8],
    ) -> Result<()> {
        Ok(())
    }

    fn read_attribute(
        &self,
        _node_id: u64,
        _endpoint: u16,
        _cluster: u16,
        _attr_id: u16,
    ) -> Result<Vec<u8>> {
        Ok(vec![0]) // Default: off
    }

    fn subscribe(&self, _node_id: u64, _specs: &[SubscribeSpec]) -> Result<()> {
        Ok(())
    }

    fn commissioned_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        Ok(Vec::new())
    }
}

/// Spy transport that records all commands for test assertions.
pub struct SpyTransport {
    commands: Mutex<Vec<RecordedCommand>>,
    devices: Mutex<Vec<MatterDeviceInfo>>,
    on_off_state: Mutex<std::collections::HashMap<u64, bool>>,
}

impl SpyTransport {
    pub fn new() -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            devices: Mutex::new(Vec::new()),
            on_off_state: Mutex::new(std::collections::HashMap::new()),
        }
    }

    pub fn commands(&self) -> Vec<RecordedCommand> {
        self.commands.lock().unwrap().clone()
    }

    pub fn add_device(&self, node_id: u64, vendor: &str, product: &str) {
        self.devices.lock().unwrap().push(MatterDeviceInfo {
            node_id,
            vendor_name: vendor.to_string(),
            product_name: product.to_string(),
            reachable: true,
        });
    }

    pub fn set_on_off(&self, node_id: u64, is_on: bool) {
        self.on_off_state.lock().unwrap().insert(node_id, is_on);
    }
}

impl Default for SpyTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl MatterTransport for SpyTransport {
    fn commission(&self, _setup_code: &str) -> Result<CommissionedDevice> {
        Ok(CommissionedDevice {
            node_id: 99,
            vendor_name: "Test".to_string(),
            product_name: "Commissioned Light".to_string(),
            vendor_id: 0,
            product_id: 0,
            serial_number: None,
            light_endpoint: 1,
            color_modes: vec![MatterColorMode::ColorTemperature],
            min_kelvin: Some(2700),
            max_kelvin: Some(6500),
        })
    }

    fn send_cluster_cmd(
        &self,
        node_id: u64,
        endpoint: u16,
        cluster: u16,
        cmd_id: u8,
        payload: &[u8],
    ) -> Result<()> {
        self.commands.lock().unwrap().push(RecordedCommand {
            node_id,
            endpoint,
            cluster,
            cmd_id,
            payload: payload.to_vec(),
        });
        Ok(())
    }

    fn read_attribute(
        &self,
        node_id: u64,
        _endpoint: u16,
        cluster: u16,
        attr_id: u16,
    ) -> Result<Vec<u8>> {
        // Return on/off state if queried
        if cluster == crate::clusters::CLUSTER_ON_OFF && attr_id == crate::clusters::ATTR_ON_OFF {
            let state = self.on_off_state.lock().unwrap();
            let is_on = state.get(&node_id).copied().unwrap_or(false);
            return Ok(vec![if is_on { 1 } else { 0 }]);
        }
        Ok(vec![0])
    }

    fn subscribe(&self, _node_id: u64, _specs: &[SubscribeSpec]) -> Result<()> {
        Ok(())
    }

    fn commissioned_devices(&self) -> Result<Vec<MatterDeviceInfo>> {
        Ok(self.devices.lock().unwrap().clone())
    }
}
