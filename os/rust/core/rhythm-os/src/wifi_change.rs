//! Durable admission and reconciliation for one owner-requested Matter move.
//! Receipts contain no credentials and are never executable after a restart.
use crate::{handlers::ApiResponse, provisioning::WifiCredentials, state::SharedState};
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

const STORE: &str = "matter/wifi-changes.json";
/// Per-appliance admission state. Owned by `AppState` so a process hosting
/// several appliances (tests, simulators) never shares one fence.
pub struct WifiChangeRuntime {
    pub(crate) lock: Mutex<()>,
    active: AtomicBool,
    boot: String,
}
impl Default for WifiChangeRuntime {
    fn default() -> Self {
        Self {
            lock: Mutex::new(()),
            active: AtomicBool::new(false),
            boot: uuid::Uuid::new_v4().to_string(),
        }
    }
}
pub(crate) fn runtime(state: &SharedState) -> Result<Arc<WifiChangeRuntime>> {
    Ok(state
        .lock()
        .map_err(|_| anyhow::anyhow!("state unavailable"))?
        .wifi_change
        .clone())
}

/// Arguments: state, native device ID, target network, remaining budget (ms).
pub type WifiChangeFn = Arc<
    dyn Fn(&SharedState, &str, &WifiCredentials, u64) -> Result<WifiChangeOutcome> + Send + Sync,
>;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WifiChangeCode {
    Succeeded,
    Unsupported,
    Offline,
    NetworkSlots,
    CredentialsRejected,
    NetworkNotFound,
    Rejected,
    FailSafeBusy,
    VerificationFailed,
    RecoveryRequired,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct WifiChangeOutcome {
    pub code: WifiChangeCode,
    pub rollback_verified: bool,
}
impl WifiChangeOutcome {
    pub fn recovery_required() -> Self {
        Self {
            code: WifiChangeCode::RecoveryRequired,
            rollback_verified: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct WifiChangeReceipt {
    pub operation_id: String,
    pub status: String,
    pub started_at_ms: u64,
    pub retry_after_ms: u64,
    pub outcome: Option<WifiChangeOutcome>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Entry {
    receipt: WifiChangeReceipt,
    device_id: String,
    profile_id: String,
    boot: String,
}
#[derive(Serialize, Deserialize)]
struct Journal {
    schema_version: u32,
    entries: Vec<Entry>,
}
fn storage(state: &SharedState) -> Result<Arc<dyn crate::storage::Storage>> {
    state
        .lock()
        .map_err(|_| anyhow::anyhow!("state unavailable"))?
        .storage
        .clone()
        .ok_or_else(|| anyhow::anyhow!("durable storage unavailable"))
}
fn load(state: &SharedState, runtime: &WifiChangeRuntime) -> Result<Journal> {
    let raw = storage(state)?.load_integration_state_file(STORE)?;
    let mut journal: Journal = match raw {
        Some(raw) => serde_json::from_str(&raw)?,
        None => Journal {
            schema_version: 1,
            entries: vec![],
        },
    };
    if journal.schema_version != 1 || journal.entries.len() > 256 {
        bail!("unsupported network change journal");
    }
    for entry in &mut journal.entries {
        if entry.receipt.status == "pending"
            && (entry.boot != runtime.boot || !runtime.active.load(Ordering::SeqCst))
        {
            entry.receipt.status = "complete".into();
            entry.receipt.outcome = Some(WifiChangeOutcome::recovery_required());
        }
    }
    Ok(journal)
}
fn save(state: &SharedState, journal: &Journal) -> Result<()> {
    storage(state)?.save_integration_state_file(STORE, &serde_json::to_string(journal)?)
}
fn unavailable() -> ApiResponse {
    ApiResponse::server_error(
        "Network change status is unavailable; keep both networks available and check again",
    )
}
fn response(receipt: &WifiChangeReceipt) -> ApiResponse {
    let mut receipt = receipt.clone();
    receipt.retry_after_ms = receipt
        .retry_after_ms
        .saturating_sub(crate::state::current_epoch_ms());
    ApiResponse::json_ok(serde_json::to_string(&receipt).unwrap())
}
fn conflict(message: &str) -> ApiResponse {
    ApiResponse {
        status: 409,
        content_type: "application/json",
        body: json!({"error":message}).to_string(),
    }
}

pub fn handle_status(state: &SharedState, operation_id: &str) -> ApiResponse {
    let Ok(runtime) = runtime(state) else {
        return unavailable();
    };
    let Ok(_guard) = runtime.lock.lock() else {
        return unavailable();
    };
    match load(state, &runtime) {
        Ok(journal) => match journal
            .entries
            .iter()
            .find(|e| e.receipt.operation_id == operation_id)
        {
            Some(entry) => response(&entry.receipt),
            None => ApiResponse::not_found(
                "Network change receipt not found; do not repeat an uncertain request",
            ),
        },
        Err(_) => unavailable(),
    }
}

pub fn handle_latest(state: &SharedState, device_id: &str) -> ApiResponse {
    let Ok(runtime) = runtime(state) else {
        return unavailable();
    };
    let Ok(_guard) = runtime.lock.lock() else {
        return unavailable();
    };
    match load(state, &runtime) {
        Ok(journal) => journal
            .entries
            .iter()
            .rev()
            .find(|e| e.device_id == device_id)
            .map(|entry| response(&entry.receipt))
            .unwrap_or_else(|| ApiResponse::not_found("No previous network change")),
        Err(_) => unavailable(),
    }
}

pub fn handle_start(state: &SharedState, body: &Value) -> ApiResponse {
    let operation_id = body
        .get("operation_id")
        .and_then(Value::as_str)
        .unwrap_or("");
    let profile_id = body.get("profile_id").and_then(Value::as_str).unwrap_or("");
    let device_id = body.get("device_id").and_then(Value::as_str).unwrap_or("");
    if uuid::Uuid::parse_str(operation_id).is_err()
        || uuid::Uuid::parse_str(profile_id).is_err()
        || device_id.is_empty()
        || device_id.len() > 100
    {
        return ApiResponse::bad_request(
            "A device, saved network, and UUID operation_id are required",
        );
    }
    let Ok(runtime) = runtime(state) else {
        return unavailable();
    };
    let Ok(_guard) = runtime.lock.lock() else {
        return unavailable();
    };
    let mut journal = match load(state, &runtime) {
        Ok(journal) => journal,
        Err(_) => return unavailable(),
    };
    if let Some(entry) = journal
        .entries
        .iter()
        .find(|e| e.receipt.operation_id == operation_id)
    {
        if entry.device_id != device_id || entry.profile_id != profile_id {
            return conflict("Operation ID already belongs to another request");
        }
        return response(&entry.receipt);
    }
    let now = crate::state::current_epoch_ms();
    if runtime.active.load(Ordering::SeqCst)
        || journal
            .entries
            .iter()
            .any(|e| e.receipt.status == "pending" || e.receipt.retry_after_ms > now)
    {
        return conflict("A network change is running or recovering; keep both networks available and check its status");
    }
    // Receipt expiry is explicit: unknown status is never permission for the app
    // to repeat a POST. The UI creates a fresh UUID only for a deliberate attempt.
    journal
        .entries
        .retain(|e| now.saturating_sub(e.receipt.started_at_ms) < 30 * 24 * 60 * 60 * 1000);
    if journal.entries.len() >= 256 {
        return conflict("Network change history is full; try again after older receipts expire");
    }
    let callback = state.lock().ok().and_then(|s| s.change_wifi_fn.clone());
    let Some(callback) = callback else {
        return ApiResponse::bad_request("This appliance cannot change a Matter network");
    };
    let wifi = match crate::wifi_profiles::selected_credentials(state, profile_id) {
        Ok(wifi) => wifi,
        Err(_) => return ApiResponse::bad_request("Saved network is unavailable"),
    };
    let receipt = WifiChangeReceipt {
        operation_id: operation_id.into(),
        status: "pending".into(),
        started_at_ms: now,
        retry_after_ms: now.saturating_add(300_000),
        outcome: None,
    };
    journal.entries.push(Entry {
        receipt: receipt.clone(),
        device_id: device_id.into(),
        profile_id: profile_id.into(),
        boot: runtime.boot.clone(),
    });
    if save(state, &journal).is_err() {
        return unavailable();
    }
    runtime.active.store(true, Ordering::SeqCst);
    let state = state.clone();
    let operation = operation_id.to_string();
    let device = device_id.to_string();
    let worker_state = state.clone();
    let worker_id = operation.clone();
    let expires_at_ms = receipt.retry_after_ms;
    let spawned = std::thread::Builder::new()
        .name("matter-wifi-change".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                // A relative budget: the native owner measures it on its
                // own monotonic clock, immune to a wall-clock step.
                let budget_ms = expires_at_ms.saturating_sub(crate::state::current_epoch_ms());
                callback(&worker_state, &device, &wifi, budget_ms)
            }));
            let outcome = result
                .ok()
                .and_then(Result::ok)
                .unwrap_or_else(WifiChangeOutcome::recovery_required);
            finish(&worker_state, &worker_id, outcome);
        });
    if spawned.is_err() {
        runtime.active.store(false, Ordering::SeqCst);
        if let Some(entry) = journal.entries.last_mut() {
            entry.receipt.status = "complete".into();
            entry.receipt.outcome = Some(WifiChangeOutcome::recovery_required());
        }
        let _ = save(&state, &journal);
        return unavailable();
    }
    response(&receipt)
}

fn finish(state: &SharedState, operation_id: &str, outcome: WifiChangeOutcome) {
    let Ok(runtime) = runtime(state) else {
        return;
    };
    let Ok(_guard) = runtime.lock.lock() else {
        return;
    };
    let mut saved = false;
    if let Ok(mut journal) = load(state, &runtime) {
        if let Some(entry) = journal
            .entries
            .iter_mut()
            .find(|e| e.receipt.operation_id == operation_id)
        {
            entry.receipt.status = "complete".into();
            // A failed/uncertain transaction keeps the fail-safe grace fence.
            if outcome.code == WifiChangeCode::Succeeded || outcome.rollback_verified {
                entry.receipt.retry_after_ms = 0;
            }
            entry.receipt.outcome = Some(outcome.clone());
            saved = save(state, &journal).is_ok();
        }
    }
    runtime.active.store(false, Ordering::SeqCst);
    drop(_guard);
    if saved {
        let code = serde_json::to_value(&outcome.code)
            .ok()
            .and_then(|v| v.as_str().map(str::to_string))
            .unwrap_or_else(|| "recovery_required".into());
        crate::wifi_profiles::record_outcome(state, "wifi_change", operation_id, &code);
    }
}

/// Reset must not race a worker that could recreate a just-deleted receipt.
/// The caller holds the returned runtime's lock for the whole reset.
pub fn ensure_idle_for_reset(state: &SharedState, runtime: &WifiChangeRuntime) -> Result<()> {
    if runtime.active.load(Ordering::SeqCst) {
        bail!("Wait for the network change to finish before resetting");
    }
    // Restoring or resetting immediately after a process crash must not erase
    // the still-live device fail-safe fence along with its receipt.
    if state.lock().ok().and_then(|s| s.storage.clone()).is_some() {
        let now = crate::state::current_epoch_ms();
        if load(state, runtime)?
            .entries
            .iter()
            .any(|e| e.receipt.retry_after_ms > now)
        {
            bail!("Wait for network recovery before restoring or resetting");
        }
    }
    Ok(())
}
