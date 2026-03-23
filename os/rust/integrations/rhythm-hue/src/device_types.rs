//! Hue device types.
//!
//! Data structures representing Hue V2 API resources for switches,
//! buttons, and rooms.

/// A Hue switch/button device discovered from the V2 API.
///
/// This represents a physical device like a Hue Dimmer Switch (RWL02x)
/// or Hue Tap Dial Switch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HueSwitchDevice {
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

impl HueSwitchDevice {
    /// Create a new switch device.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            product_name: None,
            button_ids: Vec::new(),
            room_id: None,
        }
    }

    /// Set the product name.
    pub fn with_product_name(mut self, product_name: impl Into<String>) -> Self {
        self.product_name = Some(product_name.into());
        self
    }

    /// Set the button IDs.
    pub fn with_button_ids(mut self, button_ids: Vec<String>) -> Self {
        self.button_ids = button_ids;
        self
    }

    /// Set the room ID.
    pub fn with_room_id(mut self, room_id: impl Into<String>) -> Self {
        self.room_id = Some(room_id.into());
        self
    }
}

/// A Hue button resource from the V2 API.
///
/// Each physical button on a switch device has a corresponding button resource.
/// For example, a Hue Dimmer Switch has 4 button resources (control_id 1-4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HueButton {
    /// V2 resource ID of the button.
    pub id: String,

    /// Control ID (button index, 1-4 for dimmer switch).
    pub control_id: u8,

    /// Owner device resource ID.
    pub owner_device_id: String,
}

impl HueButton {
    /// Create a new button.
    pub fn new(id: impl Into<String>, control_id: u8, owner_device_id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            control_id,
            owner_device_id: owner_device_id.into(),
        }
    }
}

/// A room from the Hue V2 API.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HueRoom {
    /// V2 resource ID.
    pub id: String,

    /// Human-readable name.
    pub name: String,
}

impl HueRoom {
    /// Create a new room.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_switch_device_builder() {
        let device = HueSwitchDevice::new("device-1", "Living Room Switch")
            .with_product_name("Hue dimmer switch")
            .with_button_ids(vec![
                "btn-1".to_string(),
                "btn-2".to_string(),
                "btn-3".to_string(),
                "btn-4".to_string(),
            ])
            .with_room_id("room-1");

        assert_eq!(device.id, "device-1");
        assert_eq!(device.name, "Living Room Switch");
        assert_eq!(device.product_name, Some("Hue dimmer switch".to_string()));
        assert_eq!(device.button_ids.len(), 4);
        assert_eq!(device.room_id, Some("room-1".to_string()));
    }

    #[test]
    fn test_button() {
        let button = HueButton::new("btn-1", 1, "device-1");

        assert_eq!(button.id, "btn-1");
        assert_eq!(button.control_id, 1);
        assert_eq!(button.owner_device_id, "device-1");
    }

    #[test]
    fn test_room() {
        let room = HueRoom::new("room-1", "Living Room");

        assert_eq!(room.id, "room-1");
        assert_eq!(room.name, "Living Room");
    }
}
