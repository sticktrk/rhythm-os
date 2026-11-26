//! Local owner approval for assistant OAuth clients, including older apps
//! that pair a Home Assistant registration using an activation code.
//! Each grant, access token and refresh token belongs to that registration.
use super::*;
use axum::extract::{Extension, Form, Query};
use axum::http::HeaderMap;
use axum::response::{Html, Redirect};
use axum::Json;
use base64::{
    engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD},
    Engine,
};

mod connections;
pub use connections::ConnectionStore;
pub(crate) use connections::{decide_connection, dynamic_register, list_connections, wait};

const HA_REDIRECT: &str = "https://my.home-assistant.io/redirect/oauth";
const PAIR_MS: u64 = 10 * 60 * 1000;
const TOKEN_SECONDS: u64 = 3600;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentClient {
    #[serde(default)]
    pub(crate) unattended_changes: bool,
    pub(crate) id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    presented_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    redirect_uri: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) destination: Option<String>,
    secret_hash: String,
    activation_hash: Option<String>,
    expires_at: u64,
    grant_hash: Option<String>,
    code_challenge: Option<String>,
    refresh_hash: Option<String>,
}

impl AgentClient {
    pub(crate) fn kind(&self) -> &'static str {
        match &self.redirect_uri {
            None => "home_assistant",
            Some(uri)
                if connections::RedirectDetails::parse(uri).is_some_and(|r| r.home_assistant) =>
            {
                "home_assistant"
            }
            _ => "oauth",
        }
    }
}

pub fn public_route(method: &Method, path: &str) -> bool {
    matches!(
        (method, path),
        (
            &Method::GET,
            "/.well-known/oauth-authorization-server"
                | "/.well-known/oauth-authorization-server/api/assistant/mcp"
                | "/api/assistant/oauth/authorize"
                | "/api/assistant/oauth/authorize/wait"
        ) | (
            &Method::POST,
            "/api/assistant/oauth/authorize"
                | "/api/assistant/oauth/token"
                | "/api/assistant/oauth/register"
        )
    )
}

fn http_origin(scheme: &str, authority: &str) -> Option<String> {
    // A Host field is authority only. Never let URL parsing reinterpret a
    // path, userinfo, percent escape or backslash as part of our origin.
    if !matches!(scheme, "http" | "https")
        || authority.is_empty()
        || !authority
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || b".-_:[]".contains(&c))
    {
        return None;
    }
    let _: axum::http::uri::Authority = authority.parse().ok()?;
    let url = reqwest::Url::parse(&format!("{scheme}://{authority}")).ok()?;
    Some(url.origin().ascii_serialization())
}

pub(crate) async fn metadata(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    uri: Uri,
    headers: HeaderMap,
) -> Response {
    let origin = if info.is_some_and(|Extension(i)| i.via_remote_access) {
        // cloudflared terminates TLS. Its external origin is configured on
        // this hub, never inferred from forwarding headers.
        let storage = state.lock().ok().and_then(|s| s.storage.clone());
        tokio::task::spawn_blocking(move || {
            let config = storage?.load_remote_access_config().ok()??;
            config
                .enabled
                .then(|| http_origin("https", &config.hostname))
                .flatten()
        })
        .await
        .ok()
        .flatten()
    } else {
        let authority = uri
            .authority()
            .map(|a| a.as_str())
            .or_else(|| headers.get("host")?.to_str().ok());
        authority.and_then(|a| http_origin(uri.scheme_str().unwrap_or("http"), a))
    };
    // HA also supports relative endpoints on an origin-less internal request.
    let base = origin.as_deref().unwrap_or("");
    let mut document = json!({
        "authorization_endpoint":format!("{base}/api/assistant/oauth/authorize"),
        "token_endpoint":format!("{base}/api/assistant/oauth/token"),
        "registration_endpoint":format!("{base}/api/assistant/oauth/register"),
        "response_types_supported":["code"],
        "grant_types_supported":["authorization_code","refresh_token"],
        "token_endpoint_auth_methods_supported":["client_secret_post","client_secret_basic","none"],
        "code_challenge_methods_supported":["S256"]
    });
    if let Some(origin) = origin {
        // RFC 8414 inserts the well-known segment before the issuer path.
        let issuer_path = uri
            .path()
            .strip_prefix("/.well-known/oauth-authorization-server")
            .unwrap_or("");
        document["issuer"] = json!(format!("{origin}{issuer_path}"));
    }
    ([("cache-control", "no-store")], Json(document)).into_response()
}

fn save(s: &mut crate::state::AppState, auth: StoredApiAuth) -> Result<(), StatusCode> {
    if let Some(storage) = &s.storage {
        storage
            .save_api_auth(&auth)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }
    s.api_auth = auth;
    Ok(())
}

fn require_local_owner(info: Option<Extension<ApiAuthRequestInfo>>) -> Result<(), StatusCode> {
    if info.is_some_and(|Extension(i)| i.role == Some(ApiTokenRole::Owner) && !i.via_remote_access)
    {
        Ok(())
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

fn insert_client(
    auth: &mut StoredApiAuth,
    client: AgentClient,
    now: u64,
) -> Result<(), StatusCode> {
    auth.agent_clients
        .retain(|c| c.refresh_hash.is_some() || c.expires_at > now);
    if auth.agent_clients.len() >= 32 {
        return Err(StatusCode::CONFLICT);
    }
    auth.agent_clients.push(client);
    Ok(())
}

pub(crate) async fn register(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    body: axum::body::Bytes,
) -> Result<Response, StatusCode> {
    require_local_owner(info)?;
    let options = AgentOptions::parse(&body)?;
    let client_id = uuid::Uuid::new_v4().to_string();
    let client_secret = generate_raw_token("rhythm_ha_client_");
    let activation = generate_raw_token("pair_");
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut auth = s.api_auth.clone();
    let now = current_epoch_ms();
    insert_client(
        &mut auth,
        AgentClient {
            presented_id: None,
            redirect_uri: None,
            destination: None,
            unattended_changes: options.unattended_changes,
            id: client_id.clone(),
            secret_hash: hash_token(&client_secret),
            activation_hash: Some(hash_token(&activation)),
            expires_at: now + PAIR_MS,
            grant_hash: None,
            code_challenge: None,
            refresh_hash: None,
        },
        now,
    )?;
    save(&mut s, auth)?;
    Ok(([("cache-control","no-store")],Json(json!({"client_id":client_id,"client_secret":client_secret,"activation_code":activation,"expires_in":PAIR_MS/1000,"unattended_changes":options.unattended_changes}))).into_response())
}

#[derive(Deserialize)]
pub(crate) struct Authorization {
    client_id: String,
    response_type: String,
    redirect_uri: String,
    #[serde(default)]
    state: String,
    code_challenge: Option<String>,
    code_challenge_method: Option<String>,
}

impl Authorization {
    fn challenge_valid(&self, required: bool) -> bool {
        match (&self.code_challenge, self.code_challenge_method.as_deref()) {
            (None, None) => !required,
            (Some(c), Some("S256")) => {
                c.len() == 43
                    && URL_SAFE_NO_PAD
                        .decode(c)
                        .is_ok_and(|bytes| bytes.len() == 32)
            }
            _ => false,
        }
    }

    fn valid_activation(&self) -> bool {
        self.response_type == "code"
            && self.redirect_uri == HA_REDIRECT
            && (1..=8192).contains(&self.state.len())
            && self.challenge_valid(false)
    }
}

pub(crate) async fn consent(
    State(state): State<SharedState>,
    info: Option<Extension<ApiAuthRequestInfo>>,
    Query(request): Query<Authorization>,
) -> Result<Response, StatusCode> {
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let now = current_epoch_ms();
    if request.valid_activation()
        && s.api_auth.agent_clients.iter().any(|client| {
            client.presented_id.is_none()
                && client.id == request.client_id
                && client.activation_hash.is_some()
                && client.expires_at > now
        })
    {
        return Ok(activation_page());
    }
    let remote = info.is_some_and(|Extension(i)| i.via_remote_access);
    if remote && !s.pro_settings.remote_mcp {
        return Ok(connections::message(
            StatusCode::FORBIDDEN,
            "Remote connections are turned off on this hub.",
        ));
    }
    connections::start(&state, &mut s, request, remote, now)
}

fn activation_page() -> Response {
    ([("cache-control","no-store"),("referrer-policy","no-referrer"),
      ("content-security-policy","default-src 'none'; form-action 'self' https://my.home-assistant.io; frame-ancestors 'none'; base-uri 'none'")],
     Html("<!doctype html><html lang='en'><meta charset='utf-8'><meta name='viewport' content='width=device-width, initial-scale=1'><title>Connect Home Assistant to Rhythm</title><h1>Connect Home Assistant to Rhythm</h1><p>Assist will be able to inspect this hub, control lights and propose changes. Enter the activation code issued by this hub to approve.</p><form method='post'><label>Activation code <input name='activation_code' type='password' required autocomplete='off'></label><button>Connect Assist</button></form></html>")).into_response()
}

#[derive(Deserialize)]
pub(crate) struct Approval {
    activation_code: String,
}

pub(crate) async fn approve(
    State(state): State<SharedState>,
    Query(request): Query<Authorization>,
    Form(form): Form<Approval>,
) -> Result<Response, StatusCode> {
    if !request.valid_activation() {
        return Err(StatusCode::BAD_REQUEST);
    }
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut auth = s.api_auth.clone();
    let client = auth
        .agent_clients
        .iter_mut()
        .find(|c| {
            c.presented_id.is_none()
                && c.id == request.client_id
                && c.expires_at > current_epoch_ms()
                && matches_hash(c.activation_hash.as_deref(), &form.activation_code)
        })
        .ok_or(StatusCode::FORBIDDEN)?;
    let code = generate_raw_token("grant_");
    client.activation_hash = None;
    client.grant_hash = Some(hash_token(&code));
    client.expires_at = current_epoch_ms() + 60_000;
    client.code_challenge = request.code_challenge;
    save(&mut s, auth)?;
    grant_redirect(HA_REDIRECT, &code, &request.state)
}

fn grant_redirect(uri: &str, code: &str, state: &str) -> Result<Response, StatusCode> {
    let mut target = reqwest::Url::parse(uri).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    target.query_pairs_mut().append_pair("code", code);
    if !state.is_empty() {
        target.query_pairs_mut().append_pair("state", state);
    }
    Ok((
        [
            ("cache-control", "no-store"),
            ("referrer-policy", "no-referrer"),
        ],
        Redirect::to(target.as_str()),
    )
        .into_response())
}

fn matches_hash(hash: Option<&str>, secret: &str) -> bool {
    hash.is_some_and(|h| constant_time_eq(h.as_bytes(), hash_token(secret).as_bytes()))
}

#[derive(Default, Deserialize)]
pub(crate) struct Exchange {
    grant_type: String,
    client_id: Option<String>,
    client_secret: Option<String>,
    refresh_token: Option<String>,
    code: Option<String>,
    redirect_uri: Option<String>,
    code_verifier: Option<String>,
}

pub(crate) async fn exchange(
    State(state): State<SharedState>,
    headers: HeaderMap,
    Form(form): Form<Exchange>,
) -> Response {
    let result = redeem(&state, &headers, &form);
    let (status, body) = match result {
        Ok(body) => (StatusCode::OK, body),
        Err(StatusCode::INTERNAL_SERVER_ERROR) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({"error":"server_error"}),
        ),
        Err(status) => (status, json!({"error":"invalid_grant"})),
    };
    (
        status,
        [("cache-control", "no-store"), ("pragma", "no-cache")],
        Json(body),
    )
        .into_response()
}

fn redeem(state: &SharedState, headers: &HeaderMap, form: &Exchange) -> Result<Value, StatusCode> {
    let basic = headers
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Basic "))
        .and_then(|data| STANDARD.decode(data).ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    let (id, secret) = basic
        .as_deref()
        .and_then(|b| b.split_once(':'))
        .or_else(|| {
            form.client_id
                .as_deref()
                .map(|id| (id, form.client_secret.as_deref().unwrap_or_default()))
        })
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let mut s = state
        .lock()
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let mut auth = s.api_auth.clone();
    let now = current_epoch_ms();
    let client = auth
        .agent_clients
        .iter_mut()
        .find(|c| match &c.presented_id {
            None => c.id == id && matches_hash(Some(&c.secret_hash), secret),
            Some(presented) => {
                let (stored, supplied) = match form.grant_type.as_str() {
                    "authorization_code" => (&c.grant_hash, &form.code),
                    "refresh_token" => (&c.refresh_hash, &form.refresh_token),
                    _ => return false,
                };
                presented == id
                    && supplied
                        .as_ref()
                        .is_some_and(|v| matches_hash(stored.as_deref(), v))
                    && (c.secret_hash.is_empty() || matches_hash(Some(&c.secret_hash), secret))
            }
        })
        .ok_or(StatusCode::UNAUTHORIZED)?;
    match form.grant_type.as_str() {
        "authorization_code"
            if client.expires_at > now
                && form.redirect_uri.as_deref()
                    == Some(client.redirect_uri.as_deref().unwrap_or(HA_REDIRECT))
                && form
                    .code
                    .as_deref()
                    .is_some_and(|code| matches_hash(client.grant_hash.as_deref(), code)) =>
        {
            if let Some(challenge) = &client.code_challenge {
                let verifier = form
                    .code_verifier
                    .as_deref()
                    .ok_or(StatusCode::BAD_REQUEST)?;
                if !(43..=128).contains(&verifier.len())
                    || *challenge != URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
                {
                    return Err(StatusCode::BAD_REQUEST);
                }
            }
            if client.presented_id.is_some() && client.secret_hash.is_empty() {
                client.secret_hash = hash_token(secret);
            }
            client.grant_hash = None;
        }
        "refresh_token"
            if form
                .refresh_token
                .as_deref()
                .is_some_and(|token| matches_hash(client.refresh_hash.as_deref(), token)) => {}
        _ => return Err(StatusCode::BAD_REQUEST),
    }
    let refresh = generate_raw_token("rhythm_ha_refresh_");
    client.refresh_hash = Some(hash_token(&refresh));
    let token = generate_raw_token(AGENT_PREFIX);
    auth.agent_tokens.retain(|t| t.id != client.id);
    auth.agent_tokens.push(StoredApiToken {
        support_principal: None,
        unattended_changes: client.unattended_changes,
        id: client.id.clone(),
        role: ApiTokenRole::Agent,
        token_hash: hash_token(&token),
        created_at_epoch_ms: now,
        label: None,
        expires_at_epoch_ms: Some(now + TOKEN_SECONDS * 1000),
    });
    save(&mut s, auth)?;
    Ok(
        json!({"access_token":token,"refresh_token":refresh,"token_type":"Bearer","expires_in":TOKEN_SECONDS}),
    )
}

#[cfg(test)]
mod tests {
    mod phone;

    use super::*;
    use crate::state::AppState;
    #[tokio::test]
    async fn ha_approval_exchange_refresh_and_revocation_are_bound_to_the_registration() {
        let state: SharedState = std::sync::Arc::new(std::sync::Mutex::new(AppState::default()));
        let owner = ApiAuthRequestInfo {
            requires_auth: true,
            claim_available: false,
            via_remote_access: false,
            role: Some(ApiTokenRole::Owner),
            token_expires_at_epoch_ms: None,
        };
        let response = register(
            State(state.clone()),
            Some(Extension(owner)),
            axum::body::Bytes::from_static(br#"{"unattended_changes":true}"#),
        )
        .await
        .unwrap();
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let reg: Value = serde_json::from_slice(&bytes).unwrap();
        let request = || Authorization {
            client_id: reg["client_id"].as_str().unwrap().into(),
            response_type: "code".into(),
            redirect_uri: HA_REDIRECT.into(),
            state: "fixture".into(),
            code_challenge: None,
            code_challenge_method: None,
        };
        let form = || {
            Form(Approval {
                activation_code: reg["activation_code"].as_str().unwrap().into(),
            })
        };
        let mut other = request();
        other.redirect_uri = "https://other.example".into();
        assert_eq!(
            approve(State(state.clone()), Query(other), form())
                .await
                .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
        let response = approve(State(state.clone()), Query(request()), form())
            .await
            .unwrap();
        assert_eq!(
            approve(State(state.clone()), Query(request()), form())
                .await
                .unwrap_err(),
            StatusCode::FORBIDDEN
        );
        let redirect =
            reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap();
        let code = redirect
            .query_pairs()
            .find(|(k, _)| k == "code")
            .unwrap()
            .1
            .into_owned();
        let initial = Exchange {
            grant_type: "authorization_code".into(),
            client_id: Some(request().client_id),
            client_secret: Some(reg["client_secret"].as_str().unwrap().into()),
            code: Some(code),
            redirect_uri: Some(HA_REDIRECT.into()),
            ..Default::default()
        };
        let tokens = redeem(&state, &HeaderMap::new(), &initial).unwrap();
        assert!(redeem(&state, &HeaderMap::new(), &initial).is_err());
        let refresh = Exchange {
            grant_type: "refresh_token".into(),
            refresh_token: Some(tokens["refresh_token"].as_str().unwrap().into()),
            ..initial
        };
        let updated = redeem(&state, &HeaderMap::new(), &refresh).unwrap();
        assert!(redeem(&state, &HeaderMap::new(), &refresh).is_err());
        assert!(state.lock().unwrap().api_auth.agent_tokens[0].unattended_changes);
        let verify = |v: &Value| {
            state
                .lock()
                .unwrap()
                .api_auth
                .verify_token_role(v["access_token"].as_str().unwrap())
        };
        assert_eq!(verify(&tokens), None);
        assert_eq!(verify(&updated), Some(ApiTokenRole::Agent));
        assert_eq!(
            manage_agent_token(&state, Some(owner), reg["client_id"].as_str(), false).status,
            204
        );
        assert_eq!(verify(&updated), None);
        assert!(redeem(
            &state,
            &HeaderMap::new(),
            &Exchange {
                refresh_token: Some(updated["refresh_token"].as_str().unwrap().into()),
                ..refresh
            }
        )
        .is_err());
    }
}
