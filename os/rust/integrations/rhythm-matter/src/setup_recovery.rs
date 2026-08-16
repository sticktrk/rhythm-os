//! Durable owner-only Matter setup payload recovery.

use std::sync::{Mutex, OnceLock};

use anyhow::{Context, Result};
use rhythm_os::pairing::PairingRecoverySecret;
use rhythm_os::state::SharedState;
use serde::{Deserialize, Serialize};

const STORE_PATH: &str = "matter/setup-payloads.json";
const STORE_SCHEMA_VERSION: u32 = 1;
const MAX_SETUP_PAYLOAD_LEN: usize = 1_024;
static STORE_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SetupPayloadStore {
    schema_version: u32,
    #[serde(default)]
    entries: Vec<SetupPayloadEntry>,
}

impl Default for SetupPayloadStore {
    fn default() -> Self {
        Self {
            schema_version: STORE_SCHEMA_VERSION,
            entries: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct SetupPayloadEntry {
    fabric_id: String,
    node_id: u64,
    endpoint: u16,
    payload_kind: SetupPayloadKind,
    setup_payload: String,
    captured_at: String,
}

/// Private identity returned when a submitted setup payload matches saved
/// recovery material on the active fabric. The setup payload itself never
/// leaves this module during repeat-pair lookup.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SetupPayloadTarget {
    pub node_id: u64,
    pub endpoint: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SetupPayloadKind {
    QrCode,
    ManualCode,
}

impl SetupPayloadKind {
    fn from_payload(payload: &str) -> Self {
        if payload
            .get(..3)
            .is_some_and(|prefix| prefix.eq_ignore_ascii_case("MT:"))
        {
            Self::QrCode
        } else {
            Self::ManualCode
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::QrCode => "qr_code",
            Self::ManualCode => "manual_code",
        }
    }
}

fn normalized_setup_payload_for_match(payload: &str) -> String {
    let payload = payload.trim();
    if payload
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("MT:"))
    {
        return payload.to_ascii_uppercase();
    }

    if payload.chars().all(|character| {
        character.is_ascii_digit() || character == '-' || character.is_ascii_whitespace()
    }) {
        return payload
            .chars()
            .filter(|character| character.is_ascii_digit())
            .collect();
    }

    payload.to_string()
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum SetupPayloadMatchKey {
    Qr {
        setup_pin: u32,
        long_discriminator: u16,
        vendor_id: u16,
        product_id: u16,
    },
    Manual {
        setup_pin: u32,
        short_discriminator: u8,
        vendor_product: Option<(u16, u16)>,
        normalized_code: String,
    },
    /// Preserve exact normalized matching for legacy/test payloads that are
    /// not valid Matter onboarding payloads.
    Exact(String),
}

fn setup_payload_match_key(payload: &str) -> SetupPayloadMatchKey {
    parse_qr_onboarding_identity(payload)
        .or_else(|| parse_manual_onboarding_identity(payload))
        .unwrap_or_else(|| SetupPayloadMatchKey::Exact(normalized_setup_payload_for_match(payload)))
}

impl SetupPayloadMatchKey {
    fn matches(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Qr {
                    setup_pin: left_pin,
                    long_discriminator: left_discriminator,
                    vendor_id: left_vendor,
                    product_id: left_product,
                },
                Self::Qr {
                    setup_pin: right_pin,
                    long_discriminator: right_discriminator,
                    vendor_id: right_vendor,
                    product_id: right_product,
                },
            ) => {
                left_pin == right_pin
                    && left_discriminator == right_discriminator
                    && left_vendor == right_vendor
                    && left_product == right_product
            }
            (
                Self::Manual {
                    normalized_code: left,
                    ..
                },
                Self::Manual {
                    normalized_code: right,
                    ..
                },
            ) => left == right,
            (Self::Exact(left), Self::Exact(right)) => left == right,
            (Self::Qr { .. }, Self::Manual { .. }) | (Self::Manual { .. }, Self::Qr { .. }) => {
                let Some((left_pin, left_discriminator, left_vendor_product)) =
                    self.cross_form_identity()
                else {
                    return false;
                };
                let Some((right_pin, right_discriminator, right_vendor_product)) =
                    other.cross_form_identity()
                else {
                    return false;
                };
                left_pin == right_pin
                    && left_discriminator == right_discriminator
                    && match (left_vendor_product, right_vendor_product) {
                        (Some(left), Some(right)) => left == right,
                        _ => true,
                    }
            }
            _ => false,
        }
    }

    fn cross_form_identity(&self) -> Option<(u32, u8, Option<(u16, u16)>)> {
        match self {
            Self::Qr {
                setup_pin,
                long_discriminator,
                vendor_id,
                product_id,
            } => Some((
                *setup_pin,
                (long_discriminator >> 8) as u8,
                Some((*vendor_id, *product_id)),
            )),
            Self::Manual {
                setup_pin,
                short_discriminator,
                vendor_product,
                ..
            } => Some((*setup_pin, *short_discriminator, *vendor_product)),
            Self::Exact(_) => None,
        }
    }
}

fn parse_manual_onboarding_identity(payload: &str) -> Option<SetupPayloadMatchKey> {
    let digits = payload
        .trim()
        .chars()
        .filter(|character| character.is_ascii_digit())
        .collect::<String>();
    if !payload.trim().chars().all(|character| {
        character.is_ascii_digit() || character == '-' || character.is_ascii_whitespace()
    }) || !matches!(digits.len(), 11 | 21)
        || !verhoeff_checksum_is_valid(&digits)
    {
        return None;
    }

    // Matter's manual-code decimal chunks carry these fields in the first
    // ten digits (the remaining ten long-code digits are vendor/product IDs).
    let chunk1 = digits.get(0..1)?.parse::<u32>().ok()?;
    let chunk2 = digits.get(1..6)?.parse::<u32>().ok()?;
    let chunk3 = digits.get(6..10)?.parse::<u32>().ok()?;
    if chunk1 > 7 || chunk2 > u16::MAX.into() || chunk3 >= (1 << 13) {
        return None;
    }
    let has_vendor_product = chunk1 & 0x4 != 0;
    if has_vendor_product != (digits.len() == 21) {
        return None;
    }

    let short_discriminator = (((chunk1 & 0x3) << 2) | ((chunk2 >> 14) & 0x3)) as u8;
    let setup_pin = (chunk2 & ((1 << 14) - 1)) | ((chunk3 & ((1 << 13) - 1)) << 14);
    let vendor_product = if has_vendor_product {
        Some((
            digits.get(10..15)?.parse::<u16>().ok()?,
            digits.get(15..20)?.parse::<u16>().ok()?,
        ))
    } else {
        None
    };
    Some(SetupPayloadMatchKey::Manual {
        setup_pin,
        short_discriminator,
        vendor_product,
        normalized_code: digits,
    })
}

fn parse_qr_onboarding_identity(payload: &str) -> Option<SetupPayloadMatchKey> {
    let payload = payload.trim();
    if !payload
        .get(..3)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case("MT:"))
    {
        return None;
    }
    let decoded = decode_base38(payload.get(3..)?)?;
    if decoded.len() < 11 {
        return None;
    }

    // Matter QR mandatory payload bit layout is little-endian: version (3),
    // VID (16), PID (16), flow (2), rendezvous (8), discriminator (12),
    // setup PIN (27), and padding (4).
    let version = read_little_endian_bits(&decoded, 0, 3)?;
    let vendor_id = read_little_endian_bits(&decoded, 3, 16)? as u16;
    let product_id = read_little_endian_bits(&decoded, 19, 16)? as u16;
    let commissioning_flow = read_little_endian_bits(&decoded, 35, 2)?;
    let long_discriminator = read_little_endian_bits(&decoded, 45, 12)? as u16;
    let setup_pin = read_little_endian_bits(&decoded, 57, 27)? as u32;
    let padding = read_little_endian_bits(&decoded, 84, 4)?;
    if version != 0 || commissioning_flow > 2 || padding != 0 {
        return None;
    }

    Some(SetupPayloadMatchKey::Qr {
        setup_pin,
        long_discriminator,
        vendor_id,
        product_id,
    })
}

fn decode_base38(encoded: &str) -> Option<Vec<u8>> {
    let encoded = encoded.as_bytes();
    let mut decoded = Vec::with_capacity((encoded.len() * 3).div_ceil(5));
    let mut offset = 0;
    while offset < encoded.len() {
        let remaining = encoded.len() - offset;
        let (encoded_len, decoded_len) = if remaining >= 5 {
            (5, 3)
        } else if remaining == 4 {
            (4, 2)
        } else if remaining == 2 {
            (2, 1)
        } else {
            return None;
        };
        let mut value = 0_u32;
        for character in encoded[offset..offset + encoded_len].iter().rev() {
            value = value
                .checked_mul(38)?
                .checked_add(base38_value(*character)?)?;
        }
        if value >= (1_u32 << (decoded_len * 8)) {
            return None;
        }
        for byte_index in 0..decoded_len {
            decoded.push(((value >> (byte_index * 8)) & 0xff) as u8);
        }
        offset += encoded_len;
    }
    Some(decoded)
}

fn base38_value(character: u8) -> Option<u32> {
    match character.to_ascii_uppercase() {
        b'0'..=b'9' => Some((character - b'0') as u32),
        b'A'..=b'Z' => Some((character.to_ascii_uppercase() - b'A' + 10) as u32),
        b'-' => Some(36),
        b'.' => Some(37),
        _ => None,
    }
}

fn read_little_endian_bits(bytes: &[u8], offset: usize, bit_count: usize) -> Option<u64> {
    if bit_count > 64 || offset.checked_add(bit_count)? > bytes.len() * 8 {
        return None;
    }
    let mut value = 0_u64;
    for output_bit in 0..bit_count {
        let source_bit = offset + output_bit;
        value |= u64::from((bytes[source_bit / 8] >> (source_bit % 8)) & 1) << output_bit;
    }
    Some(value)
}

fn verhoeff_checksum_is_valid(digits: &str) -> bool {
    const MULTIPLICATION: [[usize; 10]; 10] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 2, 3, 4, 0, 6, 7, 8, 9, 5],
        [2, 3, 4, 0, 1, 7, 8, 9, 5, 6],
        [3, 4, 0, 1, 2, 8, 9, 5, 6, 7],
        [4, 0, 1, 2, 3, 9, 5, 6, 7, 8],
        [5, 9, 8, 7, 6, 0, 4, 3, 2, 1],
        [6, 5, 9, 8, 7, 1, 0, 4, 3, 2],
        [7, 6, 5, 9, 8, 2, 1, 0, 4, 3],
        [8, 7, 6, 5, 9, 3, 2, 1, 0, 4],
        [9, 8, 7, 6, 5, 4, 3, 2, 1, 0],
    ];
    const PERMUTATION: [[usize; 10]; 8] = [
        [0, 1, 2, 3, 4, 5, 6, 7, 8, 9],
        [1, 5, 7, 6, 2, 8, 3, 0, 9, 4],
        [5, 8, 0, 3, 7, 9, 6, 1, 4, 2],
        [8, 9, 1, 6, 0, 4, 3, 5, 2, 7],
        [9, 4, 5, 3, 1, 2, 6, 8, 7, 0],
        [4, 2, 8, 6, 5, 7, 3, 9, 0, 1],
        [2, 7, 9, 3, 8, 0, 6, 4, 1, 5],
        [7, 0, 4, 6, 9, 1, 3, 2, 5, 8],
    ];

    digits
        .bytes()
        .rev()
        .enumerate()
        .try_fold(0_usize, |checksum, (index, digit)| {
            let digit = digit.checked_sub(b'0')? as usize;
            Some(MULTIPLICATION[checksum][PERMUTATION[index % 8][digit]])
        })
        == Some(0)
}

fn with_storage<T>(
    state: &SharedState,
    action: impl FnOnce(&dyn rhythm_os::storage::Storage) -> Result<T>,
) -> Result<T> {
    let state = state
        .lock()
        .map_err(|_| anyhow::anyhow!("state lock poisoned"))?;
    let storage = state
        .storage
        .as_deref()
        .ok_or_else(|| anyhow::anyhow!("storage not configured"))?;
    action(storage)
}

fn load_store(storage: &dyn rhythm_os::storage::Storage) -> Result<SetupPayloadStore> {
    let Some(content) = storage.load_integration_state_file(STORE_PATH)? else {
        return Ok(SetupPayloadStore::default());
    };
    let store: SetupPayloadStore =
        serde_json::from_str(&content).context("parsing Matter setup recovery store")?;
    if store.schema_version != STORE_SCHEMA_VERSION {
        anyhow::bail!(
            "unsupported Matter setup recovery schema {}",
            store.schema_version
        );
    }
    Ok(store)
}

fn save_store(storage: &dyn rhythm_os::storage::Storage, store: &SetupPayloadStore) -> Result<()> {
    if store.entries.is_empty() {
        return storage.delete_integration_state_file(STORE_PATH);
    }
    let content =
        serde_json::to_string_pretty(store).context("serializing Matter setup recovery store")?;
    storage
        .save_integration_state_file(STORE_PATH, &content)
        .context("persisting Matter setup recovery store")
}

/// Save the exact setup payload accepted by a successful commission.
pub fn save_setup_payload(
    state: &SharedState,
    fabric_id: &str,
    node_id: u64,
    endpoint: u16,
    setup_payload: &str,
) -> Result<()> {
    let setup_payload = setup_payload.trim();
    if setup_payload.is_empty() || setup_payload.len() > MAX_SETUP_PAYLOAD_LEN {
        anyhow::bail!("Matter setup payload has an invalid length");
    }
    let fabric_id = fabric_id.trim();
    if fabric_id.is_empty() {
        anyhow::bail!("Matter fabric identity is unavailable");
    }
    let _guard = STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter setup recovery store lock poisoned"))?;
    with_storage(state, |storage| {
        let mut store = load_store(storage)?;
        let entry = SetupPayloadEntry {
            fabric_id: fabric_id.to_string(),
            node_id,
            endpoint,
            payload_kind: SetupPayloadKind::from_payload(setup_payload),
            setup_payload: setup_payload.to_string(),
            captured_at: chrono::Utc::now().to_rfc3339(),
        };
        if let Some(existing) = store
            .entries
            .iter_mut()
            .find(|existing| existing.fabric_id == fabric_id && existing.node_id == node_id)
        {
            *existing = entry;
        } else {
            store.entries.push(entry);
        }
        save_store(storage, &store)
    })
}

/// Load setup recovery only when the caller supplies the active fabric and
/// exact native endpoint identity.
pub fn load_setup_payload(
    state: &SharedState,
    fabric_id: &str,
    native_device_id: &str,
) -> Result<Option<PairingRecoverySecret>> {
    let Some((node_id, endpoint)) = crate::lifecycle::parse_device_id(native_device_id) else {
        anyhow::bail!("Invalid Matter device ID");
    };
    let _guard = STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter setup recovery store lock poisoned"))?;
    with_storage(state, |storage| {
        let store = load_store(storage)?;
        Ok(store
            .entries
            .iter()
            .find(|entry| {
                entry.fabric_id == fabric_id
                    && entry.node_id == node_id
                    && entry.endpoint == endpoint
            })
            .map(|entry| PairingRecoverySecret {
                payload_kind: entry.payload_kind.as_str().to_string(),
                setup_payload: entry.setup_payload.clone(),
                captured_at: entry.captured_at.clone(),
            }))
    })
}

/// Find private endpoint identities whose saved payload matches a newly
/// submitted scan/code on the active fabric. Callers must still prove that a
/// returned endpoint remains registered before treating it as a recovery
/// target. More than one result is intentionally preserved so the pairing
/// orchestrator can fail safely instead of guessing between duplicate codes.
pub(crate) fn find_setup_payload_targets(
    state: &SharedState,
    fabric_id: &str,
    setup_payload: &str,
) -> Result<Vec<SetupPayloadTarget>> {
    let setup_payload = setup_payload.trim();
    if setup_payload.is_empty() || setup_payload.len() > MAX_SETUP_PAYLOAD_LEN {
        anyhow::bail!("Matter setup payload has an invalid length");
    }
    let fabric_id = fabric_id.trim();
    if fabric_id.is_empty() {
        anyhow::bail!("Matter fabric identity is unavailable");
    }
    let match_key = setup_payload_match_key(setup_payload);
    let _guard = STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter setup recovery store lock poisoned"))?;
    with_storage(state, |storage| {
        let store = load_store(storage)?;
        let mut targets = store
            .entries
            .iter()
            .filter(|entry| {
                entry.fabric_id == fabric_id
                    && setup_payload_match_key(&entry.setup_payload).matches(&match_key)
            })
            .map(|entry| SetupPayloadTarget {
                node_id: entry.node_id,
                endpoint: entry.endpoint,
            })
            .collect::<Vec<_>>();
        targets.sort_unstable_by_key(|target| (target.node_id, target.endpoint));
        targets.dedup();
        Ok(targets)
    })
}

/// Delete all endpoint records belonging to a decommissioned Matter node.
pub fn delete_setup_payloads_for_node(
    state: &SharedState,
    fabric_id: &str,
    node_id: u64,
) -> Result<()> {
    let _guard = STORE_LOCK
        .get_or_init(|| Mutex::new(()))
        .lock()
        .map_err(|_| anyhow::anyhow!("Matter setup recovery store lock poisoned"))?;
    with_storage(state, |storage| {
        let mut store = load_store(storage)?;
        store
            .entries
            .retain(|entry| !(entry.fabric_id == fabric_id && entry.node_id == node_id));
        save_store(storage, &store)
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use rhythm_os::state::AppState;
    use rhythm_os::storage::FileStorage;
    use std::sync::{Arc, Mutex};

    fn state_with_storage(name: &str) -> (SharedState, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "rhythm-matter-setup-recovery-{name}-{}",
            chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default()
        ));
        let storage = FileStorage::new(path.to_str().unwrap()).unwrap();
        let mut state = AppState::default();
        state.storage = Some(Arc::new(storage));
        (Arc::new(Mutex::new(state)), path)
    }

    #[test]
    fn qr_and_manual_payloads_round_trip_by_fabric_and_endpoint() {
        let (state, path) = state_with_storage("round-trip");
        save_setup_payload(&state, "fabric-a", 42, 2, "MT:RECOVERY-SECRET").unwrap();
        save_setup_payload(&state, "fabric-a", 43, 1, "34970112332").unwrap();

        let qr = load_setup_payload(&state, "fabric-a", "matter-42-2")
            .unwrap()
            .unwrap();
        assert_eq!(qr.payload_kind, "qr_code");
        assert_eq!(qr.setup_payload, "MT:RECOVERY-SECRET");
        let manual = load_setup_payload(&state, "fabric-a", "matter-43")
            .unwrap()
            .unwrap();
        assert_eq!(manual.payload_kind, "manual_code");
        assert_eq!(manual.setup_payload, "34970112332");
        assert!(load_setup_payload(&state, "fabric-b", "matter-42-2")
            .unwrap()
            .is_none());
        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", "mt:recovery-secret").unwrap(),
            vec![SetupPayloadTarget {
                node_id: 42,
                endpoint: 2,
            }]
        );
        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", "3497-011-2332").unwrap(),
            vec![SetupPayloadTarget {
                node_id: 43,
                endpoint: 1,
            }]
        );
        assert!(
            find_setup_payload_targets(&state, "fabric-b", "MT:RECOVERY-SECRET")
                .unwrap()
                .is_empty()
        );

        let mode = std::fs::metadata(path.join(STORE_PATH))
            .unwrap()
            .permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(mode.mode() & 0o777, 0o600);
        }
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn duplicate_payload_matches_remain_ambiguous() {
        let (state, path) = state_with_storage("duplicate-match");
        save_setup_payload(&state, "fabric-a", 42, 1, "12345678901").unwrap();
        save_setup_payload(&state, "fabric-a", 43, 2, "123-456-78901").unwrap();

        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", "123 456 78901").unwrap(),
            vec![
                SetupPayloadTarget {
                    node_id: 42,
                    endpoint: 1,
                },
                SetupPayloadTarget {
                    node_id: 43,
                    endpoint: 2,
                },
            ]
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn qr_and_manual_forms_of_the_same_onboarding_identity_match() {
        let (state, path) = state_with_storage("cross-form-match");
        const QR_CODE: &str = "MT:YNJV75HZ00KA0648G00";
        const MANUAL_CODE: &str = "34970112332";

        assert_eq!(
            setup_payload_match_key(QR_CODE),
            SetupPayloadMatchKey::Qr {
                setup_pin: 20_202_021,
                long_discriminator: 0x0f00,
                vendor_id: 9_050,
                product_id: 65_279,
            }
        );
        assert!(setup_payload_match_key(MANUAL_CODE).matches(&setup_payload_match_key(QR_CODE)));

        save_setup_payload(&state, "fabric-a", 42, 1, QR_CODE).unwrap();
        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", MANUAL_CODE).unwrap(),
            vec![SetupPayloadTarget {
                node_id: 42,
                endpoint: 1,
            }]
        );

        save_setup_payload(&state, "fabric-a", 42, 1, MANUAL_CODE).unwrap();
        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", QR_CODE).unwrap(),
            vec![SetupPayloadTarget {
                node_id: 42,
                endpoint: 1,
            }]
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn qr_matches_keep_full_discriminator_and_product_precision() {
        let base = SetupPayloadMatchKey::Qr {
            setup_pin: 20_202_021,
            long_discriminator: 0x0f00,
            vendor_id: 0xfff1,
            product_id: 0x8000,
        };
        let different_long_discriminator = SetupPayloadMatchKey::Qr {
            setup_pin: 20_202_021,
            long_discriminator: 0x0fff,
            vendor_id: 0xfff1,
            product_id: 0x8000,
        };
        let different_product = SetupPayloadMatchKey::Qr {
            setup_pin: 20_202_021,
            long_discriminator: 0x0f00,
            vendor_id: 0xfff1,
            product_id: 0x8001,
        };

        assert!(!base.matches(&different_long_discriminator));
        assert!(!base.matches(&different_product));
    }

    #[test]
    fn saving_a_rekeyed_endpoint_replaces_the_old_target_for_that_node() {
        let (state, path) = state_with_storage("endpoint-rekey");
        save_setup_payload(&state, "fabric-a", 42, 1, "MT:REKEY-SECRET").unwrap();
        save_setup_payload(&state, "fabric-a", 42, 2, "MT:REKEY-SECRET").unwrap();

        assert!(load_setup_payload(&state, "fabric-a", "matter-42")
            .unwrap()
            .is_none());
        assert!(load_setup_payload(&state, "fabric-a", "matter-42-2")
            .unwrap()
            .is_some());
        assert_eq!(
            find_setup_payload_targets(&state, "fabric-a", "MT:REKEY-SECRET").unwrap(),
            vec![SetupPayloadTarget {
                node_id: 42,
                endpoint: 2,
            }]
        );

        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn deleting_node_removes_all_endpoints_and_deletes_empty_store() {
        let (state, path) = state_with_storage("delete");
        save_setup_payload(&state, "fabric-a", 42, 1, "11111111111").unwrap();
        save_setup_payload(&state, "fabric-a", 42, 2, "22222222222").unwrap();
        delete_setup_payloads_for_node(&state, "fabric-a", 42).unwrap();

        assert!(load_setup_payload(&state, "fabric-a", "matter-42")
            .unwrap()
            .is_none());
        assert!(!path.join(STORE_PATH).exists());
        std::fs::remove_dir_all(path).ok();
    }

    #[test]
    fn explicit_secret_backup_includes_store_but_redacted_backup_does_not() {
        let (state, path) = state_with_storage("backup");
        save_setup_payload(&state, "fabric-a", 42, 1, "MT:BACKUP-SECRET").unwrap();
        let storage = state.lock().unwrap().storage.clone().unwrap();

        assert!(storage
            .load_integration_backup_files(false)
            .unwrap()
            .is_empty());
        let files = storage.load_integration_backup_files(true).unwrap();
        let recovery = files
            .iter()
            .find(|file| file.path == STORE_PATH)
            .expect("secret backup should include recovery store");
        assert!(recovery.secret);
        assert!(recovery.content.contains("MT:BACKUP-SECRET"));

        let (restored_state, restored_path) = state_with_storage("backup-restored");
        let restored_storage = restored_state.lock().unwrap().storage.clone().unwrap();
        restored_storage
            .restore_integration_backup_files(&files)
            .unwrap();
        let restored = load_setup_payload(&restored_state, "fabric-a", "matter-42")
            .unwrap()
            .unwrap();
        assert_eq!(restored.setup_payload, "MT:BACKUP-SECRET");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(restored_path.join(STORE_PATH))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        std::fs::remove_dir_all(path).ok();
        std::fs::remove_dir_all(restored_path).ok();
    }
}
