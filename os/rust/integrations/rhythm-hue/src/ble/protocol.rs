//! Philips Hue's vendor-specific BLE GATT protocol.
//!
//! Hue publishes the standard FE0F discovery service, but light control uses
//! Signify 128-bit characteristics. Multi-field writes use the `0007`
//! characteristic as a compact sequence of `(command, length, payload)` TLVs.
//! There is no transition-duration field in this protocol.

use serde::{Deserialize, Serialize};

use super::types::{HueBleColor, HueBleCommand, HueBleState};

pub const HUE_DISCOVERY_SERVICE_UUID: &str = "0000fe0f-0000-1000-8000-00805f9b34fb";
pub const DEVICE_INFORMATION_SERVICE_UUID: &str = "0000180a-0000-1000-8000-00805f9b34fb";
/// Signify's vendor service containing Hue light-state and control characteristics.
pub const LIGHT_CONTROL_SERVICE_UUID: &str = "932c32bd-0000-47a2-835a-a8d455b859dd";
pub const MANUFACTURER_NAME_UUID: &str = "00002a29-0000-1000-8000-00805f9b34fb";
pub const MODEL_NUMBER_UUID: &str = "00002a24-0000-1000-8000-00805f9b34fb";
pub const FIRMWARE_REVISION_UUID: &str = "00002a28-0000-1000-8000-00805f9b34fb";

pub const EUI64_UUID: &str = "97fe6561-0001-4f62-86e9-b71ee2da3d22";
pub const DEVICE_NAME_UUID: &str = "97fe6561-0003-4f62-86e9-b71ee2da3d22";
/// Opens Hue's short-lived pairing handoff window while the current bond is
/// still authenticated. The Pi-side key must only be removed after this write
/// succeeds, otherwise the bulb retains its key and rejects a fresh bond from
/// the same adapter.
pub const PAIRING_HANDOFF_UUID: &str = "97fe6561-2004-4f62-86e9-b71ee2da3d22";
pub const LIGHT_CAPABILITIES_UUID: &str = "932c32bd-0001-47a2-835a-a8d455b859dd";
pub const POWER_UUID: &str = "932c32bd-0002-47a2-835a-a8d455b859dd";
pub const BRIGHTNESS_UUID: &str = "932c32bd-0003-47a2-835a-a8d455b859dd";
pub const COLOR_TEMPERATURE_UUID: &str = "932c32bd-0004-47a2-835a-a8d455b859dd";
pub const XY_COLOR_UUID: &str = "932c32bd-0005-47a2-835a-a8d455b859dd";
pub const COMBINED_CONTROL_UUID: &str = "932c32bd-0007-47a2-835a-a8d455b859dd";

pub const HUE_MIN_BRIGHTNESS: u8 = 1;
pub const HUE_MAX_BRIGHTNESS: u8 = 254;
/// Legacy/default Hue CT range. Newer models can advertise a much wider
/// model-specific range (for example LCA013 is 50..=1000 mired).
pub const HUE_DEFAULT_MIN_MIRED: u16 = 153;
pub const HUE_DEFAULT_MAX_MIRED: u16 = 500;

const COMMAND_POWER: u8 = 0x01;
const COMMAND_BRIGHTNESS: u8 = 0x02;
const COMMAND_COLOR_TEMPERATURE: u8 = 0x03;
const COMMAND_XY_COLOR: u8 = 0x04;
const COMMAND_EFFECT: u8 = 0x06;
const COMMAND_EFFECT_SPEED: u8 = 0x08;

/// Dynamic scenes exposed by recent Hue BLE firmware.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HueBleEffect {
    None,
    Candle,
    Fireplace,
    Prism,
    Sparkle,
    Opal,
    Glisten,
    Underwater,
    Cosmos,
    Sunbeam,
    Enchant,
}

impl HueBleEffect {
    pub const fn protocol_value(self) -> u8 {
        match self {
            Self::None => 0x00,
            Self::Candle => 0x01,
            Self::Fireplace => 0x02,
            Self::Prism => 0x03,
            Self::Sparkle => 0x0a,
            Self::Opal => 0x0b,
            Self::Glisten => 0x0c,
            Self::Underwater => 0x0e,
            Self::Cosmos => 0x0f,
            Self::Sunbeam => 0x10,
            Self::Enchant => 0x11,
        }
    }

    pub const fn from_protocol_value(value: u8) -> Option<Self> {
        match value {
            0x00 => Some(Self::None),
            0x01 => Some(Self::Candle),
            0x02 => Some(Self::Fireplace),
            0x03 => Some(Self::Prism),
            0x0a => Some(Self::Sparkle),
            0x0b => Some(Self::Opal),
            0x0c => Some(Self::Glisten),
            0x0e => Some(Self::Underwater),
            0x0f => Some(Self::Cosmos),
            0x10 => Some(Self::Sunbeam),
            0x11 => Some(Self::Enchant),
            _ => None,
        }
    }
}

/// Encode a combined Hue BLE write.
pub fn encode_combined(command: &HueBleCommand) -> anyhow::Result<Vec<u8>> {
    let mut payload = Vec::with_capacity(18);
    if let Some(on) = command.on {
        push_tlv(&mut payload, COMMAND_POWER, &[u8::from(on)]);
    }
    if let Some(brightness) = command.brightness {
        push_tlv(
            &mut payload,
            COMMAND_BRIGHTNESS,
            &[brightness.clamp(HUE_MIN_BRIGHTNESS, HUE_MAX_BRIGHTNESS)],
        );
    }
    match command.color {
        Some(HueBleColor::ColorTemperature { mired }) => {
            push_tlv(
                &mut payload,
                COMMAND_COLOR_TEMPERATURE,
                &mired.to_le_bytes(),
            );
        }
        Some(HueBleColor::Xy { x, y }) => {
            let x = normalized_xy_to_u16(x).to_le_bytes();
            let y = normalized_xy_to_u16(y).to_le_bytes();
            push_tlv(&mut payload, COMMAND_XY_COLOR, &[x[0], x[1], y[0], y[1]]);
        }
        None => {}
    }
    if let Some(effect) = command.effect {
        push_tlv(&mut payload, COMMAND_EFFECT, &[effect.protocol_value()]);
    }
    if let Some(speed) = command.effect_speed {
        push_tlv(&mut payload, COMMAND_EFFECT_SPEED, &[speed]);
    }
    if payload.is_empty() {
        anyhow::bail!("Hue BLE command has no fields");
    }
    Ok(payload)
}

fn push_tlv(target: &mut Vec<u8>, command: u8, value: &[u8]) {
    target.push(command);
    target.push(value.len() as u8);
    target.extend_from_slice(value);
}

pub fn encode_power(on: bool) -> [u8; 1] {
    [u8::from(on)]
}

pub fn encode_brightness(brightness: u8) -> [u8; 1] {
    [brightness.clamp(HUE_MIN_BRIGHTNESS, HUE_MAX_BRIGHTNESS)]
}

pub fn encode_color_temperature(mired: u16) -> [u8; 2] {
    mired.to_le_bytes()
}

pub fn encode_xy(x: f32, y: f32) -> [u8; 4] {
    let x = normalized_xy_to_u16(x).to_le_bytes();
    let y = normalized_xy_to_u16(y).to_le_bytes();
    [x[0], x[1], y[0], y[1]]
}

pub fn normalized_xy_to_u16(value: f32) -> u16 {
    (value.clamp(0.0, 1.0) * f32::from(u16::MAX)).round() as u16
}

/// The EUI characteristic is little-endian on the wire.
pub fn decode_eui64(raw: &[u8]) -> anyhow::Result<String> {
    if raw.len() != 8 {
        anyhow::bail!("Hue EUI-64 must contain 8 bytes, got {}", raw.len());
    }
    Ok(raw
        .iter()
        .rev()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>())
}

pub fn decode_utf8(raw: &[u8]) -> anyhow::Result<String> {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    Ok(std::str::from_utf8(&raw[..end])?.trim().to_string())
}

pub fn decode_power(raw: &[u8]) -> anyhow::Result<bool> {
    raw.first()
        .copied()
        .map(|value| value != 0)
        .ok_or_else(|| anyhow::anyhow!("Hue power value is empty"))
}

pub fn decode_brightness(raw: &[u8]) -> anyhow::Result<u8> {
    raw.first()
        .copied()
        .ok_or_else(|| anyhow::anyhow!("Hue brightness value is empty"))
}

pub fn decode_color_temperature(raw: &[u8]) -> anyhow::Result<Option<u16>> {
    let bytes: [u8; 2] = raw
        .get(..2)
        .ok_or_else(|| anyhow::anyhow!("Hue color-temperature value is truncated"))?
        .try_into()
        .expect("slice has exact length");
    let mired = u16::from_le_bytes(bytes);
    Ok((mired != u16::MAX).then_some(mired))
}

/// Decode the startup/capability characteristic's CT bounds.
///
/// Hue frames this characteristic with a three-byte version header followed
/// by `(tag, length, value)` TLVs. Tags 1 and 2 are the little-endian minimum
/// and maximum mirek values. Unknown tags are deliberately ignored.
pub fn decode_color_temperature_range(raw: &[u8]) -> anyhow::Result<Option<(u16, u16)>> {
    if raw.len() < 3 {
        anyhow::bail!("Hue light capability value is shorter than its header");
    }
    let mut minimum = None;
    let mut maximum = None;
    let mut offset = 3;
    while offset < raw.len() {
        let tag = raw[offset];
        let length = *raw
            .get(offset + 1)
            .ok_or_else(|| anyhow::anyhow!("Hue light capability TLV is truncated"))?
            as usize;
        offset += 2;
        let value = raw
            .get(offset..offset + length)
            .ok_or_else(|| anyhow::anyhow!("Hue light capability TLV value is truncated"))?;
        match (tag, value) {
            (0x01, [low, high]) => minimum = Some(u16::from_le_bytes([*low, *high])),
            (0x02, [low, high]) => maximum = Some(u16::from_le_bytes([*low, *high])),
            _ => {}
        }
        offset += length;
    }

    match (minimum, maximum) {
        (Some(minimum), Some(maximum)) if minimum > 0 && minimum <= maximum => {
            Ok(Some((minimum, maximum)))
        }
        (Some(_), Some(_)) => anyhow::bail!("Hue light capability CT range is invalid"),
        _ => Ok(None),
    }
}

pub fn decode_xy(raw: &[u8]) -> anyhow::Result<Option<(f32, f32)>> {
    let bytes: [u8; 4] = raw
        .get(..4)
        .ok_or_else(|| anyhow::anyhow!("Hue XY value is truncated"))?
        .try_into()
        .expect("slice has exact length");
    if bytes == [0xff; 4] {
        return Ok(None);
    }
    let x = f32::from(u16::from_le_bytes([bytes[0], bytes[1]])) / f32::from(u16::MAX);
    let y = f32::from(u16::from_le_bytes([bytes[2], bytes[3]])) / f32::from(u16::MAX);
    Ok(Some((x, y)))
}

/// Merge independently read characteristic values into one state snapshot.
pub fn state_from_values(
    power: Option<&[u8]>,
    brightness: Option<&[u8]>,
    color_temperature: Option<&[u8]>,
    xy: Option<&[u8]>,
) -> anyhow::Result<HueBleState> {
    let on = power.map(decode_power).transpose()?;
    let brightness = brightness.map(decode_brightness).transpose()?;
    let color = if let Some(raw) = color_temperature {
        decode_color_temperature(raw)?.map(|mired| HueBleColor::ColorTemperature { mired })
    } else {
        None
    }
    .or_else(|| {
        xy.and_then(|raw| decode_xy(raw).ok().flatten())
            .map(|(x, y)| HueBleColor::Xy { x, y })
    });
    Ok(HueBleState {
        on,
        brightness,
        color,
        effect: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_live_old_bulb_eui_as_canonical_big_endian() {
        assert_eq!(
            decode_eui64(&[0xa7, 0x5b, 0x76, 0x0c, 0x01, 0x88, 0x17, 0x00]).unwrap(),
            "001788010c765ba7"
        );
    }

    #[test]
    fn combined_ct_write_is_atomic_tlv_sequence() {
        let payload = encode_combined(&HueBleCommand {
            on: Some(true),
            brightness: Some(254),
            color: Some(HueBleColor::ColorTemperature { mired: 251 }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            payload,
            vec![0x01, 0x01, 0x01, 0x02, 0x01, 0xfe, 0x03, 0x02, 0xfb, 0x00]
        );
    }

    #[test]
    fn extended_temperature_values_are_not_legacy_clamped() {
        assert_eq!(encode_color_temperature(50), [0x32, 0x00]);
        assert_eq!(encode_color_temperature(1000), [0xe8, 0x03]);
        assert_eq!(
            encode_combined(&HueBleCommand {
                color: Some(HueBleColor::ColorTemperature { mired: 50 }),
                ..Default::default()
            })
            .unwrap(),
            vec![0x03, 0x02, 0x32, 0x00]
        );
    }

    #[test]
    fn combined_xy_write_uses_little_endian_u16_coordinates() {
        let payload = encode_combined(&HueBleCommand {
            on: Some(true),
            brightness: Some(127),
            color: Some(HueBleColor::Xy { x: 0.5, y: 0.25 }),
            ..Default::default()
        })
        .unwrap();
        assert_eq!(
            payload,
            vec![0x01, 0x01, 0x01, 0x02, 0x01, 0x7f, 0x04, 0x04, 0x00, 0x80, 0x00, 0x40]
        );
    }

    #[test]
    fn inactive_color_mode_sentinels_are_ignored() {
        assert_eq!(decode_color_temperature(&[0xff, 0xff]).unwrap(), None);
        assert_eq!(decode_xy(&[0xff, 0xff, 0xff, 0xff]).unwrap(), None);
    }

    #[test]
    fn decodes_live_lca013_extended_temperature_capabilities() {
        let raw = [
            0x00, 0x01, 0x03, 0x01, 0x02, 0x32, 0x00, 0x02, 0x02, 0xe8, 0x03, 0x03, 0x02, 0x07,
            0x00, 0x04, 0x08, 0x0e, 0xfe, 0x03, 0x00, 0x00, 0x00, 0x00, 0x00, 0x06, 0x02, 0x01,
            0x00,
        ];
        assert_eq!(
            decode_color_temperature_range(&raw).unwrap(),
            Some((50, 1000))
        );
    }

    #[test]
    fn capability_parser_rejects_truncated_or_reversed_ranges() {
        assert!(decode_color_temperature_range(&[0x00, 0x01]).is_err());
        assert!(decode_color_temperature_range(&[
            0x00, 0x01, 0x03, 0x01, 0x02, 0xf4, 0x01, 0x02, 0x02, 0x99
        ])
        .is_err());
        assert!(decode_color_temperature_range(&[
            0x00, 0x01, 0x03, 0x01, 0x02, 0xf4, 0x01, 0x02, 0x02, 0x99, 0x00
        ])
        .is_err());
    }

    #[test]
    fn effect_values_match_hue_firmware_protocol() {
        assert_eq!(HueBleEffect::Fireplace.protocol_value(), 0x02);
        assert_eq!(
            HueBleEffect::from_protocol_value(0x10),
            Some(HueBleEffect::Sunbeam)
        );
        assert_eq!(HueBleEffect::from_protocol_value(0x09), None);
    }
}
