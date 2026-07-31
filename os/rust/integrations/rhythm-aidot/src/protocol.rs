use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};

pub const ADVERTISEMENT_SERVICE_UUID: &str = "00001511-0000-1000-8000-00805f9b34fb";
pub const PRIMARY_GATT_SERVICE_UUID: &str = "00001000-1115-1000-4c44-5341524e4f4f";
pub const ALTERNATE_GATT_SERVICE_UUID: &str = "f000ffc0-0451-4000-b000-000000000000";
pub const MANUFACTURER_ID: u16 = 0x1511;
pub const PRESS_PAYLOAD_LEN: usize = 19;

const QR_PREFIX: &str = "B:";
const QR_SERIAL_SEPARATOR: &str = "%G$S:";
const QR_MODEL_SEPARATOR: &str = "$M:";

/// Parsed durable fields from an Orein/AiDot button QR code.
///
/// The unknown `S` and `M` values remain opaque metadata. The original QR
/// string is intentionally not retained.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AidotSetupCode {
    pub ble_identity: String,
    pub serial_metadata: String,
    pub model_metadata: String,
}

impl AidotSetupCode {
    pub fn parse(value: &str) -> Result<Self> {
        let payload = value
            .strip_prefix(QR_PREFIX)
            .ok_or_else(|| anyhow::anyhow!("not an Orein/AiDot button QR code"))?;
        let (identity, metadata) = payload
            .split_once(QR_SERIAL_SEPARATOR)
            .ok_or_else(|| anyhow::anyhow!("invalid Orein/AiDot button QR fields"))?;
        let (serial_metadata, model_metadata) = metadata
            .split_once(QR_MODEL_SEPARATOR)
            .ok_or_else(|| anyhow::anyhow!("invalid Orein/AiDot button QR metadata"))?;

        if identity.len() != 12 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid Orein/AiDot BLE identity");
        }
        validate_metadata("S", serial_metadata)?;
        validate_metadata("M", model_metadata)?;
        if model_metadata.contains(QR_MODEL_SEPARATOR)
            || serial_metadata.contains(QR_SERIAL_SEPARATOR)
        {
            bail!("invalid Orein/AiDot button QR metadata");
        }

        Ok(Self {
            ble_identity: identity.to_ascii_uppercase(),
            serial_metadata: serial_metadata.to_string(),
            model_metadata: model_metadata.to_string(),
        })
    }

    pub fn formatted_address(&self) -> String {
        self.ble_identity
            .as_bytes()
            .chunks(2)
            .map(|chunk| std::str::from_utf8(chunk).unwrap_or_default())
            .collect::<Vec<_>>()
            .join(":")
    }

    pub fn identity_bytes(&self) -> [u8; 6] {
        let mut bytes = [0_u8; 6];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.ble_identity[index * 2..index * 2 + 2], 16)
                .expect("validated setup identity is hexadecimal");
        }
        bytes
    }
}

fn validate_metadata(label: &str, value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
    {
        bail!("invalid Orein/AiDot {label} metadata");
    }
    Ok(())
}

/// Extract the QR identity mirrored by the observed `0x1511` service data.
pub fn service_data_identity(payload: &[u8]) -> Option<String> {
    if payload.len() < 10 || payload[..4] != [0x00, 0x50, 0x00, 0x15] {
        return None;
    }
    Some(
        payload[4..10]
            .iter()
            .map(|byte| format!("{byte:02X}"))
            .collect(),
    )
}

/// Return the rolling press counter from a validated press advertisement.
pub fn press_counter(payload: &[u8]) -> Option<u8> {
    (payload.len() == PRESS_PAYLOAD_LEN && payload[1] == 0x02 && payload[2] == 0x01)
        .then_some(payload[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const BENCH_QR: &str = "B:1CD6BD2273F9%G$S:L10599FAR002073$M:A001462";

    #[test]
    fn parses_bench_qr_without_retaining_raw_payload() {
        let parsed = AidotSetupCode::parse(BENCH_QR).unwrap();
        assert_eq!(parsed.ble_identity, "1CD6BD2273F9");
        assert_eq!(parsed.formatted_address(), "1C:D6:BD:22:73:F9");
        assert_eq!(parsed.serial_metadata, "L10599FAR002073");
        assert_eq!(parsed.model_metadata, "A001462");
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(!serialized.contains("%G$"));
        assert!(!serialized.contains("B:"));
    }

    #[test]
    fn rejects_near_miss_qr_shapes() {
        for value in [
            "B:1CD6BD2273F%G$S:L10599FAR002073$M:A001462",
            "B:1CD6BD2273FG%G$S:L10599FAR002073$M:A001462",
            "B:1CD6BD2273F9$S:L10599FAR002073$M:A001462",
            "B:1CD6BD2273F9%G$S:$M:A001462",
            "B:1CD6BD2273F9%G$S:L10599FAR002073$M:A001462$M:EXTRA",
            " B:1CD6BD2273F9%G$S:L10599FAR002073$M:A001462",
        ] {
            assert!(AidotSetupCode::parse(value).is_err(), "accepted {value}");
        }
    }

    #[test]
    fn extracts_service_identity_only_from_expected_frame_shape() {
        let payload = [
            0x00, 0x50, 0x00, 0x15, 0x1c, 0xd6, 0xbd, 0x22, 0x73, 0xf9, 0, 0, 0, 0, 0x6b, 0xad,
        ];
        assert_eq!(
            service_data_identity(&payload).as_deref(),
            Some("1CD6BD2273F9")
        );
        assert_eq!(service_data_identity(&payload[..9]), None);
        let mut wrong_prefix = payload;
        wrong_prefix[3] = 0x16;
        assert_eq!(service_data_identity(&wrong_prefix), None);
    }

    #[test]
    fn press_frame_requires_expected_length_and_marker() {
        let mut payload = [0_u8; PRESS_PAYLOAD_LEN];
        payload[..3].copy_from_slice(&[0x85, 0x02, 0x01]);
        assert_eq!(press_counter(&payload), Some(0x85));
        assert_eq!(press_counter(&payload[..18]), None);
        payload[2] = 0x02;
        assert_eq!(press_counter(&payload), None);
    }
}
