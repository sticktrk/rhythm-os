//! Owner-only HA Matter facade. Every physical operation is delegated to HA Core.

use crate::mobile_access::MobileAccess;
use anyhow::{Context, Result};
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use rhythm_ha::{
    matter::{validate_setup_code, HaMatterClient, MatterCatalog, MatterDevice, Rejected},
    matter_store,
};
use rhythm_os::{
    pairing::{self, BeginPairingResult, PairingSession, PairingStatus},
    state::SharedState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{net::IpAddr, sync::Arc, time::Duration};

const HUB: &str = "homeassistant";
const UNKNOWN: &str = "Home Assistant pairing outcome is unknown. Review Home Assistant before starting another attempt.";
const NOT_STARTED: &str = "Home Assistant was unavailable before pairing started";
#[derive(Debug)]
struct NotStarted;
impl std::fmt::Display for NotStarted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(NOT_STARTED)
    }
}
impl std::error::Error for NotStarted {}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PairRequest {
    session_id: String,
    setup_code: String,
    code_source: CodeSource,
    rendezvous: Rendezvous,
    #[serde(default)]
    handoff_setup_payload: Option<String>,
    #[serde(default)]
    handoff_ip_address: Option<String>,
    #[serde(default)]
    handoff_port: Option<u16>,
    #[serde(default)]
    handoff_passcode: Option<u32>,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CodeSource {
    OriginalLabel,
    Sharing,
}
#[derive(Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Rendezvous {
    OnNetwork,
    Phone,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    identity: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveCode {
    identity: String,
    setup_code: String,
    code_source: CodeSource,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BindDevice {
    device_id: String,
    identity: String,
}

fn context(state: &SharedState) -> Result<(String, String)> {
    let s = state
        .lock()
        .map_err(|_| anyhow::anyhow!("State unavailable"))?;
    anyhow::ensure!(
        s.platform_context == "ha_addon",
        "HA Matter administration unavailable"
    );
    Ok((s.data_dir.clone(), s.server_instance_id.clone()))
}
async fn client() -> Result<HaMatterClient> {
    HaMatterClient::connect(&rhythm_ha::transport::HaConnectionConfig::for_supervisor()?).await
}
fn error(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"error":message}))).into_response()
}
fn unavailable() -> Response {
    error(
        StatusCode::SERVICE_UNAVAILABLE,
        "Home Assistant Matter operation unavailable. Refresh and review Home Assistant.",
    )
}
fn terminal(status: PairingStatus, error: Option<&str>) -> PairingSession {
    PairingSession {
        hub_type: HUB.into(),
        status,
        device: None,
        devices: vec![],
        error: error.map(str::to_owned),
        failure_stage: None,
        warnings: vec![],
        details: None,
    }
}

impl PairRequest {
    fn command(&self) -> Result<(&'static str, Value)> {
        pairing::validate_pairing_session_id(&self.session_id).map_err(anyhow::Error::msg)?;
        validate_setup_code(&self.setup_code)?;
        match self.rendezvous {
            Rendezvous::OnNetwork => {
                anyhow::ensure!(
                    self.handoff_setup_payload.is_none()
                        && self.handoff_ip_address.is_none()
                        && self.handoff_passcode.is_none()
                        && self.handoff_port.is_none(),
                    "Unexpected phone handoff"
                );
                Ok((
                    "matter/commission",
                    json!({"code":self.setup_code,"network_only":true}),
                ))
            }
            Rendezvous::Phone => {
                if let Some(payload) = &self.handoff_setup_payload {
                    validate_setup_code(payload)?;
                    anyhow::ensure!(
                        self.handoff_ip_address.is_none()
                            && self.handoff_passcode.is_none()
                            && self.handoff_port.is_none(),
                        "Ambiguous phone handoff"
                    );
                    return Ok((
                        "matter/commission",
                        json!({"code":payload,"network_only":true}),
                    ));
                }
                let address = self
                    .handoff_ip_address
                    .as_deref()
                    .context("Phone address missing")?;
                // Phone scope IDs cannot identify an HA host interface.
                let ip: IpAddr = address.parse().context("Invalid phone address")?;
                anyhow::ensure!(
                    !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast(),
                    "Invalid phone address"
                );
                anyhow::ensure!(
                    !matches!(ip,IpAddr::V6(ip) if ip.is_unicast_link_local()),
                    "Phone IPv6 scope is unavailable on HA"
                );
                anyhow::ensure!(
                    self.handoff_port.is_none_or(|port| port == 5540),
                    "HA requires the standard Matter port"
                );
                let pin = self.handoff_passcode.context("Phone passcode missing")?;
                anyhow::ensure!(
                    (1..=99_999_998).contains(&pin)
                        && ![
                            11_111_111, 22_222_222, 33_333_333, 44_444_444, 55_555_555, 66_666_666,
                            77_777_777, 88_888_888, 12_345_678, 87_654_321
                        ]
                        .contains(&pin),
                    "Invalid phone passcode"
                );
                Ok((
                    "matter/commission_on_network",
                    json!({"pin":pin,"ip_addr":ip.to_string()}),
                ))
            }
        }
    }
}

pub async fn catalog(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
) -> Response {
    let Ok((dir, scope)) = context(&state) else {
        return unavailable();
    };
    let result = async { client().await?.catalog(&scope).await }.await;
    let catalog = result.unwrap_or_else(|_| MatterCatalog::unavailable());
    if catalog.available {
        let _guard = access.mutation_gate.lock().await;
        if access.is_resetting() {
            return unavailable();
        }
        if matter_store::reconcile_devices(
            &dir,
            &scope,
            &catalog.devices,
            &catalog.registry_device_ids,
        )
        .is_err()
        {
            return unavailable();
        }
    }
    Json(catalog).into_response()
}

fn receipt(state: &SharedState, session: &str, tombstone: bool) -> Result<Value> {
    pairing::validate_pairing_session_id(session).map_err(anyhow::Error::msg)?;
    let (dir, scope) = context(state)?;
    let confirmed = matter_store::confirmed(&dir, &scope, session)?;
    if let Some(success) = &confirmed {
        pairing::reconcile_committed_pairing_success_with_fingerprint(
            state,
            session,
            HUB,
            &success.fingerprint,
            &terminal(PairingStatus::Complete, None),
        )?;
    }
    let record = if tombstone {
        pairing::lookup_or_tombstone_pairing_result(state, session)?
    } else {
        pairing::lookup_pairing_result(state, session)?
    };
    anyhow::ensure!(
        record
            .as_ref()
            .is_none_or(|r| r.hub_type.as_deref() == Some(HUB)),
        "Pairing belongs to another backend"
    );
    let result = record.as_ref().and_then(|r| r.result.as_ref());
    let status = match result {
        Some(r) if r.status == PairingStatus::Complete => "completed",
        Some(r)
            if matches!(
                r.error.as_deref(),
                Some("Home Assistant rejected pairing") | Some(NOT_STARTED)
            ) =>
        {
            "failed"
        }
        Some(_) => "unknown",
        None if record.is_some() => "pending",
        None => "failed",
    };
    let original_code_saved = confirmed.as_ref().and_then(|c| c.device.as_ref()).is_some();
    let needs_device_confirmation = confirmed.as_ref().is_some_and(|c| c.original.is_some());
    Ok(
        json!({"session_id":session,"status":status,"device":confirmed.as_ref().and_then(|c|c.device.as_ref()),
        "can_close_attempt":status == "unknown" && result.is_some(),
        "error":if status == "unknown" {Some(UNKNOWN)} else if record.is_none() {Some("No active pairing receipt. Review Home Assistant before starting a new attempt.")} else {result.and_then(|r|r.error.as_deref())},
        "original_code_saved":original_code_saved,"needs_device_confirmation":needs_device_confirmation,
        "warnings":result.map(|r|r.warnings.clone()).unwrap_or_default()}),
    )
}

pub async fn pairing_status(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
    Path(session): Path<String>,
) -> Response {
    let _guard = access.mutation_gate.lock().await;
    if access.is_resetting() {
        return unavailable();
    }
    match receipt(&state, &session, true) {
        Ok(value) => Json(value).into_response(),
        Err(_) => unavailable(),
    }
}

pub async fn pair(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
    Json(request): Json<PairRequest>,
) -> Response {
    // The outer mutation gate protects admission. The background job releases it
    // during HA I/O so ordinary light controls remain usable.
    if request.command().is_err() {
        return error(StatusCode::BAD_REQUEST, "Invalid Matter pairing request");
    }
    let Ok((_, scope)) = context(&state) else {
        return unavailable();
    };
    let params = serde_json::to_value(&request).expect("typed request");
    let Ok(fingerprint) = pairing::pairing_request_fingerprint_for_state(&state, HUB, &params)
    else {
        return unavailable();
    };
    let started = pairing::begin_pairing_result(&state, &request.session_id, HUB, &fingerprint);
    let lease = match started {
        Ok(BeginPairingResult::Started(lease)) => lease,
        Ok(BeginPairingResult::Pending(_) | BeginPairingResult::Terminal(_)) => {
            return match receipt(&state, &request.session_id, false) {
                Ok(v) => Json(v).into_response(),
                Err(_) => unavailable(),
            }
        }
        Ok(_) => {
            return error(
                StatusCode::CONFLICT,
                "Pairing session was already used or reconciled; review its status",
            )
        }
        Err(_) => return unavailable(),
    };
    let session_id = request.session_id.clone();
    tokio::spawn(async move {
        let _lease = lease;
        let outcome = async {
            let mut client = client().await.map_err(|_| NotStarted)?;
            if !client
                .catalog(&scope)
                .await
                .map_err(|_| NotStarted)?
                .available
            {
                return Err(NotStarted.into());
            }
            let (kind, fields) = request.command()?;
            client
                .command(kind, fields, Duration::from_secs(180))
                .await?;
            Ok::<_, anyhow::Error>(())
        }
        .await;
        finish_pairing(&state, &access, &request, &fingerprint, outcome).await;
    });
    Json(json!({"session_id":session_id,"status":"pending","device":null,"error":null,"can_close_attempt":false,"original_code_saved":false,"needs_device_confirmation":false,"warnings":[]})).into_response()
}

pub(super) async fn finish_pairing(
    state: &SharedState,
    access: &MobileAccess,
    request: &PairRequest,
    fingerprint: &str,
    outcome: Result<()>,
) {
    let _guard = access.mutation_gate.lock().await;
    if access.is_resetting() {
        return;
    }
    let Ok((dir, scope)) = context(state) else {
        return;
    };
    let mut terminal = match outcome {
        Ok(()) => terminal(PairingStatus::Complete, None),
        Err(e) if e.downcast_ref::<NotStarted>().is_some() => {
            terminal(PairingStatus::Failed, Some(NOT_STARTED))
        }
        Err(e) if e.downcast_ref::<Rejected>().is_some() => terminal(
            PairingStatus::Failed,
            Some("Home Assistant rejected pairing"),
        ),
        Err(_) => terminal(PairingStatus::Failed, Some(UNKNOWN)),
    };
    if terminal.status == PairingStatus::Complete {
        let original = (request.code_source == CodeSource::OriginalLabel)
            .then_some(request.setup_code.as_str());
        if matter_store::remember_success(&dir, &scope, &request.session_id, &fingerprint, original)
            .is_err()
        {
            terminal
                .warnings
                .push("Original setup code could not be saved. Keep the physical label.".into());
        }
    }
    let _ = pairing::complete_pairing_result(&state, &request.session_id, HUB, &terminal);
    let history = pairing::pairing_history_entry_for_pair(
        HUB,
        &json!({"correlation_id":request.session_id,"rendezvous":request.rendezvous,"device_type":"light"}),
        &terminal,
    );
    pairing::record_pairing_history(&state, history);
    if terminal.status == PairingStatus::Complete {
        if let Some(bootstrap) = state
            .lock()
            .ok()
            .and_then(|s| s.request_hub_bootstrap_fn.clone())
        {
            bootstrap(&state);
        }
    }
}

async fn checked_device(
    state: &SharedState,
    id: &str,
    identity: &str,
) -> Result<(HaMatterClient, MatterDevice)> {
    anyhow::ensure!(
        identity.len() == 64 && identity.bytes().all(|b| b.is_ascii_hexdigit()),
        "Invalid identity"
    );
    let (_, scope) = context(state)?;
    let mut client = client().await?;
    let device = client
        .catalog(&scope)
        .await?
        .devices
        .into_iter()
        .find(|d| d.device_id == id && d.identity == identity)
        .context("HA device identity changed")?;
    Ok((client, device))
}

pub async fn setup_code(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
    Path(id): Path<String>,
    Query(proof): Query<Identity>,
) -> Response {
    let Ok((_, device)) = checked_device(&state, &id, &proof.identity).await else {
        return error(
            StatusCode::CONFLICT,
            "HA device identity changed or is unavailable",
        );
    };
    let _guard = access.mutation_gate.lock().await;
    if access.is_resetting() {
        return unavailable();
    }
    let result =
        context(&state).and_then(|(dir, scope)| matter_store::original(&dir, &scope, &device));
    match result {Ok(code)=>Json(json!({"device_id":id,"identity":proof.identity,"available":code.is_some(),"setup_code":code,"code_source":"original_label"})).into_response(),Err(_)=>unavailable()}
}
pub async fn save_code(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(request): Json<SaveCode>,
) -> Response {
    if request.code_source != CodeSource::OriginalLabel
        || validate_setup_code(&request.setup_code).is_err()
    {
        return error(
            StatusCode::BAD_REQUEST,
            "Only an original physical label code can be saved",
        );
    }
    let Ok((_, device)) = checked_device(&state, &id, &request.identity).await else {
        return error(
            StatusCode::CONFLICT,
            "HA device identity changed or is unavailable",
        );
    };
    match context(&state).and_then(|(dir, scope)| {
        matter_store::save_original(&dir, &scope, &device, &request.setup_code)
    }) {
        Ok(()) => {
            Json(json!({"saved":true,"device_id":id,"identity":request.identity})).into_response()
        }
        Err(_) => unavailable(),
    }
}
pub async fn bind_device(
    State(state): State<SharedState>,
    Path(session): Path<String>,
    Json(request): Json<BindDevice>,
) -> Response {
    let Ok((_, device)) = checked_device(&state, &request.device_id, &request.identity).await
    else {
        return error(
            StatusCode::CONFLICT,
            "HA device identity changed or is unavailable",
        );
    };
    if context(&state)
        .and_then(|(dir, scope)| matter_store::bind_session(&dir, &scope, &session, &device))
        .is_err()
    {
        return error(
            StatusCode::CONFLICT,
            "Original setup code cannot be bound to this device or has expired",
        );
    }
    match receipt(&state, &session, false) {
        Ok(v) => Json(v).into_response(),
        Err(_) => unavailable(),
    }
}
pub async fn share(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(proof): Json<Identity>,
) -> Response {
    let Ok((mut client, _)) = checked_device(&state, &id, &proof.identity).await else {
        return error(
            StatusCode::CONFLICT,
            "HA device identity changed or is unavailable",
        );
    };
    match client
        .command(
            "matter/open_commissioning_window",
            json!({"device_id":id}),
            Duration::from_secs(30),
        )
        .await
    {
        Ok(value) => {
            let code = value["setup_qr_code"].as_str();
            if !code.is_some_and(|c| validate_setup_code(c).is_ok()) {
                return unavailable();
            }
            Json(json!({"setup_code":code,"manual_code":value["setup_manual_code"],"expires_in":300})).into_response()
        }
        Err(_) => unavailable(),
    }
}
pub async fn remove(
    State(state): State<SharedState>,
    Path(id): Path<String>,
    Json(proof): Json<Identity>,
) -> Response {
    let Ok((mut client, device)) = checked_device(&state, &id, &proof.identity).await else {
        return error(
            StatusCode::CONFLICT,
            "HA device identity changed or is unavailable",
        );
    };
    let outcome = client
        .command(
            "config/device_registry/remove_config_entry",
            json!({"device_id":id,"config_entry_id":device.config_entry_id}),
            Duration::from_secs(30),
        )
        .await;
    let Ok((dir, scope)) = context(&state) else {
        return unavailable();
    };
    // A lost acknowledgement is not a reason to send removal twice. Observe HA first.
    let removed = client
        .catalog(&scope)
        .await
        .is_ok_and(|c| c.confirms_removal(&device));
    if !removed {
        return error(
            StatusCode::CONFLICT,
            "Removal is unconfirmed. Refresh Home Assistant before trying again.",
        );
    }
    if matter_store::forget(&dir, &scope, &device).is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Device was removed, but private code cleanup needs repair",
        );
    }
    let history = pairing::pairing_history_entry_for_unpair(
        HUB,
        &json!({"device_type":"light"}),
        &PairingStatus::Complete,
        None,
        None,
    );
    pairing::record_pairing_history(&state, history);
    let _ = outcome;
    Json(json!({"removed":true,"device_id":id})).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn handoffs_are_validated_and_never_replace_original() {
        let request: PairRequest = serde_json::from_value(json!({"session_id":"session","setup_code":"12345678901","code_source":"original_label","rendezvous":"phone","handoff_setup_payload":"MT:ABC123"})).unwrap();
        let (kind, fields) = request.command().unwrap();
        assert_eq!(kind, "matter/commission");
        assert_eq!(fields, json!({"code":"MT:ABC123","network_only":true}));
        assert_eq!(request.setup_code, "12345678901");
        let mut android = request;
        android.handoff_setup_payload = None;
        android.handoff_ip_address = Some("fe80::1%en0".into());
        android.handoff_passcode = Some(20202021);
        assert!(android.command().is_err());
        android.handoff_ip_address = Some("192.0.2.1".into());
        assert_eq!(
            android.command().unwrap(),
            (
                "matter/commission_on_network",
                json!({"pin":20202021,"ip_addr":"192.0.2.1"})
            )
        );
    }
}
