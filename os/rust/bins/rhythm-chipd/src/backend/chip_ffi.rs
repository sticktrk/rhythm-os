use anyhow::Result;

use rhythm_matter::transport::{
    CommissionedDevice, MatterAttributeReport, MatterCommissionRequest, MatterGroup,
    MatterGroupMember, MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
};

use crate::service::CommissioningState;

pub struct ChipFfiController {
    compressed_fabric_id: Option<String>,
    #[cfg(not(rhythm_chipd_chip_ffi))]
    mode: ChipBridgeMode,
}

impl ChipFfiController {
    pub fn initialize(
        state: &CommissioningState,
        ble_controller: Option<u16>,
        _existing_devices: &[CommissionedDevice],
    ) -> Result<Self> {
        #[cfg(not(rhythm_chipd_chip_ffi))]
        let _ = (state, ble_controller);
        #[cfg(not(rhythm_chipd_chip_ffi))]
        let compressed_fabric_id = None;
        #[cfg(rhythm_chipd_chip_ffi)]
        let compressed_fabric_id = Some(ffi_probe::initialize_bridge(
            &state.storage_path,
            &state.fabric_id,
            state.operational_fabric_id,
            &state.ipk_hex,
            ble_controller,
        )?);

        Ok(Self {
            compressed_fabric_id,
            #[cfg(not(rhythm_chipd_chip_ffi))]
            mode: ChipBridgeMode::stub(),
        })
    }

    pub fn compressed_fabric_id(&self) -> Option<&str> {
        self.compressed_fabric_id.as_deref()
    }

    pub fn commission_light(
        &self,
        request: &MatterCommissionRequest,
    ) -> Result<CommissionedDevice> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::commission_light(request)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = request;
            Err(self.unsupported("commission_light"))
        }
    }

    pub fn probe_light(&self, node_id: u64) -> Result<CommissionedDevice> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::probe_light(node_id)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = node_id;
            Err(self.unsupported("probe_light"))
        }
    }

    pub fn decommission_device(&self, node_id: u64, force: bool) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::decommission_device(node_id, force)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, force);
            Err(self.unsupported("decommission_device"))
        }
    }

    pub fn set_on_off(&self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_on_off(node_id, endpoint, on)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, on);
            Err(self.unsupported("set_on_off"))
        }
    }

    pub fn configure_group(&self, group: &MatterGroup) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::configure_group(group)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = group;
            Err(self.unsupported("configure_group"))
        }
    }

    pub fn remove_group(&self, group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::remove_group(group_id, members)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, members);
            Err(self.unsupported("remove_group"))
        }
    }

    pub fn set_group_on_off(&self, group_id: u16, on: bool) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_group_on_off(group_id, on)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, on);
            Err(self.unsupported("set_group_on_off"))
        }
    }

    pub fn identify_group(&self, group_id: u16, duration_secs: u16) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::identify_group(group_id, duration_secs)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, duration_secs);
            Err(self.unsupported("identify_group"))
        }
    }

    pub fn set_group_brightness(
        &self,
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_group_brightness(group_id, level, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, level, transition_ms);
            Err(self.unsupported("set_group_brightness"))
        }
    }

    pub fn set_group_color_temperature(
        &self,
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_group_color_temperature(group_id, kelvin, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, kelvin, transition_ms);
            Err(self.unsupported("set_group_color_temperature"))
        }
    }

    pub fn set_group_xy(
        &self,
        group_id: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_group_xy(group_id, x, y, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, x, y, transition_ms);
            Err(self.unsupported("set_group_xy"))
        }
    }

    pub fn set_group_hue_saturation(
        &self,
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_group_hue_saturation(group_id, hue, saturation, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (group_id, hue, saturation, transition_ms);
            Err(self.unsupported("set_group_hue_saturation"))
        }
    }

    pub fn identify_light(&self, node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::identify_light(node_id, endpoint, duration_secs)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, duration_secs);
            Err(self.unsupported("identify_light"))
        }
    }

    pub fn set_brightness(
        &self,
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_brightness(node_id, endpoint, level, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, level, transition_ms);
            Err(self.unsupported("set_brightness"))
        }
    }

    pub fn run_level_command(
        &self,
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::run_level_command(
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            )
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (
                node_id,
                endpoint,
                command,
                level_or_step,
                step_mode,
                transition_ms,
            );
            Err(self.unsupported("run_level_command"))
        }
    }

    pub fn set_color_temperature(
        &self,
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_color_temperature(node_id, endpoint, kelvin, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, kelvin, transition_ms);
            Err(self.unsupported("set_color_temperature"))
        }
    }

    pub fn set_xy(
        &self,
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_xy(node_id, endpoint, x, y, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, x, y, transition_ms);
            Err(self.unsupported("set_xy"))
        }
    }

    pub fn set_hue_saturation(
        &self,
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::set_hue_saturation(node_id, endpoint, hue, saturation, transition_ms)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint, hue, saturation, transition_ms);
            Err(self.unsupported("set_hue_saturation"))
        }
    }

    pub fn read_on_off(&self, node_id: u64, endpoint: u16) -> Result<bool> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::read_on_off(node_id, endpoint)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint);
            Err(self.unsupported("read_on_off"))
        }
    }

    pub fn read_light_capability_snapshot(
        &self,
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::read_light_capability_snapshot(node_id, endpoint)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint);
            Err(self.unsupported("read_light_capability_snapshot"))
        }
    }

    pub fn read_light_state(&self, node_id: u64, endpoint: u16) -> Result<serde_json::Value> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::read_light_state(node_id, endpoint)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (node_id, endpoint);
            Err(self.unsupported("read_light_state"))
        }
    }

    pub fn subscribe_on_off(
        &self,
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::subscribe_on_off(targets, min_interval_secs, max_interval_secs)
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            let _ = (targets, min_interval_secs, max_interval_secs);
            Err(self.unsupported("subscribe_on_off"))
        }
    }

    pub fn drain_attribute_reports(&self) -> Result<Vec<MatterAttributeReport>> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            ffi_probe::drain_attribute_reports()
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            Err(self.unsupported("drain_attribute_reports"))
        }
    }

    #[cfg(not(rhythm_chipd_chip_ffi))]
    fn unsupported(&self, operation: &str) -> anyhow::Error {
        match &self.mode {
            ChipBridgeMode::Stub { reason } => anyhow::anyhow!(
                "Direct CHIP bridge operation '{}' is unavailable: {}",
                operation,
                reason
            ),
        }
    }
}

#[cfg(not(rhythm_chipd_chip_ffi))]
enum ChipBridgeMode {
    Stub { reason: String },
}

#[cfg(not(rhythm_chipd_chip_ffi))]
impl ChipBridgeMode {
    fn stub() -> Self {
        Self::Stub {
            reason: "rhythm-chipd was built without the optional direct CHIP FFI bridge. Build with `--features chip-ffi` and set `RHYTHM_CHIP_OUT_DIR` or `RHYTHM_CHIP_LIB_DIR` to target-specific connectedhomeip artifacts.".to_string(),
        }
    }
}

#[cfg(all(test, not(rhythm_chipd_chip_ffi)))]
mod tests {
    use super::*;
    use rhythm_matter::transport::{
        MatterCommissioningNetwork, MatterCommissioningRendezvous,
        MatterCommissioningWifiCredentials,
    };

    fn commissioning_state() -> CommissioningState {
        CommissioningState {
            fabric_id: "fabric".to_string(),
            operational_fabric_id: 123,
            ipk_hex: "00112233445566778899aabbccddeeff".to_string(),
            storage_path: std::env::temp_dir().join("rhythm-chip-ffi-test.ini"),
        }
    }

    fn commission_request() -> MatterCommissionRequest {
        MatterCommissionRequest {
            setup_payload: "MT:TEST".to_string(),
            node_id: 1,
            network: MatterCommissioningNetwork::Wifi,
            rendezvous: MatterCommissioningRendezvous::Auto,
            wifi_credentials: MatterCommissioningWifiCredentials {
                ssid: "Rhythm".to_string(),
                password: "secret".to_string(),
            },
        }
    }

    fn group() -> MatterGroup {
        MatterGroup {
            group_id: 55,
            name: "Kitchen".to_string(),
            members: vec![MatterGroupMember {
                node_id: 1,
                endpoint: 1,
            }],
        }
    }

    fn unsupported<T>(result: Result<T>, operation: &str) {
        let err = match result {
            Ok(_) => panic!("stub operation should be unsupported"),
            Err(err) => err,
        };
        let message = err.to_string();
        assert!(
            message.contains(operation),
            "expected operation name {operation:?} in {message:?}"
        );
        assert!(
            message.contains("built without the optional direct CHIP FFI bridge"),
            "expected stub reason in {message:?}"
        );
    }

    #[test]
    fn stub_controller_initializes_and_reports_unsupported_operations() {
        let state = commissioning_state();
        let controller = ChipFfiController::initialize(&state, Some(0), &[]).unwrap();
        let request = commission_request();
        let group = group();
        let members = group.members.clone();
        let targets = vec![MatterSubscriptionTarget {
            node_id: 1,
            endpoint: 1,
        }];

        unsupported(controller.commission_light(&request), "commission_light");
        unsupported(controller.probe_light(1), "probe_light");
        unsupported(
            controller.decommission_device(1, false),
            "decommission_device",
        );
        unsupported(controller.set_on_off(1, 1, true), "set_on_off");
        unsupported(controller.configure_group(&group), "configure_group");
        unsupported(controller.remove_group(55, &members), "remove_group");
        unsupported(controller.set_group_on_off(55, true), "set_group_on_off");
        unsupported(controller.identify_group(55, 2), "identify_group");
        unsupported(
            controller.set_group_brightness(55, 128, Some(250)),
            "set_group_brightness",
        );
        unsupported(
            controller.set_group_color_temperature(55, 2700, Some(250)),
            "set_group_color_temperature",
        );
        unsupported(controller.set_group_xy(55, 0.3, 0.4, None), "set_group_xy");
        unsupported(
            controller.set_group_hue_saturation(55, 12, 34, None),
            "set_group_hue_saturation",
        );
        unsupported(controller.identify_light(1, 1, 2), "identify_light");
        unsupported(
            controller.set_brightness(1, 1, 128, Some(250)),
            "set_brightness",
        );
        unsupported(
            controller.run_level_command(
                1,
                1,
                MatterLevelCommandVariant::StepWithOnOff,
                10,
                Some(MatterLevelStepMode::Up),
                Some(250),
            ),
            "run_level_command",
        );
        unsupported(
            controller.set_color_temperature(1, 1, 2700, None),
            "set_color_temperature",
        );
        unsupported(controller.set_xy(1, 1, 0.3, 0.4, None), "set_xy");
        unsupported(
            controller.set_hue_saturation(1, 1, 12, 34, None),
            "set_hue_saturation",
        );
        unsupported(controller.read_on_off(1, 1), "read_on_off");
        unsupported(
            controller.read_light_capability_snapshot(1, 1),
            "read_light_capability_snapshot",
        );
        unsupported(controller.read_light_state(1, 1), "read_light_state");
        unsupported(
            controller.subscribe_on_off(&targets, 1, 30),
            "subscribe_on_off",
        );
        unsupported(
            controller.drain_attribute_reports(),
            "drain_attribute_reports",
        );
    }
}

#[cfg(rhythm_chipd_chip_ffi)]
impl Drop for ChipFfiController {
    fn drop(&mut self) {
        ffi_probe::shutdown_bridge();
    }
}

#[cfg(rhythm_chipd_chip_ffi)]
mod ffi_probe {
    use std::env;
    use std::ffi::{c_char, c_uchar, c_ushort, CStr, CString};
    use std::path::Path;

    use anyhow::{Context, Result};

    use rhythm_matter::transport::{
        CommissionedDevice, MatterAttributeReport, MatterAttributeValue, MatterColorMode,
        MatterCommissionRequest, MatterCommissioningRendezvous, MatterGroup, MatterGroupMember,
        MatterLevelCommandVariant, MatterLevelStepMode, MatterSubscriptionTarget,
    };

    const ERROR_BUFFER_SIZE: usize = 512;
    const STRING_CAPACITY: usize = 128;

    const RENDEZVOUS_AUTO: u8 = 0;
    const RENDEZVOUS_BLE: u8 = 1;
    const RENDEZVOUS_ON_NETWORK: u8 = 2;
    const DEFAULT_CONTROLLER_VENDOR_ID: u16 = 0xFFF1;

    const COLOR_MODE_HUE_SATURATION: u32 = 1 << 0;
    const COLOR_MODE_XY: u32 = 1 << 1;
    const COLOR_MODE_COLOR_TEMPERATURE: u32 = 1 << 2;
    const ATTRIBUTE_VALUE_BOOL: u8 = 1;
    const MAX_DRAINED_REPORTS: usize = 128;
    const JSON_BUFFER_SIZE: usize = 64 * 1024;
    const LEVEL_COMMAND_MOVE_TO_LEVEL: u8 = 0;
    const LEVEL_COMMAND_MOVE_TO_LEVEL_WITH_ON_OFF: u8 = 1;
    const LEVEL_COMMAND_STEP: u8 = 2;
    const LEVEL_COMMAND_STEP_WITH_ON_OFF: u8 = 3;
    const LEVEL_STEP_MODE_UP: u8 = 0;
    const LEVEL_STEP_MODE_DOWN: u8 = 1;

    #[repr(C)]
    struct ChipBridgeCommissionRequest {
        setup_payload: *const c_char,
        node_id: u64,
        rendezvous_mode: c_uchar,
        wifi_ssid: *const c_char,
        wifi_password: *const c_char,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ChipBridgeDevice {
        node_id: u64,
        vendor_id: c_ushort,
        product_id: c_ushort,
        light_endpoint: c_ushort,
        color_mode_flags: u32,
        min_kelvin: c_ushort,
        max_kelvin: c_ushort,
        has_serial_number: c_uchar,
        has_min_kelvin: c_uchar,
        has_max_kelvin: c_uchar,
        vendor_name: [c_char; STRING_CAPACITY],
        product_name: [c_char; STRING_CAPACITY],
        serial_number: [c_char; STRING_CAPACITY],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ChipBridgeSubscriptionTarget {
        node_id: u64,
        endpoint: c_ushort,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ChipBridgeGroupMember {
        node_id: u64,
        endpoint: c_ushort,
    }

    #[repr(C)]
    struct ChipBridgeGroup {
        group_id: c_ushort,
        name: *const c_char,
        members: *const ChipBridgeGroupMember,
        member_count: usize,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct ChipBridgeAttributeReport {
        node_id: u64,
        endpoint: c_ushort,
        cluster_id: u32,
        attribute_id: u32,
        value_type: c_uchar,
        bool_value: bool,
    }

    unsafe extern "C" {
        fn rhythm_chip_bridge_init(
            storage_path: *const c_char,
            fabric_id: *const c_char,
            operational_fabric_id: u64,
            ipk_hex: *const c_char,
            has_ble_controller: bool,
            ble_controller: c_ushort,
            controller_vendor_id: c_ushort,
            out_compressed_fabric_id: *mut u64,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_commission_light(
            request: *const ChipBridgeCommissionRequest,
            device: *mut ChipBridgeDevice,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_probe_light(
            node_id: u64,
            device: *mut ChipBridgeDevice,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_decommission_device(
            node_id: u64,
            force: bool,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_on_off(
            node_id: u64,
            endpoint: c_ushort,
            on: bool,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_configure_group(
            group: *const ChipBridgeGroup,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_remove_group(
            group_id: c_ushort,
            members: *const ChipBridgeGroupMember,
            member_count: usize,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_group_on_off(
            group_id: c_ushort,
            on: bool,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_identify_group(
            group_id: c_ushort,
            duration_secs: c_ushort,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_group_brightness(
            group_id: c_ushort,
            level: u8,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_group_color_temperature(
            group_id: c_ushort,
            kelvin: c_ushort,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_group_xy(
            group_id: c_ushort,
            x: f32,
            y: f32,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_group_hue_saturation(
            group_id: c_ushort,
            hue: u8,
            saturation: u8,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_identify_light(
            node_id: u64,
            endpoint: c_ushort,
            duration_secs: c_ushort,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_brightness(
            node_id: u64,
            endpoint: c_ushort,
            level: u8,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_run_level_command(
            node_id: u64,
            endpoint: c_ushort,
            command: c_uchar,
            level_or_step: u8,
            step_mode: c_uchar,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_color_temperature(
            node_id: u64,
            endpoint: c_ushort,
            kelvin: c_ushort,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_xy(
            node_id: u64,
            endpoint: c_ushort,
            x: f32,
            y: f32,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_set_hue_saturation(
            node_id: u64,
            endpoint: c_ushort,
            hue: u8,
            saturation: u8,
            has_transition_ms: bool,
            transition_ms: u32,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_read_on_off(
            node_id: u64,
            endpoint: c_ushort,
            out_on: *mut bool,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_read_light_capability_snapshot(
            node_id: u64,
            endpoint: c_ushort,
            out_json: *mut c_char,
            json_size: usize,
            out_json_len: *mut usize,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_read_light_state(
            node_id: u64,
            endpoint: c_ushort,
            out_json: *mut c_char,
            json_size: usize,
            out_json_len: *mut usize,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_subscribe_on_off(
            targets: *const ChipBridgeSubscriptionTarget,
            target_count: usize,
            min_interval_secs: c_ushort,
            max_interval_secs: c_ushort,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_drain_attribute_reports(
            reports: *mut ChipBridgeAttributeReport,
            reports_capacity: usize,
            out_report_count: *mut usize,
            error_message: *mut c_char,
            error_message_size: usize,
        ) -> bool;
        fn rhythm_chip_bridge_shutdown();
    }

    pub fn initialize_bridge(
        storage_path: &Path,
        fabric_id: &str,
        operational_fabric_id: u64,
        ipk_hex: &str,
        ble_controller: Option<u16>,
    ) -> Result<String> {
        let storage_path = CString::new(storage_path.display().to_string())
            .context("encoding CHIP storage path")?;
        let fabric_id = CString::new(fabric_id).context("encoding CHIP fabric id")?;
        let ipk_hex = CString::new(ipk_hex).context("encoding CHIP IPK")?;
        let controller_vendor_id = controller_vendor_id()?;
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let mut compressed_fabric_id = 0u64;

        let success = unsafe {
            rhythm_chip_bridge_init(
                storage_path.as_ptr(),
                fabric_id.as_ptr(),
                operational_fabric_id,
                ipk_hex.as_ptr(),
                ble_controller.is_some(),
                ble_controller.unwrap_or_default(),
                controller_vendor_id,
                &mut compressed_fabric_id,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };

        if success {
            return Ok(format!("{compressed_fabric_id:016X}"));
        }

        Err(read_error_buffer(&error_buffer))
    }

    pub fn shutdown_bridge() {
        unsafe { rhythm_chip_bridge_shutdown() };
    }

    pub fn commission_light(request: &MatterCommissionRequest) -> Result<CommissionedDevice> {
        let setup_payload = CString::new(request.setup_payload.as_str())
            .context("encoding Matter setup payload")?;
        let wifi_ssid = CString::new(request.wifi_credentials.ssid.as_str())
            .context("encoding Matter commissioning SSID")?;
        let wifi_password = CString::new(request.wifi_credentials.password.as_str())
            .context("encoding Matter commissioning password")?;

        let ffi_request = ChipBridgeCommissionRequest {
            setup_payload: setup_payload.as_ptr(),
            node_id: request.node_id,
            rendezvous_mode: map_rendezvous_mode(request.rendezvous),
            wifi_ssid: wifi_ssid.as_ptr(),
            wifi_password: wifi_password.as_ptr(),
        };
        let mut ffi_device = zeroed_device();
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];

        let success = unsafe {
            rhythm_chip_bridge_commission_light(
                &ffi_request,
                &mut ffi_device,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };

        if !success {
            return Err(read_error_buffer(&error_buffer));
        }

        decode_device(ffi_device)
    }

    fn controller_vendor_id() -> Result<u16> {
        match env::var("RHYTHM_MATTER_CONTROLLER_VENDOR_ID") {
            Ok(raw) => parse_vendor_id(&raw).with_context(|| {
                format!(
                    "parsing RHYTHM_MATTER_CONTROLLER_VENDOR_ID='{}' as a Matter vendor id",
                    raw
                )
            }),
            Err(env::VarError::NotPresent) => Ok(DEFAULT_CONTROLLER_VENDOR_ID),
            Err(error) => {
                Err(anyhow::anyhow!(error)).context("reading RHYTHM_MATTER_CONTROLLER_VENDOR_ID")
            }
        }
    }

    fn parse_vendor_id(raw: &str) -> Result<u16> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            anyhow::bail!("vendor id cannot be empty");
        }

        if let Some(hex) = trimmed
            .strip_prefix("0x")
            .or_else(|| trimmed.strip_prefix("0X"))
        {
            return Ok(u16::from_str_radix(hex, 16)?);
        }

        Ok(trimmed.parse::<u16>()?)
    }

    pub fn probe_light(node_id: u64) -> Result<CommissionedDevice> {
        let mut ffi_device = zeroed_device();
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];

        let success = unsafe {
            rhythm_chip_bridge_probe_light(
                node_id,
                &mut ffi_device,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };

        if !success {
            return Err(read_error_buffer(&error_buffer));
        }

        decode_device(ffi_device)
    }

    pub fn decommission_device(node_id: u64, force: bool) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_decommission_device(
                node_id,
                force,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_on_off(node_id: u64, endpoint: u16, on: bool) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_on_off(
                node_id,
                endpoint,
                on,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn configure_group(group: &MatterGroup) -> Result<()> {
        let name = CString::new(group.name.as_str()).context("encoding Matter group name")?;
        let members = ffi_group_members(&group.members);
        let ffi_group = ChipBridgeGroup {
            group_id: group.group_id,
            name: name.as_ptr(),
            members: members.as_ptr(),
            member_count: members.len(),
        };
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_configure_group(
                &ffi_group,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn remove_group(group_id: u16, members: &[MatterGroupMember]) -> Result<()> {
        let members = ffi_group_members(members);
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_remove_group(
                group_id,
                members.as_ptr(),
                members.len(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_group_on_off(group_id: u16, on: bool) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_group_on_off(
                group_id,
                on,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn identify_group(group_id: u16, duration_secs: u16) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_identify_group(
                group_id,
                duration_secs,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_group_brightness(
        group_id: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_group_brightness(
                group_id,
                level,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_group_color_temperature(
        group_id: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_group_color_temperature(
                group_id,
                kelvin,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_group_xy(group_id: u16, x: f32, y: f32, transition_ms: Option<u32>) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_group_xy(
                group_id,
                x,
                y,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_group_hue_saturation(
        group_id: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_group_hue_saturation(
                group_id,
                hue,
                saturation,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn identify_light(node_id: u64, endpoint: u16, duration_secs: u16) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_identify_light(
                node_id,
                endpoint,
                duration_secs,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    fn ffi_group_members(members: &[MatterGroupMember]) -> Vec<ChipBridgeGroupMember> {
        members
            .iter()
            .map(|member| ChipBridgeGroupMember {
                node_id: member.node_id,
                endpoint: member.endpoint,
            })
            .collect()
    }

    pub fn set_brightness(
        node_id: u64,
        endpoint: u16,
        level: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_brightness(
                node_id,
                endpoint,
                level,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn run_level_command(
        node_id: u64,
        endpoint: u16,
        command: MatterLevelCommandVariant,
        level_or_step: u8,
        step_mode: Option<MatterLevelStepMode>,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_run_level_command(
                node_id,
                endpoint,
                map_level_command(command),
                level_or_step,
                map_step_mode(step_mode.unwrap_or(MatterLevelStepMode::Up)),
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_color_temperature(
        node_id: u64,
        endpoint: u16,
        kelvin: u16,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_color_temperature(
                node_id,
                endpoint,
                kelvin,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_xy(
        node_id: u64,
        endpoint: u16,
        x: f32,
        y: f32,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_xy(
                node_id,
                endpoint,
                x,
                y,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn set_hue_saturation(
        node_id: u64,
        endpoint: u16,
        hue: u8,
        saturation: u8,
        transition_ms: Option<u32>,
    ) -> Result<()> {
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_set_hue_saturation(
                node_id,
                endpoint,
                hue,
                saturation,
                transition_ms.is_some(),
                transition_ms.unwrap_or_default(),
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn read_on_off(node_id: u64, endpoint: u16) -> Result<bool> {
        let mut on = false;
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_read_on_off(
                node_id,
                endpoint,
                &mut on,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(on)
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn read_light_capability_snapshot(
        node_id: u64,
        endpoint: u16,
    ) -> Result<serde_json::Value> {
        read_json_value(|json_buffer, json_len, error_buffer| unsafe {
            rhythm_chip_bridge_read_light_capability_snapshot(
                node_id,
                endpoint,
                json_buffer.as_mut_ptr(),
                json_buffer.len(),
                json_len,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        })
    }

    pub fn read_light_state(node_id: u64, endpoint: u16) -> Result<serde_json::Value> {
        read_json_value(|json_buffer, json_len, error_buffer| unsafe {
            rhythm_chip_bridge_read_light_state(
                node_id,
                endpoint,
                json_buffer.as_mut_ptr(),
                json_buffer.len(),
                json_len,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        })
    }

    pub fn subscribe_on_off(
        targets: &[MatterSubscriptionTarget],
        min_interval_secs: u16,
        max_interval_secs: u16,
    ) -> Result<()> {
        let ffi_targets = targets
            .iter()
            .map(|target| ChipBridgeSubscriptionTarget {
                node_id: target.node_id,
                endpoint: target.endpoint,
            })
            .collect::<Vec<_>>();
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_subscribe_on_off(
                ffi_targets.as_ptr(),
                ffi_targets.len(),
                min_interval_secs,
                max_interval_secs,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if success {
            Ok(())
        } else {
            Err(read_error_buffer(&error_buffer))
        }
    }

    pub fn drain_attribute_reports() -> Result<Vec<MatterAttributeReport>> {
        let empty_report = ChipBridgeAttributeReport {
            node_id: 0,
            endpoint: 0,
            cluster_id: 0,
            attribute_id: 0,
            value_type: 0,
            bool_value: false,
        };
        let mut reports = vec![empty_report; MAX_DRAINED_REPORTS];
        let mut report_count = 0usize;
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = unsafe {
            rhythm_chip_bridge_drain_attribute_reports(
                reports.as_mut_ptr(),
                reports.len(),
                &mut report_count,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };
        if !success {
            return Err(read_error_buffer(&error_buffer));
        }

        reports
            .into_iter()
            .take(report_count)
            .map(decode_attribute_report)
            .collect()
    }

    fn zeroed_device() -> ChipBridgeDevice {
        ChipBridgeDevice {
            node_id: 0,
            vendor_id: 0,
            product_id: 0,
            light_endpoint: 0,
            color_mode_flags: 0,
            min_kelvin: 0,
            max_kelvin: 0,
            has_serial_number: 0,
            has_min_kelvin: 0,
            has_max_kelvin: 0,
            vendor_name: [0; STRING_CAPACITY],
            product_name: [0; STRING_CAPACITY],
            serial_number: [0; STRING_CAPACITY],
        }
    }

    fn read_error_buffer(buffer: &[c_char; ERROR_BUFFER_SIZE]) -> anyhow::Error {
        let message = unsafe { CStr::from_ptr(buffer.as_ptr()) }
            .to_str()
            .unwrap_or("unknown CHIP bridge error")
            .to_string();
        anyhow::anyhow!(if message.is_empty() {
            "unknown CHIP bridge error".to_string()
        } else {
            message
        })
    }

    fn map_rendezvous_mode(rendezvous: MatterCommissioningRendezvous) -> c_uchar {
        match rendezvous {
            MatterCommissioningRendezvous::Auto => RENDEZVOUS_AUTO,
            MatterCommissioningRendezvous::Ble => RENDEZVOUS_BLE,
            MatterCommissioningRendezvous::OnNetwork => RENDEZVOUS_ON_NETWORK,
        }
    }

    fn map_level_command(command: MatterLevelCommandVariant) -> c_uchar {
        match command {
            MatterLevelCommandVariant::MoveToLevel => LEVEL_COMMAND_MOVE_TO_LEVEL,
            MatterLevelCommandVariant::MoveToLevelWithOnOff => {
                LEVEL_COMMAND_MOVE_TO_LEVEL_WITH_ON_OFF
            }
            MatterLevelCommandVariant::Step => LEVEL_COMMAND_STEP,
            MatterLevelCommandVariant::StepWithOnOff => LEVEL_COMMAND_STEP_WITH_ON_OFF,
        }
    }

    fn map_step_mode(mode: MatterLevelStepMode) -> c_uchar {
        match mode {
            MatterLevelStepMode::Up => LEVEL_STEP_MODE_UP,
            MatterLevelStepMode::Down => LEVEL_STEP_MODE_DOWN,
        }
    }

    fn read_json_value<F>(f: F) -> Result<serde_json::Value>
    where
        F: FnOnce(
            &mut [c_char; JSON_BUFFER_SIZE],
            *mut usize,
            &mut [c_char; ERROR_BUFFER_SIZE],
        ) -> bool,
    {
        let mut json_buffer = [0 as c_char; JSON_BUFFER_SIZE];
        let mut json_len = 0usize;
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];
        let success = f(&mut json_buffer, &mut json_len, &mut error_buffer);
        if !success {
            return Err(read_error_buffer(&error_buffer));
        }

        let bytes = json_buffer[..json_len]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<_>>();
        serde_json::from_slice(&bytes).context("decoding CHIP JSON response")
    }

    fn decode_device(device: ChipBridgeDevice) -> Result<CommissionedDevice> {
        let mut color_modes = Vec::new();
        if device.color_mode_flags & COLOR_MODE_HUE_SATURATION != 0 {
            color_modes.push(MatterColorMode::HueSaturation);
        }
        if device.color_mode_flags & COLOR_MODE_XY != 0 {
            color_modes.push(MatterColorMode::Xy);
        }
        if device.color_mode_flags & COLOR_MODE_COLOR_TEMPERATURE != 0 {
            color_modes.push(MatterColorMode::ColorTemperature);
        }

        Ok(CommissionedDevice {
            node_id: device.node_id,
            vendor_name: decode_fixed_string(&device.vendor_name)?,
            product_name: decode_fixed_string(&device.product_name)?,
            vendor_id: device.vendor_id,
            product_id: device.product_id,
            serial_number: if device.has_serial_number != 0 {
                Some(decode_fixed_string(&device.serial_number)?)
            } else {
                None
            },
            light_endpoint: device.light_endpoint,
            color_modes,
            min_kelvin: if device.has_min_kelvin != 0 {
                Some(device.min_kelvin)
            } else {
                None
            },
            max_kelvin: if device.has_max_kelvin != 0 {
                Some(device.max_kelvin)
            } else {
                None
            },
        })
    }

    fn decode_attribute_report(report: ChipBridgeAttributeReport) -> Result<MatterAttributeReport> {
        let value = match report.value_type {
            ATTRIBUTE_VALUE_BOOL => MatterAttributeValue::Bool(report.bool_value),
            other => anyhow::bail!("unsupported Matter attribute report value type {}", other),
        };

        Ok(MatterAttributeReport {
            node_id: report.node_id,
            endpoint: report.endpoint,
            cluster: report.cluster_id,
            attr_id: report.attribute_id,
            value,
        })
    }

    fn decode_fixed_string(buffer: &[c_char; STRING_CAPACITY]) -> Result<String> {
        let nul_index = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        let bytes = buffer[..nul_index]
            .iter()
            .map(|byte| *byte as u8)
            .collect::<Vec<u8>>();
        String::from_utf8(bytes).context("decoding CHIP bridge string")
    }
}
