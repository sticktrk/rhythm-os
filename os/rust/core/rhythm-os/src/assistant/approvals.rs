//! Credential-bound reviews for the hub's stateful operation handlers.
//! Native plans retain their own freshness checks. The transport exposes a
//! bound handle only after registering the exact arguments and original review.
use super::*;
use crate::auth::{ApiAuthRequestInfo, ApiTokenRole, VerifiedApiToken};
use crate::state::AppState;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde::Deserialize;
use std::collections::BTreeMap;

const MAX_REVIEWS: usize = 256;
const PLAN_LIFETIME_MS: u64 = 300_000;
type Result<T> = std::result::Result<T, LightAssistantError>;

#[derive(Clone, PartialEq, Eq)]
pub(super) struct Caller {
    pub(super) role: Option<ApiTokenRole>,
    credential: Option<String>,
    unattended: bool,
}

impl Caller {
    pub(super) fn from_request(
        info: Option<Extension<ApiAuthRequestInfo>>,
        token: Option<Extension<VerifiedApiToken>>,
    ) -> Self {
        Self {
            role: info.and_then(|Extension(i)| i.role),
            credential: token.as_ref().map(|Extension(t)| t.binding_id.clone()),
            unattended: token.is_some_and(|Extension(t)| t.unattended_changes),
        }
    }
}

enum Status {
    Reviewed,
    Awaiting,
    Running,
    Declined,
    Completed(Value),
}

struct Review {
    caller: Caller,
    origin: &'static LightAssistantOperationDto,
    apply: &'static LightAssistantOperationDto,
    args: Value,
    preview: Value,
    expires: u64,
    status: Status,
}

#[derive(Default)]
pub struct PlanStore(BTreeMap<String, Review>);

fn stale() -> LightAssistantError {
    LightAssistantError::conflict(
        "plan_unavailable",
        "Review this change again with the credential that created it.",
    )
}

fn unavailable() -> LightAssistantError {
    LightAssistantError::internal("state_unavailable", "Hub unavailable")
}

impl Review {
    fn response(&self, id: &str) -> Value {
        match &self.status {
            Status::Completed(receipt) => receipt.clone(),
            state => {
                json!({"plan_id":id, "expires_at_epoch_ms":self.expires, "status":match state {
                    Status::Reviewed => "reviewed",
                    Status::Awaiting => "awaiting_owner_approval",
                    Status::Running => "applying",
                    Status::Declined => "declined",
                    Status::Completed(_) => unreachable!(),
                }})
            }
        }
    }

    fn native_exists(&self, s: &AppState) -> bool {
        let id = self.args["plan_id"].as_str().unwrap_or_default();
        match self.apply.handler {
            Handler::Automation => s.assistant_automation_plans.iter().any(|p| p.token == id),
            Handler::Devices => s.assistant_devices.plans.iter().any(|p| p.id == id),
            Handler::Lighting => s.assistant_light_plans.iter().any(|p| p.id == id),
            Handler::Motion => s.assistant_motion.plans.contains_key(id),
            Handler::Portable => s.assistant_transfers.iter().any(|p| p.id == id),
            // A device-room move is a content-addressed native plan. The
            // credential-bound review is its single-use authorization record.
            Handler::Legacy => true,
            Handler::Information => false,
        }
    }

    fn discard_native(&self, s: &mut AppState) {
        let id = self.args["plan_id"].as_str().unwrap_or_default();
        match self.apply.handler {
            Handler::Automation => s.assistant_automation_plans.retain(|p| p.token != id),
            Handler::Devices => s.assistant_devices.plans.retain(|p| p.id != id),
            Handler::Lighting => s.assistant_light_plans.retain(|p| p.id != id),
            Handler::Motion => {
                s.assistant_motion.plans.remove(id);
            }
            Handler::Portable => s.assistant_transfers.retain(|p| p.id != id),
            Handler::Legacy | Handler::Information => {}
        }
    }

    fn current(&self, s: &AppState) -> bool {
        self.expires > current_epoch_ms()
            && self.native_exists(s)
            && (self.caller.role != Some(ApiTokenRole::Agent)
                || self.caller.credential.as_ref().is_some_and(|id| {
                    s.api_auth
                        .agent_clients
                        .iter()
                        .any(|client| &client.id == id)
                        || s.api_auth.agent_tokens.iter().any(|t| {
                            &t.id == id
                                && t.expires_at_epoch_ms
                                    .is_none_or(|at| at > current_epoch_ms())
                        })
                }))
    }
}

pub(super) fn register(
    state: &SharedState,
    caller: Caller,
    origin: &'static LightAssistantOperationDto,
    apply: &'static LightAssistantOperationDto,
    preview: &mut Value,
    started: u64,
) -> Result<()> {
    let mut s = state.lock().map_err(|_| unavailable())?;
    let now = current_epoch_ms();
    let mut records = std::mem::take(&mut s.assistant_approvals.0);
    records.retain(|_, r| match r.status {
        Status::Running => true,
        Status::Reviewed | Status::Awaiting => r.current(&s),
        _ => r.expires > now,
    });
    s.assistant_approvals.0 = records;
    let mut args = preview["apply_arguments"].clone();
    args["confirmed"] = json!(true);
    let native_id = args["plan_id"].as_str().ok_or_else(stale)?;
    let id = if matches!(apply.handler, Handler::Legacy) {
        // Keep independent copies of an identical content-addressed preview.
        uuid::Uuid::new_v4().to_string()
    } else {
        native_id.to_owned()
    };
    preview["plan_id"] = json!(id);
    preview["apply_arguments"]["plan_id"] = json!(id);
    let review = Review {
        caller,
        origin,
        apply,
        args,
        expires: preview["expires_at_epoch_ms"]
            .as_u64()
            .unwrap_or(started + PLAN_LIFETIME_MS),
        preview: preview.clone(),
        status: Status::Reviewed,
    };
    if !review.current(&s) {
        return Err(stale());
    }
    if s.assistant_approvals.0.len() >= MAX_REVIEWS {
        let oldest_receipt = s
            .assistant_approvals
            .0
            .iter()
            .filter(|(_, r)| matches!(r.status, Status::Declined | Status::Completed(_)))
            .min_by_key(|(_, r)| r.expires)
            .map(|(id, _)| id.clone());
        if let Some(id) = oldest_receipt {
            s.assistant_approvals.0.remove(&id);
        }
    }
    if s.assistant_approvals.0.len() >= MAX_REVIEWS {
        review.discard_native(&mut s);
        return Err(LightAssistantError::conflict(
            "review_capacity",
            "Complete an existing review or wait for it to expire.",
        ));
    }
    s.assistant_approvals.0.insert(id, review);
    Ok(())
}

pub(super) fn outcome(state: &SharedState, caller: &Caller, args: &Value) -> Result<Value> {
    let id = args["plan_id"].as_str().ok_or_else(stale)?;
    let s = state.lock().map_err(|_| unavailable())?;
    if let Some(r) = s.assistant_approvals.0.get(id) {
        if &r.caller != caller {
            return Err(stale());
        }
        if matches!(r.status, Status::Running)
            || (r.expires > current_epoch_ms()
                && (!matches!(r.status, Status::Reviewed | Status::Awaiting) || r.current(&s)))
        {
            return Ok(r.response(id));
        }
    }
    Ok(json!({"status":"expired","plan_id":id}))
}

pub(super) async fn apply(
    state: SharedState,
    caller: &Caller,
    op: &'static LightAssistantOperationDto,
    mut args: Value,
) -> Result<Value> {
    let id = args["plan_id"].as_str().ok_or_else(stale)?.to_owned();
    let (native_args, replay) = {
        let mut s = state.lock().map_err(|_| unavailable())?;
        let review = s.assistant_approvals.0.get(&id).ok_or_else(stale)?;
        if &review.caller != caller || review.apply.id != op.id {
            return Err(stale());
        }
        let mut expected = review.args.clone();
        expected["plan_id"] = json!(id);
        if op.id == OP_APPLY_MOVE_DEVICE_ROOM_PLAN && args.get("from_room_id") == Some(&Value::Null)
        {
            args.as_object_mut().unwrap().remove("from_room_id");
        }
        if args != expected {
            return Err(stale());
        }
        if op.id == "apply_motion_target"
            && matches!(&review.status, Status::Completed(r) if r["status"] == "applied")
            && review.current(&s)
        {
            // The motion handler verifies current links before confirming a replay.
            (review.args.clone(), true)
        } else {
            if !matches!(review.status, Status::Reviewed | Status::Awaiting) || !review.current(&s)
            {
                return Err(stale());
            }
            let review = s.assistant_approvals.0.get_mut(&id).unwrap();
            if caller.role == Some(ApiTokenRole::Agent)
                && (!caller.unattended || review.origin.requires_owner_approval)
            {
                let first = matches!(review.status, Status::Reviewed);
                review.status = Status::Awaiting;
                let response = review.response(&id);
                let expires = review.expires;
                if first {
                    attention::requested(&state, &s, "plan", &id, expires);
                }
                return Ok(response);
            }
            review.status = Status::Running;
            (review.args.clone(), false)
        }
    };
    if replay {
        transport::dispatch(state, op, native_args).await
    } else {
        run(state, id, op, native_args).await
    }
}

async fn run(
    state: SharedState,
    id: String,
    op: &'static LightAssistantOperationDto,
    args: Value,
) -> Result<Value> {
    // Continue recording the outcome if the client disconnects after dispatch.
    tokio::spawn(async move {
        let mut result = transport::dispatch(state.clone(), op, args).await;
        if let Ok(value) = &mut result {
            if value.get("plan_id").is_some() {
                value["plan_id"] = json!(id);
            }
        }
        let mut s = state.lock().map_err(|_| unavailable())?;
        if let Some(review) = s.assistant_approvals.0.get_mut(&id) {
            let receipt = match &result {
                Ok(value) => value.clone(),
                Err(error) => {
                    json!({"status":"failed","plan_id":id,"error":error.envelope().error})
                }
            };
            review.status = Status::Completed(receipt);
            review.expires = current_epoch_ms() + PLAN_LIFETIME_MS;
        }
        result
    })
    .await
    .map_err(|_| unavailable())?
}

fn owner(info: Option<Extension<ApiAuthRequestInfo>>) -> bool {
    info.is_some_and(|Extension(i)| i.role == Some(ApiTokenRole::Owner))
}

pub(crate) async fn list(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
) -> Response {
    if !owner(info) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Ok(s) = state.lock() else {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    };
    let pending: Vec<_> = s
        .assistant_approvals
        .0
        .iter()
        .filter(|(_, review)| matches!(review.status, Status::Awaiting) && review.current(&s))
        .map(|(id, review)| {
            json!({
                "plan_id": id,
                "operation": review.apply.id,
                "plan_operation": review.origin.id,
                "agent_credential_id": review.caller.credential,
                "expires_at_epoch_ms": review.expires,
                "review": review.preview,
            })
        })
        .collect();
    (
        [("cache-control", "no-store")],
        Json(json!({"approvals":pending})),
    )
        .into_response()
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Decision {
    Approve,
    Decline,
}
#[derive(Deserialize)]
pub(crate) struct OwnerDecision {
    decision: Decision,
}

pub(crate) async fn decide(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    Path(id): Path<String>,
    Json(body): Json<OwnerDecision>,
) -> Response {
    if !owner(info) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let (op, args) = {
        let Ok(mut s) = state.lock() else {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };
        let Some(review) = s.assistant_approvals.0.get(&id) else {
            return StatusCode::CONFLICT.into_response();
        };
        if !matches!(review.status, Status::Awaiting) || !review.current(&s) {
            return StatusCode::CONFLICT.into_response();
        }
        attention::resolved(&s, "plan", &id, review.expires);
        if matches!(body.decision, Decision::Decline) {
            let mut review = s.assistant_approvals.0.remove(&id).unwrap();
            review.discard_native(&mut s);
            review.status = Status::Declined;
            let result = review.response(&id);
            s.assistant_approvals.0.insert(id, review);
            return Json(result).into_response();
        }
        let review = s.assistant_approvals.0.get_mut(&id).unwrap();
        review.status = Status::Running;
        (review.apply, review.args.clone())
    };
    match run(state, id, op, args).await {
        Ok(receipt) => Json(receipt).into_response(),
        Err(error) => (
            StatusCode::from_u16(error.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR),
            Json(error.envelope()),
        )
            .into_response(),
    }
}
