//! DTOs for Hue behavior tracking.
//!
//! These types are designed for FFI compatibility (no HashMap/HashSet).

use rhythm_hue::{HueBehaviorTracker, HueButton, HueRoom, HueSwitchDevice};

/// DTO for HueBehaviorTracker state.
///
/// Uses Vec instead of HashMap/HashSet for FFI compatibility.
#[derive(Debug, Clone, Default)]
pub struct HueBehaviorTrackerDto {
    /// Behavior ID to device ID mappings as (behavior_id, device_id) tuples.
    pub behavior_mappings: Vec<BehaviorMappingDto>,

    /// Device IDs that have at least one behavior_instance.
    pub configured_device_ids: Vec<String>,
}

/// A single behavior to device mapping.
#[derive(Debug, Clone)]
pub struct BehaviorMappingDto {
    /// The behavior_instance resource ID.
    pub behavior_id: String,

    /// The device resource ID this behavior configures.
    pub device_id: String,
}

impl From<HueBehaviorTracker> for HueBehaviorTrackerDto {
    fn from(tracker: HueBehaviorTracker) -> Self {
        let behavior_mappings = tracker
            .behavior_mappings()
            .into_iter()
            .map(|(behavior_id, device_id)| BehaviorMappingDto {
                behavior_id,
                device_id,
            })
            .collect();

        let configured_device_ids = tracker.configured_device_ids().iter().cloned().collect();

        Self {
            behavior_mappings,
            configured_device_ids,
        }
    }
}

impl From<HueBehaviorTrackerDto> for HueBehaviorTracker {
    fn from(dto: HueBehaviorTrackerDto) -> Self {
        let mut tracker = HueBehaviorTracker::new();
        let mappings: Vec<(String, String)> = dto
            .behavior_mappings
            .into_iter()
            .map(|m| (m.behavior_id, m.device_id))
            .collect();
        tracker.restore_from_mappings(&mappings);
        tracker
    }
}

/// Result of removing a behavior.
#[derive(Debug, Clone)]
pub struct BehaviorRemoveResultDto {
    /// The updated tracker state.
    pub tracker: HueBehaviorTrackerDto,

    /// True if a device was unconfigured (no more behaviors reference it).
    pub device_unconfigured: bool,
}

/// DTO for a Hue switch device.
#[derive(Debug, Clone)]
pub struct HueSwitchDeviceDto {
    /// V2 resource ID of the device.
    pub id: String,

    /// Human-readable name.
    pub name: String,

    /// Product name/model (e.g., "Hue dimmer switch").
    pub product_name: Option<String>,

    /// List of button service IDs belonging to this device.
    pub button_ids: Vec<String>,

    /// V2 resource ID of the room this device is assigned to.
    pub room_id: Option<String>,
}

impl From<HueSwitchDevice> for HueSwitchDeviceDto {
    fn from(device: HueSwitchDevice) -> Self {
        Self {
            id: device.id,
            name: device.name,
            product_name: device.product_name,
            button_ids: device.button_ids,
            room_id: device.room_id,
        }
    }
}

impl From<HueSwitchDeviceDto> for HueSwitchDevice {
    fn from(dto: HueSwitchDeviceDto) -> Self {
        let mut device = HueSwitchDevice::new(dto.id, dto.name);
        if let Some(product_name) = dto.product_name {
            device = device.with_product_name(product_name);
        }
        device = device.with_button_ids(dto.button_ids);
        if let Some(room_id) = dto.room_id {
            device = device.with_room_id(room_id);
        }
        device
    }
}

/// DTO for a Hue button.
#[derive(Debug, Clone)]
pub struct HueButtonDto {
    /// V2 resource ID of the button.
    pub id: String,

    /// Control ID (button index, 1-4 for dimmer switch).
    pub control_id: u8,

    /// Owner device resource ID.
    pub owner_device_id: String,
}

impl From<HueButton> for HueButtonDto {
    fn from(button: HueButton) -> Self {
        Self {
            id: button.id,
            control_id: button.control_id,
            owner_device_id: button.owner_device_id,
        }
    }
}

impl From<HueButtonDto> for HueButton {
    fn from(dto: HueButtonDto) -> Self {
        HueButton::new(dto.id, dto.control_id, dto.owner_device_id)
    }
}

/// DTO for a Hue room.
#[derive(Debug, Clone)]
pub struct HueRoomDto {
    /// V2 resource ID.
    pub id: String,

    /// Human-readable name.
    pub name: String,
}

impl From<HueRoom> for HueRoomDto {
    fn from(room: HueRoom) -> Self {
        Self {
            id: room.id,
            name: room.name,
        }
    }
}

impl From<HueRoomDto> for HueRoom {
    fn from(dto: HueRoomDto) -> Self {
        HueRoom::new(dto.id, dto.name)
    }
}
