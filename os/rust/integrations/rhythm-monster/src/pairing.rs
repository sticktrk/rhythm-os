//! Staged Monster pairing contract shared by the appliance hub and the app.
//!
//! The app brokers the cloud steps, so pairing is three explicit requests on
//! the generic `api/devices/pair` route, each terminal on its own:
//!
//! 1. `discover`  – scan the shared adapter for Ayla strips and read each DSN.
//! 2. `provision` – write the cloud setup token and the appliance's Wi-Fi.
//! 3. `adopt`     – accept the cloud-issued LAN key after a signed readback.
//!
//! Every response carries `details.stage`; a failed `provision` also carries
//! `details.uncertain` so the caller never retries provisioning blindly.
use crate::{
    store::{display_name_for, LightDeviceRecord},
    types::validate_dsn,
    LightCredentials, LightSecret,
};
use anyhow::{bail, Result};
use rhythm_core::runtime::hub_registry::DeviceType;
use rhythm_os::hub::HubType;
use rhythm_os::pairing::{PairedDeviceInfo, PairingSession, PairingStatus};
use serde_json::{json, Value};

pub const HUB_TYPE: &str = HubType::MONSTER;
pub const STAGE_DISCOVER: &str = "discover";
pub const STAGE_PROVISION: &str = "provision";
pub const STAGE_ADOPT: &str = "adopt";
pub const DEFAULT_SCAN_SECS: u64 = 8;
pub const MAX_SCAN_SECS: u64 = 20;
pub const MAX_CANDIDATES: usize = 4;
pub const MANUFACTURER: &str = "Monster";
pub const DEFAULT_MODEL: &str = "xt-16ft-hw-neon-led-rgbic";

#[derive(Clone)]
pub enum LightPairingStage {
    Discover {
        scan_secs: u64,
    },
    Provision {
        dsn: String,
        address: String,
        setup_token: LightSecret,
    },
    Adopt {
        credentials: LightCredentials,
        name: Option<String>,
        model: Option<String>,
    },
}

impl LightPairingStage {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Discover { .. } => STAGE_DISCOVER,
            Self::Provision { .. } => STAGE_PROVISION,
            Self::Adopt { .. } => STAGE_ADOPT,
        }
    }
}

fn string_field(params: &Value, key: &str) -> Result<String> {
    let value = params
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing Monster pairing field '{key}'"))?;
    Ok(value.to_string())
}

pub fn is_bluetooth_address(value: &str) -> bool {
    let parts: Vec<&str> = value.split(':').collect();
    parts.len() == 6
        && parts
            .iter()
            .all(|part| part.len() == 2 && part.bytes().all(|b| b.is_ascii_hexdigit()))
}

pub fn parse_stage(params: &Value) -> Result<LightPairingStage> {
    let stage = string_field(params, "stage")?;
    match stage.as_str() {
        STAGE_DISCOVER => {
            let scan_secs = params
                .get("scan_secs")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_SCAN_SECS)
                .clamp(1, MAX_SCAN_SECS);
            Ok(LightPairingStage::Discover { scan_secs })
        }
        STAGE_PROVISION => {
            let dsn = string_field(params, "dsn")?;
            validate_dsn(&dsn).map_err(|_| anyhow::anyhow!("invalid Monster DSN"))?;
            let address = string_field(params, "address")?.to_ascii_uppercase();
            if !is_bluetooth_address(&address) {
                bail!("invalid Monster Bluetooth address");
            }
            let setup_token = string_field(params, "setup_token")?;
            if setup_token.len() != 32 || !setup_token.bytes().all(|b| b.is_ascii_hexdigit()) {
                bail!("invalid Monster setup token");
            }
            Ok(LightPairingStage::Provision {
                dsn,
                address,
                setup_token: LightSecret::new(setup_token),
            })
        }
        STAGE_ADOPT => {
            let credentials: LightCredentials = serde_json::from_value(params.clone())
                .map_err(|_| anyhow::anyhow!("invalid Monster LAN credentials"))?;
            credentials
                .validate()
                .map_err(|error| anyhow::anyhow!("invalid Monster LAN credentials: {error}"))?;
            let name = params
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(|value| value.chars().take(64).collect::<String>());
            let model = params
                .get("model")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty() && value.len() <= 64)
                .map(str::to_string);
            Ok(LightPairingStage::Adopt {
                credentials,
                name,
                model,
            })
        }
        other => bail!("unknown Monster pairing stage '{other}'"),
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiscoveredCandidate {
    pub dsn: String,
    pub address: String,
}

fn session(status: PairingStatus, details: Value) -> PairingSession {
    PairingSession {
        hub_type: HUB_TYPE.to_string(),
        status,
        device: None,
        devices: Vec::new(),
        error: None,
        failure_stage: None,
        warnings: Vec::new(),
        details: Some(details),
    }
}

pub fn discover_session(candidates: &[DiscoveredCandidate]) -> PairingSession {
    session(
        PairingStatus::Complete,
        json!({
            "stage": STAGE_DISCOVER,
            "candidates": candidates
                .iter()
                .map(|candidate| json!({"dsn": candidate.dsn, "address": candidate.address}))
                .collect::<Vec<_>>(),
        }),
    )
}

pub fn provision_session(dsn: &str) -> PairingSession {
    session(
        PairingStatus::Complete,
        json!({ "stage": STAGE_PROVISION, "dsn": dsn }),
    )
}

pub fn failed_session(stage: &str, message: impl Into<String>, uncertain: bool) -> PairingSession {
    let mut failed = session(
        PairingStatus::Failed,
        json!({ "stage": stage, "uncertain": uncertain }),
    );
    failed.error = Some(message.into());
    failed
}

pub fn paired_device_info(record: &LightDeviceRecord) -> PairedDeviceInfo {
    PairedDeviceInfo {
        device_id: record.native_id(),
        name: record.name.clone(),
        device_type: DeviceType::Light,
        manufacturer: Some(MANUFACTURER.to_string()),
        model: Some(
            record
                .model
                .clone()
                .unwrap_or_else(|| DEFAULT_MODEL.to_string()),
        ),
    }
}

pub fn adopted_session(record: &LightDeviceRecord) -> PairingSession {
    let info = paired_device_info(record);
    let mut complete = session(
        PairingStatus::Complete,
        json!({ "stage": STAGE_ADOPT, "dsn": record.dsn }),
    );
    complete.device = Some(info.clone());
    complete.devices = vec![info];
    complete
}

pub fn record_for_adoption(
    credentials: &LightCredentials,
    name: Option<&str>,
    model: Option<&str>,
    paired_at_epoch_secs: u64,
) -> LightDeviceRecord {
    LightDeviceRecord {
        dsn: credentials.dsn.clone(),
        ip: credentials.ip,
        local_key: credentials.local_key.clone(),
        local_key_id: credentials.local_key_id,
        name: display_name_for(name, &credentials.dsn),
        model: Some(model.unwrap_or(DEFAULT_MODEL).to_string()),
        paired_at_epoch_secs,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_each_stage_and_rejects_malformed_input() {
        assert!(matches!(
            parse_stage(&json!({"stage": "discover", "scan_secs": 99})).unwrap(),
            LightPairingStage::Discover {
                scan_secs: MAX_SCAN_SECS
            }
        ));
        let provision = parse_stage(&json!({
            "stage": "provision",
            "dsn": "ACFIXTURE123456",
            "address": "aa:bb:cc:dd:ee:ff",
            "setup_token": "0123456789abcdef0123456789abcdef",
        }))
        .unwrap();
        match provision {
            LightPairingStage::Provision { address, .. } => {
                assert_eq!(address, "AA:BB:CC:DD:EE:FF")
            }
            _ => panic!("expected provision"),
        }
        for bad in [
            json!({"stage": "reset"}),
            json!({"stage": "provision", "dsn": "../x", "address": "AA:BB:CC:DD:EE:FF", "setup_token": "0123456789abcdef0123456789abcdef"}),
            json!({"stage": "provision", "dsn": "ACFIXTURE123456", "address": "not-a-mac", "setup_token": "0123456789abcdef0123456789abcdef"}),
            json!({"stage": "provision", "dsn": "ACFIXTURE123456", "address": "AA:BB:CC:DD:EE:FF", "setup_token": "short"}),
            json!({"stage": "adopt", "dsn": "ACFIXTURE123456", "ip": "8.8.8.8", "local_key": "synthetic-key", "local_key_id": 1}),
        ] {
            assert!(parse_stage(&bad).is_err(), "{bad}");
        }
        let adopt = parse_stage(&json!({
            "stage": "adopt",
            "dsn": "ACFIXTURE123456",
            "ip": "192.168.4.20",
            "local_key": "synthetic-fixture-key",
            "local_key_id": 7,
            "name": "  Desk strip  ",
        }))
        .unwrap();
        match adopt {
            LightPairingStage::Adopt { name, model, .. } => {
                assert_eq!(name.as_deref(), Some("Desk strip"));
                assert!(model.is_none());
            }
            _ => panic!("expected adopt"),
        }
    }

    #[test]
    fn sessions_carry_stage_details_and_never_secrets() {
        let discover = discover_session(&[DiscoveredCandidate {
            dsn: "ACFIXTURE123456".into(),
            address: "AA:BB:CC:DD:EE:FF".into(),
        }]);
        assert_eq!(discover.status, PairingStatus::Complete);
        assert_eq!(discover.details.as_ref().unwrap()["stage"], STAGE_DISCOVER);
        assert_eq!(
            discover.details.as_ref().unwrap()["candidates"][0]["dsn"],
            "ACFIXTURE123456"
        );

        let failed = failed_session(STAGE_PROVISION, "delivery uncertain", true);
        assert_eq!(failed.status, PairingStatus::Failed);
        assert_eq!(failed.details.as_ref().unwrap()["uncertain"], true);

        let credentials: LightCredentials = serde_json::from_value(json!({
            "dsn": "ACFIXTURE123456",
            "ip": "192.168.4.20",
            "local_key": "synthetic-fixture-key",
            "local_key_id": 7,
        }))
        .unwrap();
        let record = record_for_adoption(&credentials, None, None, 5);
        let adopted = adopted_session(&record);
        let json = serde_json::to_string(&adopted).unwrap();
        assert!(!json.contains("synthetic-fixture-key"));
        assert_eq!(adopted.device.unwrap().device_id, "monster-acfixture123456");
        assert_eq!(record.name, "Monster Neon Flow 3456");
    }
}
