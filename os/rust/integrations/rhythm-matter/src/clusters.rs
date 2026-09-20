//! Small Matter lighting helpers shared by the typed controller paths.

/// Matter TransitionTime is a u16 in deciseconds; chipd clamps to this limit.
pub(crate) const MAX_WIRE_TRANSITION_MS: u32 = u16::MAX as u32 * 100;

/// Match chipd's round-up to deciseconds without overflowing at u32::MAX.
pub(crate) fn wire_transition_ms(requested_ms: u32) -> u32 {
    requested_ms.min(MAX_WIRE_TRANSITION_MS).div_ceil(100) * 100
}

/// On/Off cluster ID.
pub const CLUSTER_ON_OFF: u16 = 0x0006;
/// On/Off cluster ID as carried in attribute reports.
pub const CLUSTER_ON_OFF_U32: u32 = CLUSTER_ON_OFF as u32;
/// On/Off attribute ID.
pub const ATTR_ON_OFF: u16 = 0x0000;
/// On/Off attribute ID as carried in attribute reports.
pub const ATTR_ON_OFF_U32: u32 = ATTR_ON_OFF as u32;

/// Convert brightness percentage (1-100) to Matter level (1-254).
pub fn brightness_to_level(brightness: u8) -> u8 {
    if brightness == 0 {
        return 0;
    }

    let scaled = (brightness as u16 * 254) / 100;
    scaled.clamp(1, 254) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brightness_to_level_boundaries() {
        assert_eq!(brightness_to_level(0), 0);
        assert_eq!(brightness_to_level(1), 2);
        assert_eq!(brightness_to_level(50), 127);
        assert_eq!(brightness_to_level(100), 254);
    }
}
