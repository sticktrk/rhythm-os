//! Test support — no-op and typed spy Matter transports.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Result;

use crate::transport::{
    CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterDeviceInfo, MatterGroup,
    MatterGroupMember, MatterTransport,
};

/// A recorded typed controller operation for test assertions.
#[derive(Debug, Clone, PartialEq)]
pub enum RecordedOperation {
    SetOnOff {
        node_id: u64,
        endpoint: u16,
        on: bool,
    },
    ConfigureGroup {
        group: MatterGroup,
    },
    RemoveGroup {
        group_id: u16,
        members: Vec<MatterGroupMember>,
    },
    SetGroupOnOff {
        group_id: u16,
        on: bool,
    },
    IdentifyGroup {
        group_id: u16,
        duration_secs: u16,
    },
    SetGroupBrightness {
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    },
    SetGroupColorTemperature {
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    },
    SetGroupXy {
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    },
    SetGroupHueSaturation {
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
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

    fn configure_group(&self, _group: &MatterGroup) -> Result<()> {
        Ok(())
    }

    fn remove_group(&self, _group_id: u16, _members: &[MatterGroupMember]) -> Result<()> {
        Ok(())
    }

    fn set_group_on_off(&self, _group_id: u16, _on: bool) -> Result<()> {
        Ok(())
    }

    fn identify_group(&self, _group_id: u16, _duration_secs: u16) -> Result<()> {
        Ok(())
    }

    fn set_group_brightness(
        &self,
        _group_id: u16,
        _level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_group_color_temperature(
        &self,
        _group_id: u16,
        _kelvin: u16,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_group_xy(
        &self,
        _group_id: u16,
        _x: f32,
        _y: f32,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Ok(())
    }

    fn set_group_hue_saturation(
        &self,
        _group_id: u16,
        _hue: u8,
        _saturation: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
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

type GroupRemovalObserver = Box<dyn Fn(u16, &[MatterGroupMember]) + Send + Sync>;

/// Spy transport that records typed operations and configurable results.
pub struct SpyTransport {
    operations: Mutex<Vec<RecordedOperation>>,
    devices: Mutex<Vec<MatterDeviceInfo>>,
    probes: Mutex<HashMap<u64, CommissionedDevice>>,
    on_off_state: Mutex<HashMap<u64, bool>>,
    groups: Mutex<HashMap<u16, MatterGroup>>,
    failing_nodes: Mutex<HashSet<u64>>,
    failing_read_nodes: Mutex<HashSet<u64>>,
    timing_out_nodes: Mutex<HashSet<u64>>,
    color_temperature_delay: Mutex<Option<Duration>>,
    active_color_temperature: AtomicUsize,
    max_active_color_temperature: AtomicUsize,
    failing_groups: Mutex<HashSet<u16>>,
    failing_group_removals: Mutex<HashSet<u16>>,
    group_removal_observer: Mutex<Option<GroupRemovalObserver>>,
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
            groups: Mutex::new(HashMap::new()),
            failing_nodes: Mutex::new(HashSet::new()),
            failing_read_nodes: Mutex::new(HashSet::new()),
            timing_out_nodes: Mutex::new(HashSet::new()),
            color_temperature_delay: Mutex::new(None),
            active_color_temperature: AtomicUsize::new(0),
            max_active_color_temperature: AtomicUsize::new(0),
            failing_groups: Mutex::new(HashSet::new()),
            failing_group_removals: Mutex::new(HashSet::new()),
            group_removal_observer: Mutex::new(None),
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

    pub fn fail_read_node(&self, node_id: u64) {
        self.failing_read_nodes.lock().unwrap().insert(node_id);
    }

    pub fn allow_read_node(&self, node_id: u64) {
        self.failing_read_nodes.lock().unwrap().remove(&node_id);
    }

    pub fn timeout_node_commands(&self, node_id: u64) {
        self.timing_out_nodes.lock().unwrap().insert(node_id);
    }

    pub fn allow_node_commands(&self, node_id: u64) {
        self.timing_out_nodes.lock().unwrap().remove(&node_id);
    }

    pub fn delay_color_temperature(&self, delay: Duration) {
        *self.color_temperature_delay.lock().unwrap() = Some(delay);
    }

    pub fn max_concurrent_color_temperature(&self) -> usize {
        self.max_active_color_temperature.load(Ordering::SeqCst)
    }

    pub fn fail_group_commands(&self, group_id: u16) {
        self.failing_groups.lock().unwrap().insert(group_id);
    }

    pub fn fail_group_removals(&self, group_id: u16) {
        self.failing_group_removals.lock().unwrap().insert(group_id);
    }

    pub fn observe_group_removals<F>(&self, observer: F)
    where
        F: Fn(u16, &[MatterGroupMember]) + Send + Sync + 'static,
    {
        *self.group_removal_observer.lock().unwrap() = Some(Box::new(observer));
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

    fn should_fail_read(&self, node_id: u64) -> bool {
        self.failing_read_nodes.lock().unwrap().contains(&node_id)
    }

    fn should_timeout(&self, node_id: u64) -> bool {
        self.timing_out_nodes.lock().unwrap().contains(&node_id)
    }

    fn timeout_error(node_id: u64) -> anyhow::Error {
        anyhow::anyhow!(
            "setting Matter command for node {}: native/chip_bridge.cc:540: CHIP Error 0x00000032: Timeout",
            node_id
        )
    }

    fn should_fail_group(&self, group_id: u16) -> bool {
        self.failing_groups.lock().unwrap().contains(&group_id)
    }

    fn should_fail_group_removal(&self, group_id: u16) -> bool {
        self.failing_group_removals
            .lock()
            .unwrap()
            .contains(&group_id)
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
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::SetOnOff {
            node_id,
            endpoint,
            on,
        });
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
        self.on_off_state.lock().unwrap().insert(node_id, on);
        Ok(())
    }

    fn configure_group(&self, group: &MatterGroup) -> Result<()> {
        for member in &group.members {
            if self.should_fail(member.node_id) {
                anyhow::bail!("device {} not found in registry", member.node_id);
            }
        }
        self.groups
            .lock()
            .unwrap()
            .insert(group.group_id, group.clone());
        self.record(RecordedOperation::ConfigureGroup {
            group: group.clone(),
        });
        Ok(())
    }

    fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        self.record(RecordedOperation::RemoveGroup {
            group_id,
            members: members.to_vec(),
        });
        if let Some(observer) = self.group_removal_observer.lock().unwrap().as_ref() {
            observer(group_id, members);
        }
        if self.should_fail_group_removal(group_id) {
            anyhow::bail!("group {} removal failed", group_id);
        }
        self.groups.lock().unwrap().remove(&group_id);
        Ok(())
    }

    fn set_group_on_off(&self, group_id: u16, on: bool) -> Result<()> {
        self.record(RecordedOperation::SetGroupOnOff { group_id, on });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        if let Some(group) = self.groups.lock().unwrap().get(&group_id).cloned() {
            for member in group.members {
                self.on_off_state.lock().unwrap().insert(member.node_id, on);
            }
        }
        Ok(())
    }

    fn identify_group(&self, group_id: u16, duration_secs: u16) -> Result<()> {
        self.record(RecordedOperation::IdentifyGroup {
            group_id,
            duration_secs,
        });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        Ok(())
    }

    fn set_group_brightness(
        &self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.record(RecordedOperation::SetGroupBrightness {
            group_id,
            level,
            transition_ms,
        });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        if let Some(group) = self.groups.lock().unwrap().get(&group_id).cloned() {
            for member in group.members {
                self.on_off_state
                    .lock()
                    .unwrap()
                    .insert(member.node_id, level > 0);
            }
        }
        Ok(())
    }

    fn set_group_color_temperature(
        &self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.record(RecordedOperation::SetGroupColorTemperature {
            group_id,
            kelvin,
            transition_ms,
        });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        Ok(())
    }

    fn set_group_xy(
        &self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.record(RecordedOperation::SetGroupXy {
            group_id,
            x,
            y,
            transition_ms,
        });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        Ok(())
    }

    fn set_group_hue_saturation(
        &self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        self.record(RecordedOperation::SetGroupHueSaturation {
            group_id,
            hue,
            saturation,
            transition_ms,
        });
        if self.should_fail_group(group_id) {
            anyhow::bail!("group {} command failed", group_id);
        }
        Ok(())
    }

    fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        if self.should_fail(node_id) {
            anyhow::bail!("device {} not found in registry", node_id);
        }
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::IdentifyLight {
            node_id,
            endpoint,
            duration_secs,
        });
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
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
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::SetBrightness {
            node_id,
            endpoint,
            level,
            transition_ms,
        });
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
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
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::SetColorTemperature {
            node_id,
            endpoint,
            kelvin,
            transition_ms,
        });
        let delay = *self.color_temperature_delay.lock().unwrap();
        if let Some(delay) = delay {
            let active = self.active_color_temperature.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_active_color_temperature
                .fetch_max(active, Ordering::SeqCst);
            std::thread::sleep(delay);
            self.active_color_temperature.fetch_sub(1, Ordering::SeqCst);
        }
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
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
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::SetXy {
            node_id,
            endpoint,
            x,
            y,
            transition_ms,
        });
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
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
        let timeout = self.should_timeout(node_id);
        self.record(RecordedOperation::SetHueSaturation {
            node_id,
            endpoint,
            hue,
            saturation,
            transition_ms,
        });
        if timeout {
            return Err(Self::timeout_error(node_id));
        }
        Ok(())
    }

    fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
        self.record(RecordedOperation::ReadOnOff { node_id, endpoint });
        if self.should_fail(node_id) || self.should_fail_read(node_id) {
            anyhow::bail!(
                "CHIP Error 0x32: Timeout reading on/off for node {}",
                node_id
            );
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::{
        MatterCommissioningNetwork, MatterCommissioningRendezvous,
        MatterCommissioningWifiCredentials,
    };

    fn commission_request(node_id: u64) -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: format!("MT:{node_id}"),
            node_id,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Auto,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "lab".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    fn matter_group(group_id: u16, members: &[(u64, u16)]) -> MatterGroup {
        MatterGroup {
            group_id,
            name: format!("Room {group_id}"),
            members: members
                .iter()
                .map(|(node_id, endpoint)| MatterGroupMember {
                    node_id: *node_id,
                    endpoint: *endpoint,
                })
                .collect(),
        }
    }

    fn custom_device(node_id: u64, vendor: &str, product: &str) -> CommissionedDevice {
        CommissionedDevice {
            node_id,
            vendor_name: vendor.to_string(),
            product_name: product.to_string(),
            vendor_id: 123,
            product_id: 456,
            serial_number: Some(format!("serial-{node_id}")),
            light_endpoint: 2,
            color_modes: vec![MatterColorMode::HueSaturation, MatterColorMode::Xy],
            min_kelvin: None,
            max_kelvin: Some(5000),
        }
    }

    #[test]
    fn noop_transport_accepts_all_operations_and_returns_defaults() {
        let transport = NoOpTransport;
        let request = commission_request(42);

        let commissioned = transport.commission_light(&request).unwrap();
        assert_eq!(commissioned.node_id, 42);
        assert_eq!(commissioned.product_name, "Light");
        assert_eq!(commissioned.light_endpoint, 1);
        assert_eq!(
            commissioned.color_modes,
            vec![MatterColorMode::ColorTemperature]
        );

        transport.decommission_device(42, true).unwrap();
        assert!(transport.list_devices().unwrap().is_empty());
        let probed = transport.probe_light(43).unwrap();
        assert_eq!(probed.node_id, 43);
        assert_eq!(probed.vendor_name, "Test");

        let group = matter_group(7, &[(42, 1), (43, 2)]);
        transport.set_on_off(42, 1, true).unwrap();
        transport.configure_group(&group).unwrap();
        transport
            .remove_group(group.group_id, &group.members)
            .unwrap();
        transport.set_group_on_off(7, false).unwrap();
        transport.identify_group(7, 5).unwrap();
        transport.set_group_brightness(7, 128, Some(250)).unwrap();
        transport
            .set_group_color_temperature(7, 4100, None)
            .unwrap();
        transport.set_group_xy(7, 0.31, 0.29, Some(100)).unwrap();
        transport
            .set_group_hue_saturation(7, 23, 180, None)
            .unwrap();

        transport.identify_light(42, 1, 3).unwrap();
        transport.set_brightness(42, 1, 200, Some(150)).unwrap();
        transport.set_color_temperature(42, 1, 3000, None).unwrap();
        transport.set_xy(42, 1, 0.4, 0.35, Some(50)).unwrap();
        transport.set_hue_saturation(42, 1, 99, 120, None).unwrap();
        assert!(!transport.read_on_off(42, 1).unwrap());
    }

    #[test]
    fn spy_transport_commissions_probes_decommissions_and_records_failures() {
        let transport = SpyTransport::new();
        transport.set_commission_result(Ok(custom_device(999, "Acme", "Pendant")));

        let request = commission_request(123);
        let commissioned = transport.commission_light(&request).unwrap();
        assert_eq!(commissioned.node_id, 123);
        assert_eq!(commissioned.vendor_name, "Acme");
        assert_eq!(commissioned.product_name, "Pendant");
        assert_eq!(commissioned.light_endpoint, 2);
        assert_eq!(transport.commission_requests(), vec![request.clone()]);

        let devices = transport.list_devices().unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].node_id, 123);
        assert_eq!(devices[0].vendor_name, "Acme");
        assert!(devices[0].reachable);

        let probed = transport.probe_light(123).unwrap();
        assert_eq!(probed.node_id, 123);
        assert_eq!(probed.product_name, "Pendant");

        transport.set_probe_device(custom_device(55, "Other", "Table Lamp"));
        let replacement = transport.probe_light(55).unwrap();
        assert_eq!(replacement.node_id, 55);
        assert_eq!(replacement.vendor_name, "Other");
        assert_eq!(replacement.product_name, "Table Lamp");

        transport.decommission_device(123, true).unwrap();
        assert_eq!(transport.decommissioned(), vec![(123, true)]);
        assert!(transport.probe_light(123).is_err());
        assert!(transport.list_devices().unwrap().is_empty());

        transport.set_commission_result(Err(anyhow::anyhow!("fabric unavailable")));
        let error = transport
            .commission_light(&commission_request(124))
            .unwrap_err();
        assert!(error.to_string().contains("fabric unavailable"));
        assert_eq!(transport.commission_requests().len(), 2);
    }

    #[test]
    fn spy_transport_records_group_operations_and_failures() {
        let transport = SpyTransport::new();
        transport.add_device(1, "Vendor", "Ceiling");
        transport.add_device(2, "Vendor", "Sconce");
        let group = matter_group(17, &[(1, 1), (2, 3)]);

        transport.configure_group(&group).unwrap();
        transport.set_group_on_off(17, true).unwrap();
        assert!(transport.read_on_off(1, 1).unwrap());
        assert!(transport.read_on_off(2, 3).unwrap());

        transport.set_group_brightness(17, 0, Some(500)).unwrap();
        assert!(!transport.read_on_off(1, 1).unwrap());
        assert!(!transport.read_on_off(2, 3).unwrap());

        transport.identify_group(17, 11).unwrap();
        transport
            .set_group_color_temperature(17, 4200, None)
            .unwrap();
        transport.set_group_xy(17, 0.2, 0.4, Some(75)).unwrap();
        transport
            .set_group_hue_saturation(17, 15, 220, Some(90))
            .unwrap();
        transport.remove_group(17, &group.members).unwrap();
        transport.set_group_on_off(17, true).unwrap();
        assert!(!transport.read_on_off(1, 1).unwrap());

        let operations = transport.operations();
        assert!(operations.contains(&RecordedOperation::ConfigureGroup {
            group: group.clone()
        }));
        assert!(operations.contains(&RecordedOperation::SetGroupOnOff {
            group_id: 17,
            on: true
        }));
        assert!(operations.contains(&RecordedOperation::SetGroupBrightness {
            group_id: 17,
            level: 0,
            transition_ms: Some(500)
        }));
        assert!(operations.contains(&RecordedOperation::IdentifyGroup {
            group_id: 17,
            duration_secs: 11
        }));
        assert!(
            operations.contains(&RecordedOperation::SetGroupColorTemperature {
                group_id: 17,
                kelvin: 4200,
                transition_ms: None
            })
        );
        assert!(operations.contains(&RecordedOperation::SetGroupXy {
            group_id: 17,
            x: 0.2,
            y: 0.4,
            transition_ms: Some(75)
        }));
        assert!(
            operations.contains(&RecordedOperation::SetGroupHueSaturation {
                group_id: 17,
                hue: 15,
                saturation: 220,
                transition_ms: Some(90)
            })
        );
        assert!(operations.contains(&RecordedOperation::RemoveGroup {
            group_id: 17,
            members: group.members.clone()
        }));

        transport.fail_node(3);
        let bad_group = matter_group(18, &[(3, 1)]);
        assert!(transport.configure_group(&bad_group).is_err());

        transport.fail_group_commands(19);
        assert!(transport.set_group_on_off(19, false).is_err());
        assert!(transport.identify_group(19, 1).is_err());
        assert!(transport.set_group_brightness(19, 200, None).is_err());
        assert!(transport
            .set_group_color_temperature(19, 2700, Some(10))
            .is_err());
        assert!(transport.set_group_xy(19, 0.1, 0.2, None).is_err());
        assert!(transport
            .set_group_hue_saturation(19, 9, 10, Some(20))
            .is_err());
    }

    #[test]
    fn spy_transport_records_light_operations_and_read_failures() {
        let transport = SpyTransport::default();
        transport.add_device(5, "Vendor", "Lamp");
        let devices = transport.list_devices().unwrap();
        assert_eq!(devices[0].node_id, 5);
        assert_eq!(devices[0].product_name, "Lamp");

        transport.set_on_off_state(5, true);
        assert!(transport.read_on_off(5, 1).unwrap());
        transport.set_on_off(5, 1, false).unwrap();
        assert!(!transport.read_on_off(5, 1).unwrap());

        transport.identify_light(5, 1, 6).unwrap();
        transport.set_brightness(5, 1, 254, Some(100)).unwrap();
        assert!(transport.read_on_off(5, 1).unwrap());
        transport.set_brightness(5, 1, 0, None).unwrap();
        assert!(!transport.read_on_off(5, 1).unwrap());
        transport
            .set_color_temperature(5, 1, 3300, Some(25))
            .unwrap();
        transport.set_xy(5, 1, 0.45, 0.32, None).unwrap();
        transport
            .set_hue_saturation(5, 1, 42, 210, Some(30))
            .unwrap();
        assert!(!transport.read_on_off(8, 1).unwrap());

        let operations = transport.operations();
        assert!(operations.contains(&RecordedOperation::SetOnOff {
            node_id: 5,
            endpoint: 1,
            on: false
        }));
        assert!(operations.contains(&RecordedOperation::IdentifyLight {
            node_id: 5,
            endpoint: 1,
            duration_secs: 6
        }));
        assert!(operations.contains(&RecordedOperation::SetBrightness {
            node_id: 5,
            endpoint: 1,
            level: 254,
            transition_ms: Some(100)
        }));
        assert!(
            operations.contains(&RecordedOperation::SetColorTemperature {
                node_id: 5,
                endpoint: 1,
                kelvin: 3300,
                transition_ms: Some(25)
            })
        );
        assert!(operations.contains(&RecordedOperation::SetXy {
            node_id: 5,
            endpoint: 1,
            x: 0.45,
            y: 0.32,
            transition_ms: None
        }));
        assert!(operations.contains(&RecordedOperation::SetHueSaturation {
            node_id: 5,
            endpoint: 1,
            hue: 42,
            saturation: 210,
            transition_ms: Some(30)
        }));

        transport.fail_node(6);
        assert!(transport.set_on_off(6, 1, true).is_err());
        assert!(transport.identify_light(6, 1, 1).is_err());
        assert!(transport.set_brightness(6, 1, 100, None).is_err());
        assert!(transport.set_color_temperature(6, 1, 4000, None).is_err());
        assert!(transport.set_xy(6, 1, 0.2, 0.3, None).is_err());
        assert!(transport.set_hue_saturation(6, 1, 1, 2, None).is_err());
        assert!(transport.read_on_off(6, 1).is_err());

        transport.set_on_off_state(7, true);
        transport.fail_read_node(7);
        assert!(transport.read_on_off(7, 1).is_err());
    }
}
