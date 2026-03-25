//! Matter cluster command builders.
//!
//! Provides typed helpers for the lighting-related Matter clusters:
//! On/Off (0x0006), Level Control (0x0008), and Color Control (0x0300).

use anyhow::Result;

use crate::transport::MatterTransport;

// ============================================================================
// Cluster IDs
// ============================================================================

/// On/Off cluster (Matter spec section 1.5).
pub const CLUSTER_ON_OFF: u16 = 0x0006;
/// Level Control cluster (Matter spec section 1.6).
pub const CLUSTER_LEVEL_CONTROL: u16 = 0x0008;
/// Color Control cluster (Matter spec section 3.2).
pub const CLUSTER_COLOR_CONTROL: u16 = 0x0300;

// ============================================================================
// On/Off commands
// ============================================================================

/// On/Off cluster command IDs.
pub const CMD_OFF: u8 = 0x00;
pub const CMD_ON: u8 = 0x01;

/// Send On command.
pub fn send_on<T: MatterTransport>(transport: &T, node_id: u64, endpoint: u16) -> Result<()> {
    transport.send_cluster_cmd(node_id, endpoint, CLUSTER_ON_OFF, CMD_ON, &[])
}

/// Send Off command.
pub fn send_off<T: MatterTransport>(transport: &T, node_id: u64, endpoint: u16) -> Result<()> {
    transport.send_cluster_cmd(node_id, endpoint, CLUSTER_ON_OFF, CMD_OFF, &[])
}

// ============================================================================
// Level Control commands
// ============================================================================

/// MoveToLevel command ID.
pub const CMD_MOVE_TO_LEVEL: u8 = 0x00;
/// MoveToLevelWithOnOff command ID (turns on if off).
pub const CMD_MOVE_TO_LEVEL_WITH_ON_OFF: u8 = 0x04;

/// Send MoveToLevelWithOnOff command.
///
/// `level` is 0-254 (Matter uses 0-254, not 0-255).
/// `transition_time` is in tenths of a second (0 = instant).
pub fn send_level<T: MatterTransport>(
    transport: &T,
    node_id: u64,
    endpoint: u16,
    level: u8,
    transition_tenths: u16,
) -> Result<()> {
    // TLV encoding: level (u8), transition time (u16), options mask (u8), options override (u8)
    let mut payload = Vec::with_capacity(5);
    payload.push(level);
    payload.extend_from_slice(&transition_tenths.to_le_bytes());
    payload.push(0x00); // options mask
    payload.push(0x00); // options override
    transport.send_cluster_cmd(
        node_id,
        endpoint,
        CLUSTER_LEVEL_CONTROL,
        CMD_MOVE_TO_LEVEL_WITH_ON_OFF,
        &payload,
    )
}

/// Convert brightness percentage (1-100) to Matter level (1-254).
pub fn brightness_to_level(brightness: u8) -> u8 {
    if brightness == 0 {
        return 0;
    }
    let scaled = (brightness as u16 * 254) / 100;
    scaled.clamp(1, 254) as u8
}

// ============================================================================
// Color Control commands
// ============================================================================

/// MoveToColorTemperature command ID.
pub const CMD_MOVE_TO_COLOR_TEMPERATURE: u8 = 0x0A;

/// Send MoveToColorTemperature command.
///
/// `mireds` is the color temperature in mireds (1,000,000 / kelvin).
/// `transition_time` is in tenths of a second.
pub fn send_color_temperature<T: MatterTransport>(
    transport: &T,
    node_id: u64,
    endpoint: u16,
    mireds: u16,
    transition_tenths: u16,
) -> Result<()> {
    // TLV encoding: color_temp_mireds (u16), transition time (u16), options mask (u8), options override (u8)
    let mut payload = Vec::with_capacity(6);
    payload.extend_from_slice(&mireds.to_le_bytes());
    payload.extend_from_slice(&transition_tenths.to_le_bytes());
    payload.push(0x00); // options mask
    payload.push(0x00); // options override
    transport.send_cluster_cmd(
        node_id,
        endpoint,
        CLUSTER_COLOR_CONTROL,
        CMD_MOVE_TO_COLOR_TEMPERATURE,
        &payload,
    )
}

/// Convert Kelvin to mireds.
pub fn kelvin_to_mireds(kelvin: u16) -> u16 {
    if kelvin == 0 {
        return 500; // default warm white
    }
    (1_000_000u32 / kelvin as u32).clamp(1, 65279) as u16
}

// ============================================================================
// Attribute IDs (for reading state)
// ============================================================================

/// On/Off cluster: OnOff attribute.
pub const ATTR_ON_OFF: u16 = 0x0000;
/// Level Control: CurrentLevel attribute.
pub const ATTR_CURRENT_LEVEL: u16 = 0x0000;
/// Color Control: ColorTemperatureMireds attribute.
pub const ATTR_COLOR_TEMP_MIREDS: u16 = 0x0007;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_to_level_boundaries() {
        assert_eq!(brightness_to_level(0), 0);
        assert_eq!(brightness_to_level(1), 2); // 1% -> ~2.54 -> 2
        assert_eq!(brightness_to_level(50), 127);
        assert_eq!(brightness_to_level(100), 254);
    }

    #[test]
    fn kelvin_to_mireds_conversions() {
        assert_eq!(kelvin_to_mireds(2700), 370);
        assert_eq!(kelvin_to_mireds(4000), 250);
        assert_eq!(kelvin_to_mireds(6500), 153);
        // Edge case
        assert_eq!(kelvin_to_mireds(0), 500);
    }
}
