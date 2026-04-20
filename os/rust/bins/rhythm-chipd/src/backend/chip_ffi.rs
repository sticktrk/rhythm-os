use anyhow::Result;

use rhythm_matter::transport::{CommissionedDevice, MatterCommissionRequest};

use crate::service::CommissioningState;

pub struct ChipFfiController {
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
        #[cfg(rhythm_chipd_chip_ffi)]
        ffi_probe::initialize_bridge(&state.storage_path, &state.fabric_id, ble_controller)?;

        Ok(Self {
            #[cfg(not(rhythm_chipd_chip_ffi))]
            mode: ChipBridgeMode::stub(),
        })
    }

    pub fn commission_light(
        &mut self,
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

    pub fn probe_light(&mut self, node_id: u64) -> Result<CommissionedDevice> {
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

    pub fn decommission_device(&mut self, node_id: u64, force: bool) -> Result<()> {
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

    pub fn set_on_off(&mut self, node_id: u64, endpoint: u16, on: bool) -> Result<()> {
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

    pub fn set_brightness(
        &mut self,
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

    pub fn set_color_temperature(
        &mut self,
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
        &mut self,
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
        &mut self,
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

    pub fn read_on_off(&mut self, node_id: u64, endpoint: u16) -> Result<bool> {
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
        CommissionedDevice, MatterColorMode, MatterCommissionRequest, MatterCommissioningRendezvous,
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

    unsafe extern "C" {
        fn rhythm_chip_bridge_init(
            storage_path: *const c_char,
            fabric_id: *const c_char,
            has_ble_controller: bool,
            ble_controller: c_ushort,
            controller_vendor_id: c_ushort,
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
        fn rhythm_chip_bridge_set_brightness(
            node_id: u64,
            endpoint: c_ushort,
            level: u8,
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
        fn rhythm_chip_bridge_shutdown();
    }

    pub fn initialize_bridge(
        storage_path: &Path,
        fabric_id: &str,
        ble_controller: Option<u16>,
    ) -> Result<()> {
        let storage_path = CString::new(storage_path.display().to_string())
            .context("encoding CHIP storage path")?;
        let fabric_id = CString::new(fabric_id).context("encoding CHIP fabric id")?;
        let controller_vendor_id = controller_vendor_id()?;
        let mut error_buffer = [0 as c_char; ERROR_BUFFER_SIZE];

        let success = unsafe {
            rhythm_chip_bridge_init(
                storage_path.as_ptr(),
                fabric_id.as_ptr(),
                ble_controller.is_some(),
                ble_controller.unwrap_or_default(),
                controller_vendor_id,
                error_buffer.as_mut_ptr(),
                error_buffer.len(),
            )
        };

        if success {
            return Ok(());
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
