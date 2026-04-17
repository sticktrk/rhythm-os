#[cfg(rhythm_chipd_chip_ffi)]
use std::path::PathBuf;

use anyhow::Result;

use rhythm_matter::transport::{CommissionedDevice, MatterCommissionRequest};

use crate::service::CommissioningState;

pub struct ChipFfiController {
    mode: ChipBridgeMode,
    #[cfg(rhythm_chipd_chip_ffi)]
    storage_path: PathBuf,
}

impl ChipFfiController {
    pub fn initialize(
        _state: &CommissioningState,
        _existing_devices: &[CommissionedDevice],
    ) -> Result<Self> {
        Ok(Self {
            mode: ChipBridgeMode::detect()?,
            #[cfg(rhythm_chipd_chip_ffi)]
            storage_path: _state.storage_path.clone(),
        })
    }

    pub fn commission_light(
        &mut self,
        _request: &MatterCommissionRequest,
    ) -> Result<CommissionedDevice> {
        Err(self.unsupported("commission_light"))
    }

    pub fn probe_light(&mut self, _node_id: u64) -> Result<CommissionedDevice> {
        Err(self.unsupported("probe_light"))
    }

    pub fn decommission_device(&mut self, _node_id: u64, _force: bool) -> Result<()> {
        Err(self.unsupported("decommission_device"))
    }

    pub fn set_on_off(&mut self, _node_id: u64, _endpoint: u16, _on: bool) -> Result<()> {
        Err(self.unsupported("set_on_off"))
    }

    pub fn set_brightness(
        &mut self,
        _node_id: u64,
        _endpoint: u16,
        _level: u8,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Err(self.unsupported("set_brightness"))
    }

    pub fn set_color_temperature(
        &mut self,
        _node_id: u64,
        _endpoint: u16,
        _kelvin: u16,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Err(self.unsupported("set_color_temperature"))
    }

    pub fn set_xy(
        &mut self,
        _node_id: u64,
        _endpoint: u16,
        _x: f32,
        _y: f32,
        _transition_ms: Option<u32>,
    ) -> Result<()> {
        Err(self.unsupported("set_xy"))
    }

    pub fn read_on_off(&mut self, _node_id: u64, _endpoint: u16) -> Result<bool> {
        Err(self.unsupported("read_on_off"))
    }

    fn unsupported(&self, operation: &str) -> anyhow::Error {
        match &self.mode {
            #[cfg(not(rhythm_chipd_chip_ffi))]
            ChipBridgeMode::Stub { reason } => anyhow::anyhow!(
                "Direct CHIP bridge operation '{}' is unavailable: {}",
                operation,
                reason
            ),
            #[cfg(rhythm_chipd_chip_ffi)]
            ChipBridgeMode::Linked { link_mode } => anyhow::anyhow!(
                "Direct CHIP bridge operation '{}' is not implemented yet. Linked mode='{}' storage='{}'",
                operation,
                link_mode,
                self.storage_path.display()
            ),
        }
    }
}

enum ChipBridgeMode {
    #[cfg(not(rhythm_chipd_chip_ffi))]
    Stub { reason: String },
    #[cfg(rhythm_chipd_chip_ffi)]
    Linked { link_mode: String },
}

impl ChipBridgeMode {
    fn detect() -> Result<Self> {
        #[cfg(rhythm_chipd_chip_ffi)]
        {
            return Ok(Self::Linked {
                link_mode: ffi_probe::linked_mode()?,
            });
        }

        #[cfg(not(rhythm_chipd_chip_ffi))]
        {
            Ok(Self::Stub {
                reason: "rhythm-chipd was built without the optional direct CHIP FFI bridge. Build with `--features chip-ffi` and set `RHYTHM_CHIP_OUT_DIR` or `RHYTHM_CHIP_LIB_DIR` to target-specific connectedhomeip artifacts.".to_string(),
            })
        }
    }
}

#[cfg(rhythm_chipd_chip_ffi)]
mod ffi_probe {
    use std::ffi::{c_char, CStr};

    use anyhow::{Context, Result};

    unsafe extern "C" {
        fn rhythm_chip_bridge_link_mode() -> *const c_char;
    }

    pub fn linked_mode() -> Result<String> {
        let raw = unsafe { rhythm_chip_bridge_link_mode() };
        if raw.is_null() {
            anyhow::bail!("bridge probe returned a null mode pointer");
        }

        unsafe { CStr::from_ptr(raw) }
            .to_str()
            .context("decoding bridge mode")
            .map(ToString::to_string)
    }
}
