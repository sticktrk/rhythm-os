//! Versioned local-BLE device profiles.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::OnceLock;

use anyhow::{bail, Context, Result};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_core::ButtonAction;
use rhythm_os::hub::ParsedDeviceProfileId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const OREIN_OC02001_PROFILE_ID: &str = "orein.oc02001.button.v1";
pub const OREIN_ADVERTISEMENT_SERVICE_UUID: &str = "00001511-0000-1000-8000-00805f9b34fb";
pub const OREIN_PRIMARY_GATT_SERVICE_UUID: &str = "00001000-1115-1000-4c44-5341524e4f4f";
pub const OREIN_ALTERNATE_GATT_SERVICE_UUID: &str = "f000ffc0-0451-4000-b000-000000000000";
pub const OREIN_MANUFACTURER_ID: u16 = 0x1511;
pub const OREIN_PRESS_PAYLOAD_LEN: usize = 19;

/// Defensive bounds at the transport/profile boundary. They comfortably
/// contain a complete extended BLE advertisement while preventing a malformed
/// BlueZ property set or future fixture from becoming unbounded profile input.
pub const MAX_ADVERTISEMENT_SERVICE_UUIDS: usize = 32;
pub const MAX_ADVERTISEMENT_SERVICE_DATA_ENTRIES: usize = 32;
pub const MAX_ADVERTISEMENT_MANUFACTURER_DATA_ENTRIES: usize = 32;
pub const MAX_ADVERTISEMENT_ELEMENT_BYTES: usize = 512;
pub const MAX_ADVERTISEMENT_TOTAL_DATA_BYTES: usize = 2_048;
pub const MAX_PROFILE_EVENTS_PER_ADVERTISEMENT: usize = 16;
pub const MAX_GATT_IDENTITY_SERVICES: usize = 16;

const QR_PREFIX: &str = "B:";
const QR_SERIAL_SEPARATOR: &str = "%G$S:";
const QR_MODEL_SEPARATOR: &str = "$M:";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleProfileDescriptor {
    pub id: &'static str,
    /// Older IDs whose setup, identity, event, and persisted replay contracts
    /// this active decoder can safely consume. Only the current `id` is stored
    /// or submitted after resolution.
    pub compatible_profile_ids: Vec<&'static str>,
    pub device_type: &'static str,
    pub display_name: &'static str,
    pub onboarding_methods: Vec<&'static str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ValidatedBleSetup {
    pub stable_identity: String,
    pub metadata: BTreeMap<String, String>,
}

impl ValidatedBleSetup {
    pub fn stable_identity(&self) -> &str {
        &self.stable_identity
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BleNormalizedEvent {
    Button {
        endpoint_suffix: String,
        action: ButtonAction,
    },
    Motion {
        detected: bool,
    },
    Contact {
        open: bool,
    },
}

/// Profile-owned replay evidence for one independent logical event stream.
///
/// Values are opaque to the host and bounded by the store. A multi-control
/// remote may use one stream per endpoint, while a counterless protocol may
/// omit replay evidence and accept its unavoidable at-least-once semantics.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleReplayToken {
    pub stream: String,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleProfileEvent {
    pub normalized: BleNormalizedEvent,
    pub replay: Option<BleReplayToken>,
}

/// Owned, transport-neutral snapshot of every profile-relevant field in one
/// advertisement. Construction is fallible so profiles never receive an
/// unbounded or ambiguous collection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleAdvertisement {
    service_uuids: BTreeSet<Uuid>,
    service_data: BTreeMap<Uuid, Vec<u8>>,
    manufacturer_data: BTreeMap<u16, Vec<u8>>,
}

impl BleAdvertisement {
    pub fn try_from_parts(
        service_uuids: impl IntoIterator<Item = Uuid>,
        service_data: impl IntoIterator<Item = (Uuid, Vec<u8>)>,
        manufacturer_data: impl IntoIterator<Item = (u16, Vec<u8>)>,
    ) -> Result<Self> {
        let mut bounded_service_uuids = BTreeSet::new();
        for service_uuid in service_uuids {
            if !bounded_service_uuids.insert(service_uuid) {
                bail!("duplicate Bluetooth advertisement service UUID");
            }
            if bounded_service_uuids.len() > MAX_ADVERTISEMENT_SERVICE_UUIDS {
                bail!("too many Bluetooth advertisement service UUIDs");
            }
        }

        let mut total_data_bytes = 0_usize;
        let mut bounded_service_data = BTreeMap::new();
        for (service_uuid, payload) in service_data {
            validate_advertisement_element(payload.len(), &mut total_data_bytes)?;
            if bounded_service_data.insert(service_uuid, payload).is_some() {
                bail!("duplicate Bluetooth service-data entry");
            }
            if bounded_service_data.len() > MAX_ADVERTISEMENT_SERVICE_DATA_ENTRIES {
                bail!("too many Bluetooth service-data entries");
            }
        }

        let mut bounded_manufacturer_data = BTreeMap::new();
        for (manufacturer_id, payload) in manufacturer_data {
            validate_advertisement_element(payload.len(), &mut total_data_bytes)?;
            if bounded_manufacturer_data
                .insert(manufacturer_id, payload)
                .is_some()
            {
                bail!("duplicate Bluetooth manufacturer-data entry");
            }
            if bounded_manufacturer_data.len() > MAX_ADVERTISEMENT_MANUFACTURER_DATA_ENTRIES {
                bail!("too many Bluetooth manufacturer-data entries");
            }
        }

        Ok(Self {
            service_uuids: bounded_service_uuids,
            service_data: bounded_service_data,
            manufacturer_data: bounded_manufacturer_data,
        })
    }

    pub fn empty() -> Self {
        Self {
            service_uuids: BTreeSet::new(),
            service_data: BTreeMap::new(),
            manufacturer_data: BTreeMap::new(),
        }
    }

    pub fn view(&self) -> BleAdvertisementView<'_> {
        BleAdvertisementView {
            service_uuids: &self.service_uuids,
            service_data: &self.service_data,
            manufacturer_data: &self.manufacturer_data,
        }
    }
}

fn validate_advertisement_element(len: usize, total_data_bytes: &mut usize) -> Result<()> {
    if len > MAX_ADVERTISEMENT_ELEMENT_BYTES {
        bail!("Bluetooth advertisement element is too large");
    }
    *total_data_bytes = total_data_bytes
        .checked_add(len)
        .ok_or_else(|| anyhow::anyhow!("Bluetooth advertisement size overflow"))?;
    if *total_data_bytes > MAX_ADVERTISEMENT_TOTAL_DATA_BYTES {
        bail!("Bluetooth advertisement data is too large");
    }
    Ok(())
}

/// Borrowed profile view over a validated advertisement snapshot.
#[derive(Clone, Copy, Debug)]
pub struct BleAdvertisementView<'a> {
    service_uuids: &'a BTreeSet<Uuid>,
    service_data: &'a BTreeMap<Uuid, Vec<u8>>,
    manufacturer_data: &'a BTreeMap<u16, Vec<u8>>,
}

impl<'a> BleAdvertisementView<'a> {
    pub fn advertises_service(self, service_uuid: &str) -> bool {
        Uuid::parse_str(service_uuid)
            .ok()
            .is_some_and(|service_uuid| self.service_uuids.contains(&service_uuid))
    }

    pub fn service_data(self, service_uuid: &str) -> Option<&'a [u8]> {
        let service_uuid = Uuid::parse_str(service_uuid).ok()?;
        self.service_data.get(&service_uuid).map(Vec::as_slice)
    }

    pub fn manufacturer_data(self, manufacturer_id: u16) -> Option<&'a [u8]> {
        self.manufacturer_data
            .get(&manufacturer_id)
            .map(Vec::as_slice)
    }

    pub fn service_uuids(self) -> impl Iterator<Item = &'a Uuid> {
        self.service_uuids.iter()
    }

    pub fn service_data_entries(self) -> impl Iterator<Item = (&'a Uuid, &'a [u8])> {
        self.service_data
            .iter()
            .map(|(service_uuid, payload)| (service_uuid, payload.as_slice()))
    }

    pub fn manufacturer_data_entries(self) -> impl Iterator<Item = (u16, &'a [u8])> {
        self.manufacturer_data
            .iter()
            .map(|(manufacturer_id, payload)| (*manufacturer_id, payload.as_slice()))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleButtonEndpointProjection {
    pub suffix: &'static str,
    pub control_id: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BleProfileProjection {
    pub device_type: DeviceType,
    pub display_name: &'static str,
    pub manufacturer: Option<&'static str>,
    pub model: Option<&'static str>,
    /// Every declared button endpoint. Motion/contact profiles leave this
    /// empty; multi-control remotes register one entry per physical control.
    pub button_endpoints: Vec<BleButtonEndpointProjection>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReplayOrder {
    Forward,
    Duplicate,
    Stale,
}

/// The additional proof, if any, required after setup and advertisement
/// identity match. Non-connectable devices can remain advertisement-only;
/// connectable profiles declare the exact GATT services accepted as proof.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BleProfileAdmission {
    AdvertisementOnly,
    GattServiceProof {
        service_uuids: &'static [&'static str],
    },
}

impl BleProfileAdmission {
    pub fn accepts_gatt_service(self, service_uuid: &str) -> bool {
        match self {
            Self::AdvertisementOnly => false,
            Self::GattServiceProof { service_uuids } => service_uuids
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(service_uuid)),
        }
    }
}

/// Reproducible contract for a lightweight local-BLE event device family.
///
/// Profiles own setup validation, stable identity evidence, explicit
/// advertisement-only or GATT-service admission, full-advertisement decoding,
/// normalized actions, and replay ordering. A profile may derive identity and
/// zero or more events from service data, manufacturer data, or their
/// combination. It does not own a runtime, scanner, persistence engine,
/// lifecycle, or public hub type. Stateful GATT controllers such as Hue bulbs
/// remain rich drivers and share only the adapter/runtime boundary; this
/// event-profile host does not model them as declarative devices.
pub trait BleDeviceProfile: Sync {
    fn descriptor(&self) -> BleProfileDescriptor;
    fn projection(&self) -> BleProfileProjection;
    fn admission(&self) -> BleProfileAdmission;
    fn parse_pairing_setup(&self, value: &serde_json::Value) -> Result<ValidatedBleSetup>;
    fn stable_identity_from_advertisement(
        &self,
        advertisement: BleAdvertisementView<'_>,
    ) -> Option<String>;
    fn decode_events(&self, advertisement: BleAdvertisementView<'_>) -> Vec<BleProfileEvent>;
    fn replay_order(&self, stream: &str, previous: &[u8], candidate: &[u8]) -> ReplayOrder;
}

/// Invoke a profile decoder while enforcing a host-owned event fan-out bound.
pub fn decode_profile_events(
    profile: &dyn BleDeviceProfile,
    advertisement: BleAdvertisementView<'_>,
) -> Result<Vec<BleProfileEvent>> {
    let events = profile.decode_events(advertisement);
    if events.len() > MAX_PROFILE_EVENTS_PER_ADVERTISEMENT {
        bail!("local Bluetooth profile decoded too many events");
    }
    Ok(events)
}

pub struct OreinOc02001ButtonProfile;

pub static OREIN_OC02001_BUTTON_PROFILE: OreinOc02001ButtonProfile = OreinOc02001ButtonProfile;
static REGISTERED_PROFILES: [&'static dyn BleDeviceProfile; 1] = [&OREIN_OC02001_BUTTON_PROFILE];
static PROFILE_REGISTRY: BleProfileRegistry<'static> =
    BleProfileRegistry::new(&REGISTERED_PROFILES);
static PROFILE_REGISTRY_VALIDATION: OnceLock<std::result::Result<(), String>> = OnceLock::new();

/// Compile-time registry for lightweight BLE event profiles.
///
/// Keeping lookup independent from the process runtime makes adding a profile
/// a data-only registration step: implement the protocol contract, add one
/// entry to `REGISTERED_PROFILES`, and replay its conformance fixture.
#[derive(Clone, Copy)]
pub struct BleProfileRegistry<'a> {
    profiles: &'a [&'a dyn BleDeviceProfile],
}

impl<'a> BleProfileRegistry<'a> {
    pub const fn new(profiles: &'a [&'a dyn BleDeviceProfile]) -> Self {
        Self { profiles }
    }

    pub fn profiles(self) -> &'a [&'a dyn BleDeviceProfile] {
        self.profiles
    }

    pub fn find(self, id: &str) -> Option<&'a dyn BleDeviceProfile> {
        self.profiles.iter().copied().find(|profile| {
            let descriptor = profile.descriptor();
            descriptor.id == id || descriptor.compatible_profile_ids.contains(&id)
        })
    }

    pub fn canonical_id(self, id: &str) -> Option<&'static str> {
        self.find(id).map(|profile| profile.descriptor().id)
    }

    pub fn validate(self) -> Result<()> {
        let mut profile_ids = BTreeSet::new();
        let mut active_families = BTreeSet::new();
        for profile in self.profiles {
            let descriptor = profile.descriptor();
            let Some(current_id) = ParsedDeviceProfileId::parse(descriptor.id) else {
                bail!("invalid local Bluetooth profile ID: {}", descriptor.id);
            };
            if !active_families.insert(current_id.family()) {
                bail!(
                    "multiple active local Bluetooth decoders for profile family: {}",
                    current_id.family()
                );
            }
            if !profile_ids.insert(descriptor.id) {
                bail!("duplicate local Bluetooth profile ID: {}", descriptor.id);
            }
            for compatible_id in &descriptor.compatible_profile_ids {
                let Some(compatible_id_parts) = ParsedDeviceProfileId::parse(compatible_id) else {
                    bail!("invalid compatible local Bluetooth profile ID: {compatible_id}");
                };
                if compatible_id_parts.family() != current_id.family()
                    || compatible_id_parts.version() >= current_id.version()
                {
                    bail!(
                        "local Bluetooth profile {} has incompatible version lineage: {}",
                        descriptor.id,
                        compatible_id
                    );
                }
                if !profile_ids.insert(*compatible_id) {
                    bail!(
                        "duplicate local Bluetooth current/compatible profile ID: {compatible_id}"
                    );
                }
            }
            if descriptor.display_name.trim().is_empty() {
                bail!("local Bluetooth profile has an empty display name");
            }
            if descriptor.onboarding_methods.is_empty()
                || descriptor
                    .onboarding_methods
                    .iter()
                    .any(|method| !valid_registry_token(method, 64))
            {
                bail!(
                    "local Bluetooth profile {} has invalid onboarding methods",
                    descriptor.id
                );
            }

            let projection = profile.projection();
            if projection.device_type == DeviceType::Light {
                bail!(
                    "local Bluetooth event profile {} cannot declare a controllable light",
                    descriptor.id
                );
            }
            if descriptor.device_type != projection_device_type(&projection.device_type)
                || descriptor.display_name != projection.display_name
            {
                bail!(
                    "local Bluetooth profile {} descriptor/projection mismatch",
                    descriptor.id
                );
            }

            let mut endpoint_suffixes = BTreeSet::new();
            let mut control_ids = BTreeSet::new();
            for endpoint in &projection.button_endpoints {
                if !valid_registry_token(endpoint.suffix, 64) || endpoint.control_id == 0 {
                    bail!(
                        "local Bluetooth profile {} has an invalid button endpoint",
                        descriptor.id
                    );
                }
                if !endpoint_suffixes.insert(endpoint.suffix.to_ascii_lowercase())
                    || !control_ids.insert(endpoint.control_id)
                {
                    bail!(
                        "local Bluetooth profile {} has duplicate button endpoints",
                        descriptor.id
                    );
                }
            }
            match projection.device_type {
                DeviceType::Button if projection.button_endpoints.is_empty() => bail!(
                    "local Bluetooth button profile {} declares no endpoints",
                    descriptor.id
                ),
                DeviceType::Button => {}
                _ if !projection.button_endpoints.is_empty() => bail!(
                    "non-button local Bluetooth profile {} declares button endpoints",
                    descriptor.id
                ),
                _ => {}
            }

            match profile.admission() {
                BleProfileAdmission::AdvertisementOnly => {}
                BleProfileAdmission::GattServiceProof { service_uuids } => {
                    if service_uuids.is_empty() || service_uuids.len() > MAX_GATT_IDENTITY_SERVICES
                    {
                        bail!(
                            "local Bluetooth profile {} has invalid GATT identity evidence",
                            descriptor.id
                        );
                    }
                    let mut parsed = BTreeSet::new();
                    for service_uuid in service_uuids {
                        let service_uuid = Uuid::parse_str(service_uuid).with_context(|| {
                            format!(
                                "local Bluetooth profile {} has an invalid GATT service UUID",
                                descriptor.id
                            )
                        })?;
                        if !parsed.insert(service_uuid) {
                            bail!(
                                "local Bluetooth profile {} has duplicate GATT service evidence",
                                descriptor.id
                            );
                        }
                    }
                }
            }
        }
        Ok(())
    }
}

pub fn profiles() -> &'static [&'static dyn BleDeviceProfile] {
    ensure_registered_profiles_valid();
    PROFILE_REGISTRY.profiles()
}

pub fn profile_by_id(id: &str) -> Option<&'static dyn BleDeviceProfile> {
    ensure_registered_profiles_valid();
    PROFILE_REGISTRY.find(id)
}

/// Resolve a registered current or explicitly compatible profile ID to the
/// one current decoder/storage ID for its family.
pub fn canonical_profile_id(id: &str) -> Option<&'static str> {
    ensure_registered_profiles_valid();
    PROFILE_REGISTRY.canonical_id(id)
}

fn ensure_registered_profiles_valid() {
    let validation = PROFILE_REGISTRY_VALIDATION.get_or_init(|| {
        PROFILE_REGISTRY
            .validate()
            .map_err(|error| error.to_string())
    });
    if let Err(error) = validation {
        panic!("invalid local Bluetooth profile registry: {error}");
    }
}

fn projection_device_type(device_type: &DeviceType) -> &'static str {
    match device_type {
        DeviceType::Light => "light",
        DeviceType::Button => "button",
        DeviceType::Motion => "motion",
        DeviceType::Contact => "contact",
    }
}

fn valid_registry_token(value: &str, max_len: usize) -> bool {
    !value.is_empty()
        && value.len() <= max_len
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

impl BleDeviceProfile for OreinOc02001ButtonProfile {
    fn descriptor(&self) -> BleProfileDescriptor {
        BleProfileDescriptor {
            id: OREIN_OC02001_PROFILE_ID,
            compatible_profile_ids: Vec::new(),
            device_type: "button",
            display_name: "Button",
            onboarding_methods: vec![rhythm_os::hub::DEVICE_ONBOARDING_METHOD_LOCAL_BLE_QR],
        }
    }

    fn projection(&self) -> BleProfileProjection {
        BleProfileProjection {
            device_type: DeviceType::Button,
            display_name: "Button",
            manufacturer: Some("Orein/AiDot"),
            model: Some("OC02001-CR-B"),
            button_endpoints: vec![BleButtonEndpointProjection {
                suffix: "button-1",
                control_id: 1,
            }],
        }
    }

    fn admission(&self) -> BleProfileAdmission {
        BleProfileAdmission::GattServiceProof {
            service_uuids: &[
                OREIN_PRIMARY_GATT_SERVICE_UUID,
                OREIN_ALTERNATE_GATT_SERVICE_UUID,
            ],
        }
    }

    fn parse_pairing_setup(&self, value: &serde_json::Value) -> Result<ValidatedBleSetup> {
        let setup = OreinOc02001Setup::from_pairing_value(value)?;
        Ok(ValidatedBleSetup {
            stable_identity: setup.ble_identity,
            metadata: BTreeMap::from([
                ("serial".to_string(), setup.serial_metadata),
                ("model".to_string(), setup.model_metadata),
            ]),
        })
    }

    fn stable_identity_from_advertisement(
        &self,
        advertisement: BleAdvertisementView<'_>,
    ) -> Option<String> {
        advertisement
            .service_data(OREIN_ADVERTISEMENT_SERVICE_UUID)
            .and_then(orein_service_data_identity)
    }

    fn decode_events(&self, advertisement: BleAdvertisementView<'_>) -> Vec<BleProfileEvent> {
        advertisement
            .manufacturer_data(OREIN_MANUFACTURER_ID)
            .and_then(orein_press_counter)
            .map(|counter| {
                vec![BleProfileEvent {
                    normalized: BleNormalizedEvent::Button {
                        endpoint_suffix: "button-1".to_string(),
                        action: ButtonAction::OnPress,
                    },
                    replay: Some(BleReplayToken {
                        stream: "press".to_string(),
                        value: vec![counter],
                    }),
                }]
            })
            .unwrap_or_default()
    }

    fn replay_order(&self, stream: &str, previous: &[u8], candidate: &[u8]) -> ReplayOrder {
        let ("press", [previous], [candidate]) = (stream, previous, candidate) else {
            return ReplayOrder::Stale;
        };
        if candidate == previous {
            ReplayOrder::Duplicate
        } else {
            // Bench evidence establishes only that a changed byte denotes a
            // new press. It does not establish signed ordering or a maximum
            // missed-event window, so every different value (including a
            // wrap or large gap) is forward evidence for this profile.
            ReplayOrder::Forward
        }
    }
}

/// Privacy-bounded setup fields produced by a profile parser.
///
/// The raw QR payload is deliberately absent. The appliance validates this
/// structure again instead of trusting the app-side classification.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OreinOc02001Setup {
    pub ble_identity: String,
    pub serial_metadata: String,
    pub model_metadata: String,
}

impl OreinOc02001Setup {
    pub fn parse_qr(value: &str) -> Result<Self> {
        let payload = value
            .strip_prefix(QR_PREFIX)
            .ok_or_else(|| anyhow::anyhow!("not a supported local Bluetooth QR code"))?;
        let (identity, metadata) = payload
            .split_once(QR_SERIAL_SEPARATOR)
            .ok_or_else(|| anyhow::anyhow!("invalid local Bluetooth QR fields"))?;
        let (serial_metadata, model_metadata) = metadata
            .split_once(QR_MODEL_SEPARATOR)
            .ok_or_else(|| anyhow::anyhow!("invalid local Bluetooth QR metadata"))?;
        if model_metadata.contains(QR_MODEL_SEPARATOR)
            || serial_metadata.contains(QR_SERIAL_SEPARATOR)
        {
            bail!("invalid local Bluetooth QR metadata");
        }
        Self::validated(identity, serial_metadata, model_metadata)
    }

    pub fn from_pairing_value(value: &serde_json::Value) -> Result<Self> {
        let setup = serde_json::from_value::<Self>(value.clone())
            .context("invalid local Bluetooth setup fields")?;
        Self::validated(
            &setup.ble_identity,
            &setup.serial_metadata,
            &setup.model_metadata,
        )
    }

    fn validated(identity: &str, serial_metadata: &str, model_metadata: &str) -> Result<Self> {
        if identity.len() != 12 || !identity.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            bail!("invalid local Bluetooth identity");
        }
        validate_metadata("serial", serial_metadata)?;
        validate_metadata("model", model_metadata)?;
        Ok(Self {
            ble_identity: identity.to_ascii_uppercase(),
            serial_metadata: serial_metadata.to_string(),
            model_metadata: model_metadata.to_string(),
        })
    }

    pub fn identity_bytes(&self) -> [u8; 6] {
        let mut bytes = [0_u8; 6];
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.ble_identity[index * 2..index * 2 + 2], 16)
                .expect("validated local-BLE identity is hexadecimal");
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
        bail!("invalid local Bluetooth {label} metadata");
    }
    Ok(())
}

/// Extract the stable QR identity mirrored by Orein service data.
pub fn orein_service_data_identity(payload: &[u8]) -> Option<String> {
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

/// Return the rolling press counter from a validated Orein advertisement.
pub fn orein_press_counter(payload: &[u8]) -> Option<u8> {
    (payload.len() == OREIN_PRESS_PAYLOAD_LEN && payload[1] == 0x02 && payload[2] == 0x01)
        .then_some(payload[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    const SYNTHETIC_QR: &str = "B:0A0B0C0D0E0F%G$S:SYNTHETIC000001$M:TESTMODEL001";

    #[test]
    fn parses_synthetic_qr_without_retaining_raw_payload() {
        let parsed = OreinOc02001Setup::parse_qr(SYNTHETIC_QR).unwrap();
        assert_eq!(parsed.ble_identity, "0A0B0C0D0E0F");
        assert_eq!(parsed.serial_metadata, "SYNTHETIC000001");
        assert_eq!(parsed.model_metadata, "TESTMODEL001");
        let serialized = serde_json::to_string(&parsed).unwrap();
        assert!(!serialized.contains("%G$"));
        assert!(!serialized.contains("B:"));
    }

    #[test]
    fn pairing_shape_is_strict_and_revalidated() {
        let setup = OreinOc02001Setup::from_pairing_value(&serde_json::json!({
            "ble_identity": "0a0b0c0d0e0f",
            "serial_metadata": "SYNTHETIC000001",
            "model_metadata": "TESTMODEL001"
        }))
        .unwrap();
        assert_eq!(setup.ble_identity, "0A0B0C0D0E0F");
        assert!(OreinOc02001Setup::from_pairing_value(&serde_json::json!({
            "ble_identity": "0A0B0C0D0E0F",
            "serial_metadata": "ok",
            "model_metadata": "ok",
            "raw_qr": SYNTHETIC_QR
        }))
        .is_err());
    }

    #[test]
    fn rejects_near_miss_qr_shapes_and_collision_candidates() {
        for value in [
            "B:0A0B0C0D0E0%G$S:SYNTHETIC000001$M:TESTMODEL001",
            "B:0A0B0C0D0E0G%G$S:SYNTHETIC000001$M:TESTMODEL001",
            "B:0A0B0C0D0E0F$S:SYNTHETIC000001$M:TESTMODEL001",
            "B:0A0B0C0D0E0F%G$S:$M:TESTMODEL001",
            "B:0A0B0C0D0E0F%G$S:SYNTHETIC000001$M:TESTMODEL001$M:EXTRA",
            " B:0A0B0C0D0E0F%G$S:SYNTHETIC000001$M:TESTMODEL001",
            "MT:Y.K9042C00KA0648G00",
        ] {
            assert!(
                OreinOc02001Setup::parse_qr(value).is_err(),
                "accepted {value}"
            );
        }
    }

    #[test]
    fn extracts_service_identity_only_from_expected_frame_shape() {
        let payload = [
            0x00, 0x50, 0x00, 0x15, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff,
        ];
        assert_eq!(
            orein_service_data_identity(&payload).as_deref(),
            Some("0A0B0C0D0E0F")
        );
        assert_eq!(orein_service_data_identity(&payload[..9]), None);
        let mut wrong_prefix = payload;
        wrong_prefix[3] = 0x16;
        assert_eq!(orein_service_data_identity(&wrong_prefix), None);
    }

    #[test]
    fn press_frame_requires_expected_length_and_marker() {
        let mut payload = [0_u8; OREIN_PRESS_PAYLOAD_LEN];
        payload[..3].copy_from_slice(&[0x85, 0x02, 0x01]);
        assert_eq!(orein_press_counter(&payload), Some(0x85));
        assert_eq!(orein_press_counter(&payload[..18]), None);
        payload[2] = 0x02;
        assert_eq!(orein_press_counter(&payload), None);
    }

    #[test]
    fn registry_exposes_a_complete_vendor_neutral_profile_contract() {
        PROFILE_REGISTRY.validate().unwrap();
        let profile = profile_by_id(OREIN_OC02001_PROFILE_ID).unwrap();
        let descriptor = profile.descriptor();
        assert_eq!(descriptor.device_type, "button");
        assert_eq!(descriptor.display_name, "Button");
        assert_eq!(descriptor.onboarding_methods, vec!["local_ble_qr"]);
        assert!(profile
            .admission()
            .accepts_gatt_service(OREIN_PRIMARY_GATT_SERVICE_UUID));
        assert_eq!(
            profile.replay_order("press", &[0xff], &[0x00]),
            ReplayOrder::Forward
        );
        assert_eq!(
            profile.replay_order("press", &[0x00], &[0xff]),
            ReplayOrder::Forward
        );
    }

    const SYNTHETIC_MOTION_SERVICE_UUID: &str = "00000001-0000-1000-8000-00805f9b34fb";
    const SYNTHETIC_SECONDARY_MOTION_SERVICE_UUID: &str = "00000003-0000-1000-8000-00805f9b34fb";
    const SYNTHETIC_IDENTITY_MANUFACTURER_ID: u16 = 0x1234;

    #[derive(Clone)]
    struct SyntheticMotionProfile {
        descriptor: BleProfileDescriptor,
        projection: BleProfileProjection,
        admission: BleProfileAdmission,
    }

    impl SyntheticMotionProfile {
        fn valid() -> Self {
            Self {
                descriptor: BleProfileDescriptor {
                    id: "example.motion.v1",
                    compatible_profile_ids: Vec::new(),
                    device_type: "motion",
                    display_name: "Motion sensor",
                    onboarding_methods: vec!["local_ble_qr"],
                },
                projection: BleProfileProjection {
                    device_type: DeviceType::Motion,
                    display_name: "Motion sensor",
                    manufacturer: None,
                    model: None,
                    button_endpoints: Vec::new(),
                },
                admission: BleProfileAdmission::AdvertisementOnly,
            }
        }
    }

    impl BleDeviceProfile for SyntheticMotionProfile {
        fn descriptor(&self) -> BleProfileDescriptor {
            self.descriptor.clone()
        }

        fn projection(&self) -> BleProfileProjection {
            self.projection.clone()
        }

        fn admission(&self) -> BleProfileAdmission {
            self.admission
        }

        fn parse_pairing_setup(&self, value: &serde_json::Value) -> Result<ValidatedBleSetup> {
            let stable_identity = value
                .get("identity")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.is_empty() && value.len() <= 32)
                .ok_or_else(|| anyhow::anyhow!("invalid synthetic identity"))?;
            Ok(ValidatedBleSetup {
                stable_identity: stable_identity.to_string(),
                metadata: BTreeMap::new(),
            })
        }

        fn stable_identity_from_advertisement(
            &self,
            advertisement: BleAdvertisementView<'_>,
        ) -> Option<String> {
            let payload = advertisement.manufacturer_data(SYNTHETIC_IDENTITY_MANUFACTURER_ID)?;
            (payload.len() == 2).then(|| format!("{:02X}{:02X}", payload[0], payload[1]))
        }

        fn decode_events(&self, advertisement: BleAdvertisementView<'_>) -> Vec<BleProfileEvent> {
            [
                (SYNTHETIC_MOTION_SERVICE_UUID, "motion-primary"),
                (SYNTHETIC_SECONDARY_MOTION_SERVICE_UUID, "motion-secondary"),
            ]
            .into_iter()
            .filter_map(|(service_uuid, stream)| {
                let payload = advertisement
                    .service_data(service_uuid)
                    .filter(|payload| payload.len() == 3)?;
                Some(BleProfileEvent {
                    normalized: BleNormalizedEvent::Motion {
                        detected: payload[2] != 0,
                    },
                    replay: Some(BleReplayToken {
                        stream: stream.to_string(),
                        // A deliberately wider token proves the host does
                        // not prescribe Orein's rolling-counter shape.
                        value: payload[..2].to_vec(),
                    }),
                })
            })
            .collect()
        }

        fn replay_order(&self, stream: &str, previous: &[u8], candidate: &[u8]) -> ReplayOrder {
            if !matches!(stream, "motion-primary" | "motion-secondary")
                || previous.len() != 2
                || candidate.len() != 2
            {
                return ReplayOrder::Stale;
            }
            match u16::from_be_bytes([candidate[0], candidate[1]])
                .wrapping_sub(u16::from_be_bytes([previous[0], previous[1]]))
            {
                0 => ReplayOrder::Duplicate,
                1..=0x7fff => ReplayOrder::Forward,
                _ => ReplayOrder::Stale,
            }
        }
    }

    #[test]
    fn adding_a_second_profile_uses_the_same_registry_and_host_contract() {
        let synthetic = SyntheticMotionProfile::valid();
        let entries: [&dyn BleDeviceProfile; 2] = [&OREIN_OC02001_BUTTON_PROFILE, &synthetic];
        let registry = BleProfileRegistry::new(&entries);
        registry.validate().unwrap();

        let profile = registry.find("example.motion.v1").unwrap();
        let setup = profile
            .parse_pairing_setup(&serde_json::json!({ "identity": "A1B2" }))
            .unwrap();
        assert_eq!(setup.stable_identity(), "A1B2");
        assert_eq!(profile.projection().device_type, DeviceType::Motion);
        assert_eq!(profile.admission(), BleProfileAdmission::AdvertisementOnly);
        let service_uuid = Uuid::parse_str(SYNTHETIC_MOTION_SERVICE_UUID).unwrap();
        let secondary_service_uuid =
            Uuid::parse_str(SYNTHETIC_SECONDARY_MOTION_SERVICE_UUID).unwrap();
        let advertisement = BleAdvertisement::try_from_parts(
            [service_uuid, secondary_service_uuid],
            [
                (service_uuid, vec![0x01, 0x07, 1]),
                (secondary_service_uuid, vec![0x01, 0x08, 0]),
            ],
            [(SYNTHETIC_IDENTITY_MANUFACTURER_ID, vec![0xA1, 0xB2])],
        )
        .unwrap();
        assert_eq!(
            profile.stable_identity_from_advertisement(advertisement.view()),
            Some("A1B2".to_string())
        );
        assert_eq!(
            profile.decode_events(advertisement.view()),
            vec![
                BleProfileEvent {
                    normalized: BleNormalizedEvent::Motion { detected: true },
                    replay: Some(BleReplayToken {
                        stream: "motion-primary".to_string(),
                        value: vec![0x01, 0x07],
                    }),
                },
                BleProfileEvent {
                    normalized: BleNormalizedEvent::Motion { detected: false },
                    replay: Some(BleReplayToken {
                        stream: "motion-secondary".to_string(),
                        value: vec![0x01, 0x08],
                    }),
                }
            ]
        );
        assert!(profile
            .decode_events(BleAdvertisement::empty().view())
            .is_empty());
        assert_eq!(
            profile.replay_order("motion-primary", &[0xff, 0xff], &[0x00, 0x00]),
            ReplayOrder::Forward
        );
        assert_eq!(registry.profiles().len(), 2);
    }

    #[test]
    fn advertisement_snapshot_rejects_ambiguous_or_unbounded_input() {
        let service_uuid = Uuid::parse_str(SYNTHETIC_MOTION_SERVICE_UUID).unwrap();
        assert!(BleAdvertisement::try_from_parts(
            [],
            [(service_uuid, vec![1]), (service_uuid, vec![2]),],
            [],
        )
        .is_err());
        assert!(BleAdvertisement::try_from_parts(
            (0..=MAX_ADVERTISEMENT_SERVICE_UUIDS)
                .map(|offset| Uuid::from_u128(0x1000 + offset as u128)),
            [],
            [],
        )
        .is_err());
        assert!(BleAdvertisement::try_from_parts(
            [],
            [(service_uuid, vec![0; MAX_ADVERTISEMENT_ELEMENT_BYTES + 1])],
            [],
        )
        .is_err());
    }

    #[test]
    fn registry_rejects_duplicate_profile_ids() {
        let profile = SyntheticMotionProfile::valid();
        let entries: [&dyn BleDeviceProfile; 2] = [&profile, &profile];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());
    }

    #[test]
    fn registry_resolves_only_explicit_older_lineage_to_one_current_decoder() {
        let mut current = SyntheticMotionProfile::valid();
        current.descriptor.id = "example.motion.v3";
        current.descriptor.compatible_profile_ids = vec!["example.motion.v1", "example.motion.v2"];
        let entries: [&dyn BleDeviceProfile; 1] = [&current];
        let registry = BleProfileRegistry::new(&entries);
        registry.validate().unwrap();

        assert_eq!(
            registry.find("example.motion.v1").unwrap().descriptor().id,
            "example.motion.v3"
        );
        assert_eq!(
            registry.canonical_id("example.motion.v2"),
            Some("example.motion.v3")
        );
        assert!(registry.find("example.motion.v4").is_none());
    }

    #[test]
    fn registry_rejects_competing_current_decoders_and_invalid_lineage() {
        let first = SyntheticMotionProfile::valid();
        let mut second = SyntheticMotionProfile::valid();
        second.descriptor.id = "example.motion.v2";
        let entries: [&dyn BleDeviceProfile; 2] = [&first, &second];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut cross_family = SyntheticMotionProfile::valid();
        cross_family.descriptor.id = "example.motion.v2";
        cross_family.descriptor.compatible_profile_ids = vec!["other.motion.v1"];
        let entries: [&dyn BleDeviceProfile; 1] = [&cross_family];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut newer_alias = SyntheticMotionProfile::valid();
        newer_alias.descriptor.compatible_profile_ids = vec!["example.motion.v2"];
        let entries: [&dyn BleDeviceProfile; 1] = [&newer_alias];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut malformed = SyntheticMotionProfile::valid();
        malformed.descriptor.id = "Example.motion.v1";
        let entries: [&dyn BleDeviceProfile; 1] = [&malformed];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());
    }

    #[test]
    fn registry_rejects_empty_onboarding_and_descriptor_projection_mismatch() {
        let mut empty_onboarding = SyntheticMotionProfile::valid();
        empty_onboarding.descriptor.onboarding_methods.clear();
        let entries: [&dyn BleDeviceProfile; 1] = [&empty_onboarding];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut mismatch = SyntheticMotionProfile::valid();
        mismatch.descriptor.device_type = "contact";
        let entries: [&dyn BleDeviceProfile; 1] = [&mismatch];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut name_mismatch = SyntheticMotionProfile::valid();
        name_mismatch.descriptor.display_name = "Different";
        let entries: [&dyn BleDeviceProfile; 1] = [&name_mismatch];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());

        let mut light = SyntheticMotionProfile::valid();
        light.descriptor.device_type = "light";
        light.projection.device_type = DeviceType::Light;
        let entries: [&dyn BleDeviceProfile; 1] = [&light];
        assert!(BleProfileRegistry::new(&entries).validate().is_err());
    }

    #[test]
    fn registry_accepts_advertisement_only_and_bounds_gatt_admission_evidence() {
        let advertisement_only = SyntheticMotionProfile::valid();
        let entries: [&dyn BleDeviceProfile; 1] = [&advertisement_only];
        BleProfileRegistry::new(&entries).validate().unwrap();

        let mut valid_gatt = SyntheticMotionProfile::valid();
        valid_gatt.admission = BleProfileAdmission::GattServiceProof {
            service_uuids: &["00000002-0000-1000-8000-00805f9b34fb"],
        };
        let entries: [&dyn BleDeviceProfile; 1] = [&valid_gatt];
        BleProfileRegistry::new(&entries).validate().unwrap();

        for service_uuids in [
            &[][..],
            &["not-a-uuid"][..],
            &[
                "00000002-0000-1000-8000-00805f9b34fb",
                "00000002-0000-1000-8000-00805f9b34fb",
            ][..],
        ] {
            let mut invalid = SyntheticMotionProfile::valid();
            invalid.admission = BleProfileAdmission::GattServiceProof { service_uuids };
            let entries: [&dyn BleDeviceProfile; 1] = [&invalid];
            assert!(BleProfileRegistry::new(&entries).validate().is_err());
        }
    }

    #[test]
    fn registry_rejects_invalid_and_duplicate_button_endpoints() {
        let button_profile = |endpoints| {
            let mut profile = SyntheticMotionProfile::valid();
            profile.descriptor.device_type = "button";
            profile.projection.device_type = DeviceType::Button;
            profile.projection.button_endpoints = endpoints;
            profile
        };

        for profile in [
            button_profile(vec![BleButtonEndpointProjection {
                suffix: "invalid suffix",
                control_id: 1,
            }]),
            button_profile(vec![BleButtonEndpointProjection {
                suffix: "button-1",
                control_id: 0,
            }]),
            button_profile(vec![
                BleButtonEndpointProjection {
                    suffix: "button-1",
                    control_id: 1,
                },
                BleButtonEndpointProjection {
                    suffix: "BUTTON-1",
                    control_id: 2,
                },
            ]),
            button_profile(vec![
                BleButtonEndpointProjection {
                    suffix: "button-1",
                    control_id: 1,
                },
                BleButtonEndpointProjection {
                    suffix: "button-2",
                    control_id: 1,
                },
            ]),
        ] {
            let entries: [&dyn BleDeviceProfile; 1] = [&profile];
            assert!(BleProfileRegistry::new(&entries).validate().is_err());
        }
    }
}
