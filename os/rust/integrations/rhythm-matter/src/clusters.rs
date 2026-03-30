//! Matter cluster command builders.
//!
//! Provides typed helpers for the lighting-related Matter clusters:
//! On/Off (0x0006), Level Control (0x0008), and Color Control (0x0300).
//!
//! On desktop, TLV payloads are built using matc's spec-generated codecs.
//! On embedded/test, manual byte-packing is used as fallback.

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
// Command IDs
// ============================================================================

pub const CMD_OFF: u8 = 0x00;
pub const CMD_ON: u8 = 0x01;
pub const CMD_MOVE_TO_LEVEL_WITH_ON_OFF: u8 = 0x04;
pub const CMD_MOVE_TO_COLOR: u8 = 0x07;
pub const CMD_MOVE_TO_COLOR_TEMPERATURE: u8 = 0x0A;

// ============================================================================
// Attribute IDs (for reading state)
// ============================================================================

/// On/Off cluster: OnOff attribute.
pub const ATTR_ON_OFF: u16 = 0x0000;
/// Level Control: CurrentLevel attribute.
pub const ATTR_CURRENT_LEVEL: u16 = 0x0000;
/// Color Control: ColorTemperatureMireds attribute.
pub const ATTR_COLOR_TEMP_MIREDS: u16 = 0x0007;

// ============================================================================
// Payload builders — desktop uses matc codecs, fallback uses manual encoding
// ============================================================================

/// Build MoveToLevelWithOnOff TLV payload.
#[cfg(feature = "desktop")]
pub fn build_level_payload(level: u8, transition_tenths: u16) -> Result<Vec<u8>> {
    matc::clusters::codec::level_control::encode_move_to_level_with_on_off(
        level,
        Some(transition_tenths),
        0,
        0,
    )
}

/// Build MoveToLevelWithOnOff TLV payload (manual fallback for embedded/test).
#[cfg(not(feature = "desktop"))]
pub fn build_level_payload(level: u8, transition_tenths: u16) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(5);
    payload.push(level);
    payload.extend_from_slice(&transition_tenths.to_le_bytes());
    payload.push(0x00); // options mask
    payload.push(0x00); // options override
    Ok(payload)
}

/// Build MoveToColorTemperature TLV payload.
#[cfg(feature = "desktop")]
pub fn build_color_temperature_payload(mireds: u16, transition_tenths: u16) -> Result<Vec<u8>> {
    matc::clusters::codec::color_control::encode_move_to_color_temperature(
        mireds,
        transition_tenths,
        0,
        0,
    )
}

/// Build MoveToColorTemperature TLV payload (manual fallback for embedded/test).
#[cfg(not(feature = "desktop"))]
pub fn build_color_temperature_payload(mireds: u16, transition_tenths: u16) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(6);
    payload.extend_from_slice(&mireds.to_le_bytes());
    payload.extend_from_slice(&transition_tenths.to_le_bytes());
    payload.push(0x00); // options mask
    payload.push(0x00); // options override
    Ok(payload)
}

// ============================================================================
// High-level command senders
// ============================================================================

/// Send On command.
pub fn send_on<T: MatterTransport>(transport: &T, node_id: u64, endpoint: u16) -> Result<()> {
    transport.send_cluster_cmd(node_id, endpoint, CLUSTER_ON_OFF, CMD_ON, &[])
}

/// Send Off command.
pub fn send_off<T: MatterTransport>(transport: &T, node_id: u64, endpoint: u16) -> Result<()> {
    transport.send_cluster_cmd(node_id, endpoint, CLUSTER_ON_OFF, CMD_OFF, &[])
}

/// Send MoveToLevelWithOnOff command.
///
/// `level` is 0-254 (Matter uses 0-254, not 0-255).
/// `transition_tenths` is in tenths of a second (0 = instant).
pub fn send_level<T: MatterTransport>(
    transport: &T,
    node_id: u64,
    endpoint: u16,
    level: u8,
    transition_tenths: u16,
) -> Result<()> {
    let payload = build_level_payload(level, transition_tenths)?;
    transport.send_cluster_cmd(
        node_id,
        endpoint,
        CLUSTER_LEVEL_CONTROL,
        CMD_MOVE_TO_LEVEL_WITH_ON_OFF,
        &payload,
    )
}

/// Build MoveToColor (XY) TLV payload.
#[cfg(feature = "desktop")]
pub fn build_color_xy_payload(color_x: u16, color_y: u16, transition_tenths: u16) -> Result<Vec<u8>> {
    matc::clusters::codec::color_control::encode_move_to_color(
        color_x,
        color_y,
        transition_tenths,
        0,
        0,
    )
}

/// Build MoveToColor (XY) TLV payload (manual fallback for embedded/test).
#[cfg(not(feature = "desktop"))]
pub fn build_color_xy_payload(color_x: u16, color_y: u16, transition_tenths: u16) -> Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(8);
    payload.extend_from_slice(&color_x.to_le_bytes());
    payload.extend_from_slice(&color_y.to_le_bytes());
    payload.extend_from_slice(&transition_tenths.to_le_bytes());
    payload.push(0x00); // options mask
    payload.push(0x00); // options override
    Ok(payload)
}

/// Send MoveToColor (XY) command.
///
/// `color_x` and `color_y` are CIE xy coordinates as 16-bit fixed point
/// (multiply float 0.0–1.0 by 65535).
/// `transition_tenths` is in tenths of a second.
pub fn send_color_xy<T: MatterTransport>(
    transport: &T,
    node_id: u64,
    endpoint: u16,
    color_x: u16,
    color_y: u16,
    transition_tenths: u16,
) -> Result<()> {
    let payload = build_color_xy_payload(color_x, color_y, transition_tenths)?;
    transport.send_cluster_cmd(
        node_id,
        endpoint,
        CLUSTER_COLOR_CONTROL,
        CMD_MOVE_TO_COLOR,
        &payload,
    )
}

/// Convert float xy (0.0–1.0) to Matter's 16-bit fixed point.
pub fn xy_to_matter(xy: f32) -> u16 {
    (xy.clamp(0.0, 1.0) * 65535.0) as u16
}

/// Send MoveToColorTemperature command.
///
/// `mireds` is the color temperature in mireds (1,000,000 / kelvin).
/// `transition_tenths` is in tenths of a second.
pub fn send_color_temperature<T: MatterTransport>(
    transport: &T,
    node_id: u64,
    endpoint: u16,
    mireds: u16,
    transition_tenths: u16,
) -> Result<()> {
    let payload = build_color_temperature_payload(mireds, transition_tenths)?;
    transport.send_cluster_cmd(
        node_id,
        endpoint,
        CLUSTER_COLOR_CONTROL,
        CMD_MOVE_TO_COLOR_TEMPERATURE,
        &payload,
    )
}

// ============================================================================
// Conversion utilities
// ============================================================================

/// Convert brightness percentage (1-100) to Matter level (1-254).
pub fn brightness_to_level(brightness: u8) -> u8 {
    if brightness == 0 {
        return 0;
    }
    let scaled = (brightness as u16 * 254) / 100;
    scaled.clamp(1, 254) as u8
}

/// Convert Kelvin to mireds.
pub fn kelvin_to_mireds(kelvin: u16) -> u16 {
    if kelvin == 0 {
        return 500; // default warm white
    }
    (1_000_000u32 / kelvin as u32).clamp(1, 65279) as u16
}

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
        assert_eq!(kelvin_to_mireds(0), 500);
    }

    #[test]
    fn level_payload_has_correct_length() {
        let payload = build_level_payload(128, 10).unwrap();
        assert!(payload.len() >= 5); // level + transition(2) + options(2)
    }

    #[test]
    fn color_temp_payload_has_correct_length() {
        let payload = build_color_temperature_payload(370, 10).unwrap();
        assert!(payload.len() >= 6); // mireds(2) + transition(2) + options(2)
    }
}
