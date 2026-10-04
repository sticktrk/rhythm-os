//! Separate mobile and internal-admin authentication around shared domain APIs.

use std::sync::Arc;

use axum::{
    body::{Body, Bytes},
    extract::{Extension, FromRequest, State},
    http::{Request, StatusCode, Version},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, post},
    Json, Router,
};
use rhythm_os::{
    auth::{ApiAuthRequestInfo, ApiTokenRole},
    logging,
    state::SharedState,
};
use serde_json::json;

use crate::mobile_access::MobileAccess;

fn shared_routes() -> Router<SharedState> {
    rhythm_os::axum_router::api_routes()
        .route("/api/addon/status", get(crate::policy::status))
        .route(
            "/api/addon/lights",
            get(crate::selection::get).put(crate::selection::put),
        )
        .route(
            "/api/addon/mobile-tokens",
            get(crate::mobile_access::list_tokens),
        )
        .route(
            "/api/addon/mobile-tokens/:id",
            delete(crate::mobile_access::revoke_token),
        )
        .layer(middleware::from_fn(image_health))
}

async fn image_health(request: Request<Body>, next: Next) -> Response {
    // Platform-specific addition behind the same outer admission layers. The
    // common server/appliance health contract and admin authentication stay intact.
    if request.method() == axum::http::Method::GET && request.uri().path() == "/health" {
        return Json(json!({"status": "healthy", "build": crate::build_info::current()}))
            .into_response();
    }
    next.run(request).await
}

pub fn create_mobile_router(state: SharedState, access: Arc<MobileAccess>) -> Router {
    let api = shared_routes()
        .route(
            "/api/addon/enrollment/exchange",
            post(crate::mobile_access::exchange_enrollment),
        )
        .with_state(state.clone())
        .layer(middleware::from_fn_with_state(state.clone(), mobile_auth))
        .layer(middleware::from_fn(serialize_mutations))
        .layer(middleware::from_fn_with_state(
            state,
            crate::policy::enforce,
        ))
        .layer(Extension(access))
        .layer(middleware::from_fn(buffer_request_body))
        .layer(middleware::from_fn(no_store));
    logging::with_http_observability(api)
}

pub fn create_admin_router(state: SharedState, access: Arc<MobileAccess>) -> Router {
    let api = shared_routes()
        .route(
            "/api/addon/enrollment",
            post(crate::mobile_access::create_enrollment),
        )
        .with_state(state.clone())
        .layer(middleware::from_fn(admin_auth))
        .layer(middleware::from_fn(serialize_mutations))
        .layer(middleware::from_fn_with_state(
            state,
            crate::policy::enforce,
        ))
        .layer(Extension(access))
        .layer(middleware::from_fn(buffer_request_body))
        .layer(middleware::from_fn(no_store));
    logging::with_http_observability(api)
}

fn bearer(request: &Request<Body>) -> Option<&str> {
    request
        .headers()
        .get("authorization")?
        .to_str()
        .ok()?
        .strip_prefix("Bearer ")
}

async fn no_store(request: Request<Body>, next: Next) -> Response {
    let mut response = next.run(request).await;
    response.headers_mut().insert(
        "cache-control",
        "no-store".parse().expect("constant header"),
    );
    response
}

async fn buffer_request_body(request: Request<Body>, next: Next) -> Response {
    // Policy/auth failures and handlers such as enrollment do not extract a
    // body. Dropping an unread HTTP/1 body can make Hyper close the connection
    // while Dart is still writing it, losing even a successful mutation's
    // response. Consume it before admission, then replay it to JSON extractors.
    // Bytes uses Axum's normal 2 MiB bound; include ignored and chunked bodies.
    let (parts, body) = request.into_parts();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15),
        Bytes::from_request(Request::new(body), &()),
    )
    .await;
    let mut rejection = match result {
        Ok(Ok(bytes)) => {
            return next
                .run(Request::from_parts(parts, Body::from(bytes)))
                .await;
        }
        Ok(Err(error)) => error.into_response(),
        Err(_) => (
            StatusCode::REQUEST_TIMEOUT,
            Json(json!({"error": "Request body timed out"})),
        )
            .into_response(),
    };
    // Failed reads may leave bytes unread. Explicitly retire HTTP/1 sockets so
    // clients do not reuse them after an oversized, malformed or stalled body.
    if matches!(parts.version, Version::HTTP_10 | Version::HTTP_11) {
        rejection
            .headers_mut()
            .insert("connection", "close".parse().expect("constant header"));
    }
    rejection
}

async fn serialize_mutations(
    Extension(access): Extension<Arc<MobileAccess>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    // A reset must drain earlier writes before deleting durable state. Requests
    // queued behind it recheck reset and authentication before their handler.
    // Read streams stay independent. Public enrollment exchanges already share
    // the reset hook's enrollment lock and parse their body before taking it.
    if matches!(request.method().as_str(), "GET" | "HEAD" | "OPTIONS")
        || request.uri().path() == "/api/addon/enrollment/exchange"
    {
        return next.run(request).await;
    }
    let guard = access.mutation_gate.clone().lock_owned().await;
    if access.is_resetting() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Installation is resetting"})),
        )
            .into_response();
    }
    // A disconnected client must not release the barrier while a handler's
    // spawn_blocking mutation continues writing to storage.
    match tokio::spawn(async move {
        let _guard = guard;
        next.run(request).await
    })
    .await
    {
        Ok(response) => response,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}

async fn mobile_auth(
    State(state): State<SharedState>,
    Extension(access): Extension<Arc<MobileAccess>>,
    request: Request<Body>,
    next: Next,
) -> Response {
    if access.is_resetting() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Installation is resetting"})),
        )
            .into_response();
    }
    let path = request.uri().path();
    if path == "/api/addon/enrollment" {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Home Assistant administrator approval required"})),
        )
            .into_response();
    }
    if path == "/api/addon/enrollment/exchange" || path == "/api/addon/status" {
        return next.run(request).await;
    }
    let role =
        bearer(&request).and_then(|token| state.lock().ok()?.api_auth.verify_token_role(token));
    if path != "/health" && path != "/api/auth/status" && role.is_none() {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Mobile bearer token required"})),
        )
            .into_response();
    }
    if path.starts_with("/api/addon/mobile-tokens") && role != Some(ApiTokenRole::Owner) {
        return (
            StatusCode::FORBIDDEN,
            Json(json!({"error":"Owner bearer token required"})),
        )
            .into_response();
    }
    // Shared middleware preserves support scopes, audit, remote provenance, and
    // active-stream revocation. Positive proof above remains strict after reset.
    rhythm_os::auth::require_api_auth_middleware(State(state), request, next).await
}

async fn admin_auth(
    Extension(access): Extension<Arc<MobileAccess>>,
    mut request: Request<Body>,
    next: Next,
) -> Response {
    if access.is_resetting() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"Installation is resetting"})),
        )
            .into_response();
    }
    if !bearer(&request).is_some_and(|token| access.verify_admin(token)) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error":"Local admin authentication required"})),
        )
            .into_response();
    }
    // Only the loopback listener with its own ephemeral secret grants this
    // authority. HA identity headers and persistent phone tokens grant none.
    request.extensions_mut().insert(ApiAuthRequestInfo {
        requires_auth: true,
        claim_available: false,
        via_remote_access: false,
        role: Some(ApiTokenRole::Owner),
        token_expires_at_epoch_ms: None,
    });
    next.run(request).await
}

#[cfg(test)]
mod tests;
