//! Phone approval is a rendezvous between a browser secret and a local owner.
//! The list/notification identifies a request, but only the browser sees its code.
use super::*;
use crate::{assistant::attention, state::AppState};
use rand::Rng;

const REQUEST_MS: u64 = 5 * 60 * 1000;
const MAX_PENDING: usize = 4;

pub(super) struct RedirectDetails {
    pub(super) home_assistant: bool,
    destination: String,
}

impl RedirectDetails {
    pub(super) fn parse(raw: &str) -> Option<Self> {
        if raw.len() > 2048
            || raw.contains(|c: char| c.is_whitespace() || c.is_control() || c == '\\')
        {
            return None;
        }
        // Reject even empty userinfo. URL normalization would otherwise erase it.
        let authority = raw.split_once("://")?.1.split(['/', '?', '#']).next()?;
        if authority.is_empty() || authority.contains('@') {
            return None;
        }
        let url = reqwest::Url::parse(raw).ok()?;
        let host = url.host_str()?;
        let loopback = matches!(host, "localhost" | "127.0.0.1" | "[::1]");
        let ha_callback = url.path() == "/auth/external/callback";
        if url.fragment().is_some()
            || !url.username().is_empty()
            || url.password().is_some()
            || !(url.scheme() == "https" || (url.scheme() == "http" && (loopback || ha_callback)))
        {
            return None;
        }
        Some(Self {
            home_assistant: raw == HA_REDIRECT || ha_callback,
            destination: if raw == HA_REDIRECT {
                "Home Assistant".into()
            } else if loopback {
                "An app on this computer".into()
            } else {
                host.to_ascii_lowercase()
            },
        })
    }

    fn kind(&self) -> &'static str {
        if self.home_assistant {
            "home_assistant"
        } else {
            "oauth"
        }
    }
}

#[derive(Deserialize)]
pub(crate) struct Registration {
    redirect_uris: Vec<String>,
}

pub(crate) async fn dynamic_register(
    Json(input): Json<Registration>,
) -> Result<Response, StatusCode> {
    if !(1..=8).contains(&input.redirect_uris.len())
        || !input
            .redirect_uris
            .iter()
            .all(|uri| RedirectDetails::parse(uri).is_some())
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok((
        StatusCode::CREATED,
        [("cache-control", "no-store")],
        Json(json!({
            "client_id": uuid::Uuid::new_v4().to_string(),
            "redirect_uris": input.redirect_uris,
            "token_endpoint_auth_method": "none",
            "grant_types": ["authorization_code", "refresh_token"],
            "response_types": ["code"],
        })),
    )
        .into_response())
}

#[derive(PartialEq, Eq)]
enum Decision {
    Pending,
    Approved { unattended_changes: bool },
    Declined,
}

struct Connection {
    id: String,
    browser_hash: String,
    match_code: String,
    attempts_left: u8,
    request: Authorization,
    redirect: RedirectDetails,
    remote: bool,
    created: u64,
    expires: u64,
    decision: Decision,
}

#[derive(Default)]
pub struct ConnectionStore {
    entries: Vec<Connection>,
}

impl ConnectionStore {
    fn prune(&mut self, now: u64) {
        self.entries.retain(|entry| entry.expires > now);
    }
}

pub(super) fn start(
    state: &SharedState,
    s: &mut AppState,
    request: Authorization,
    remote: bool,
    now: u64,
) -> Result<Response, StatusCode> {
    let redirect = RedirectDetails::parse(&request.redirect_uri).ok_or(StatusCode::BAD_REQUEST)?;
    if request.response_type != "code"
        || !(1..=128).contains(&request.client_id.chars().count())
        || request.state.len() > 8192
        || !request.challenge_valid(!redirect.home_assistant)
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    s.assistant_connections.prune(now);
    let entries = &mut s.assistant_connections.entries;
    if let Some(i) = entries.iter().position(|c| {
        c.decision == Decision::Pending
            && c.request.client_id == request.client_id
            && c.request.redirect_uri == request.redirect_uri
    }) {
        let old = entries.remove(i);
        attention::resolved(s, "connection", &old.id, old.expires);
    } else if entries
        .iter()
        .filter(|c| c.decision == Decision::Pending)
        .count()
        >= MAX_PENDING
    {
        return Ok(message(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many waiting requests. Decline waiting requests in the Rhythm app and try again.",
        ));
    }

    let browser_secret = generate_raw_token("wait_");
    let entry = Connection {
        id: uuid::Uuid::new_v4().to_string(),
        browser_hash: hash_token(&browser_secret),
        match_code: rand::thread_rng().gen_range(10..=99).to_string(),
        attempts_left: 3,
        request,
        redirect,
        remote,
        created: now,
        expires: now + REQUEST_MS,
        decision: Decision::Pending,
    };
    let response = waiting_page(&entry, &browser_secret);
    let id = entry.id.clone();
    let expires = entry.expires;
    s.assistant_connections.entries.push(entry);
    attention::requested(state, s, "connection", &id, expires);
    Ok(response)
}

fn escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(status: StatusCode, head: &str, body: &str) -> Response {
    (status, [
        ("cache-control", "no-store"),
        ("referrer-policy", "no-referrer"),
        ("content-security-policy", "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'"),
    ], Html(format!(r#"<!doctype html><html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Connect to Rhythm</title>{head}</head><body><main>{body}</main></body></html>"#))).into_response()
}

pub(super) fn message(status: StatusCode, text: &str) -> Response {
    page(status, "", &format!("<h1>{}</h1>", escape(text)))
}

fn waiting_page(c: &Connection, browser_secret: &str) -> Response {
    page(
        StatusCode::OK,
        &format!(
            r#"<meta http-equiv="refresh" content="2;url=/api/assistant/oauth/authorize/wait?request={}">"#,
            escape(browser_secret)
        ),
        &format!(
            r#"<h1>Approve on your phone</h1><p>{} wants to connect to your Rhythm hub.</p><p>Enter this code in the Rhythm app.</p><strong style="font-size:4rem">{}</strong>"#,
            escape(&c.redirect.destination),
            escape(&c.match_code)
        ),
    )
}

#[derive(Deserialize)]
pub(crate) struct Wait {
    #[serde(default)]
    request: String,
}

pub(crate) async fn wait(
    State(state): State<SharedState>,
    Query(query): Query<Wait>,
) -> Result<Response, StatusCode> {
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = current_epoch_ms();
    s.assistant_connections.prune(now);
    let browser_hash = hash_token(&query.request);
    let entries = &mut s.assistant_connections.entries;
    let Some(index) = entries
        .iter()
        .position(|c| constant_time_eq(c.browser_hash.as_bytes(), browser_hash.as_bytes()))
    else {
        return Ok(message(
            StatusCode::OK,
            "Request expired. Start again from the client.",
        ));
    };
    let entry = &entries[index];
    match entry.decision {
        Decision::Pending => Ok(waiting_page(entry, &query.request)),
        Decision::Declined => {
            entries.remove(index);
            Ok(message(StatusCode::OK, "Request declined."))
        }
        Decision::Approved { unattended_changes } => {
            let code = generate_raw_token("grant_");
            let response =
                grant_redirect(&entry.request.redirect_uri, &code, &entry.request.state)?;
            let client = AgentClient {
                unattended_changes,
                id: uuid::Uuid::new_v4().to_string(),
                presented_id: Some(entry.request.client_id.clone()),
                redirect_uri: Some(entry.request.redirect_uri.clone()),
                destination: Some(entry.redirect.destination.clone()),
                secret_hash: String::new(),
                activation_hash: None,
                expires_at: now + 60_000,
                grant_hash: Some(hash_token(&code)),
                code_challenge: entry.request.code_challenge.clone(),
                refresh_hash: None,
            };
            let mut auth = s.api_auth.clone();
            insert_client(&mut auth, client, now)?;
            save(&mut s, auth)?;
            // A failed save leaves the approved browser request available for retry.
            s.assistant_connections.entries.remove(index);
            Ok(response)
        }
    }
}

pub(crate) async fn list_connections(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
) -> Result<Response, StatusCode> {
    require_local_owner(info)?;
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    s.assistant_connections.prune(current_epoch_ms());
    let pending: Vec<_> = s.assistant_connections.entries.iter()
        .filter(|c| c.decision == Decision::Pending)
        .map(|c| json!({
            "id": c.id, "destination": c.redirect.destination, "kind": c.redirect.kind(),
            "remote": c.remote, "created_at_epoch_ms": c.created, "expires_at_epoch_ms": c.expires,
        })).collect();
    Ok((
        [("cache-control", "no-store")],
        Json(json!({"connections": pending})),
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum OwnerDecision {
    Approve,
    Decline,
}

#[derive(Deserialize)]
pub(crate) struct ConnectionDecision {
    decision: OwnerDecision,
    #[serde(default)]
    match_code: String,
    #[serde(default)]
    unattended_changes: bool,
}

pub(crate) async fn decide_connection(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    axum::extract::Path(id): axum::extract::Path<String>,
    Json(input): Json<ConnectionDecision>,
) -> Result<Response, StatusCode> {
    require_local_owner(info)?;
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    s.assistant_connections.prune(current_epoch_ms());
    let pending = s
        .assistant_connections
        .entries
        .iter_mut()
        .find(|c| c.id == id && c.decision == Decision::Pending);
    let no_store = [("cache-control", "no-store")];
    let Some(entry) = pending else {
        return Ok((
            StatusCode::CONFLICT,
            no_store,
            Json(json!({"status":"expired"})),
        )
            .into_response());
    };
    let (status, body) = match input.decision {
        OwnerDecision::Decline => {
            entry.decision = Decision::Declined;
            (StatusCode::OK, json!({"status":"declined"}))
        }
        OwnerDecision::Approve
            if constant_time_eq(entry.match_code.as_bytes(), input.match_code.as_bytes()) =>
        {
            entry.decision = Decision::Approved {
                unattended_changes: input.unattended_changes,
            };
            (StatusCode::OK, json!({"status":"approved"}))
        }
        OwnerDecision::Approve => {
            entry.attempts_left -= 1;
            if entry.attempts_left == 0 {
                entry.decision = Decision::Declined;
            }
            (
                StatusCode::CONFLICT,
                json!({"status":"code_mismatch", "attempts_left":entry.attempts_left}),
            )
        }
    };
    if entry.decision != Decision::Pending {
        let expires = entry.expires;
        attention::resolved(&s, "connection", &id, expires);
    }
    Ok((status, no_store, Json(body)).into_response())
}
