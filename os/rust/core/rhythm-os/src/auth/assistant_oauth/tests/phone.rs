use super::*;

fn state() -> SharedState {
    SharedState::default()
}

fn owner(remote: bool) -> Option<Extension<ApiAuthRequestInfo>> {
    Some(Extension(ApiAuthRequestInfo {
        requires_auth: true,
        claim_available: false,
        via_remote_access: remote,
        role: Some(ApiTokenRole::Owner),
        token_expires_at_epoch_ms: None,
    }))
}

fn request(client: &str, uri: &str) -> Authorization {
    serde_json::from_value(json!({"client_id":client, "response_type":"code", "redirect_uri":uri}))
        .unwrap()
}

async fn body(response: Response) -> String {
    String::from_utf8(
        axum::body::to_bytes(response.into_body(), 16384)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap()
}

async fn json_body(response: Response) -> Value {
    serde_json::from_str(&body(response).await).unwrap()
}

async fn pending(s: &SharedState) -> Vec<Value> {
    let response = list_connections(State(s.clone()), owner(false))
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    json_body(response).await["connections"]
        .as_array()
        .unwrap()
        .clone()
}

struct Browser {
    secret: String,
    match_code: String,
}

async fn start(s: &SharedState, request: Authorization) -> Browser {
    let response = consent(State(s.clone()), None, Query(request))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["referrer-policy"], "no-referrer");
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.headers()["content-security-policy"],
        "default-src 'none'; style-src 'unsafe-inline'; frame-ancestors 'none'; base-uri 'none'"
    );
    let html = body(response).await;
    assert!(html.contains("Approve on your phone"));
    let secret = html
        .split("/wait?request=")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned();
    let match_code = html
        .split("<strong ")
        .nth(1)
        .unwrap()
        .split('>')
        .nth(1)
        .unwrap()
        .split('<')
        .next()
        .unwrap()
        .to_owned();
    assert!((10..=99).contains(&match_code.parse::<u8>().unwrap()));
    Browser { secret, match_code }
}

async fn poll(s: &SharedState, secret: &str) -> Response {
    wait(
        State(s.clone()),
        Query(serde_json::from_value(json!({"request": secret})).unwrap()),
    )
    .await
    .unwrap()
}

async fn decide(s: &SharedState, id: &str, input: Value) -> (StatusCode, Value) {
    let response = decide_connection(
        State(s.clone()),
        owner(false),
        axum::extract::Path(id.to_owned()),
        Json(serde_json::from_value(input).unwrap()),
    )
    .await
    .unwrap();
    (response.status(), json_body(response).await)
}

async fn approve_browser(
    s: &SharedState,
    request: Authorization,
    unattended: bool,
) -> reqwest::Url {
    let browser = start(s, request).await;
    let pending = pending(s).await;
    assert_eq!(pending.len(), 1);
    let id = pending[0]["id"].as_str().unwrap();
    let (status, decision) = decide(s, id, json!({"decision":"approve", "match_code":browser.match_code, "unattended_changes":unattended})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(decision, json!({"status":"approved"}));
    assert_ne!(browser.secret, id);
    let response = poll(s, &browser.secret).await;
    assert_eq!(response.status(), StatusCode::SEE_OTHER);
    assert!(body(poll(s, &browser.secret).await)
        .await
        .contains("Request expired"));
    reqwest::Url::parse(response.headers()["location"].to_str().unwrap()).unwrap()
}

fn grant(url: &reqwest::Url) -> String {
    url.query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .into_owned()
}

fn exchange_grant(
    s: &SharedState,
    client: &str,
    uri: &str,
    secret: Option<&str>,
    code: &str,
    verifier: Option<&str>,
) -> Result<Value, StatusCode> {
    redeem(
        s,
        &HeaderMap::new(),
        &Exchange {
            grant_type: "authorization_code".into(),
            client_id: Some(client.into()),
            client_secret: secret.map(str::to_owned),
            redirect_uri: Some(uri.into()),
            code: Some(code.into()),
            code_verifier: verifier.map(str::to_owned),
            ..Default::default()
        },
    )
}

fn refresh(s: &SharedState, secret: &str, token: &Value) -> Result<Value, StatusCode> {
    redeem(
        s,
        &HeaderMap::new(),
        &Exchange {
            grant_type: "refresh_token".into(),
            client_id: Some("rhythm".into()),
            client_secret: Some(secret.into()),
            refresh_token: Some(token["refresh_token"].as_str().unwrap().into()),
            ..Default::default()
        },
    )
}

#[tokio::test]
async fn owner_approval_mints_one_registration_and_refresh_is_bound_to_it() {
    let s = state();
    let mut req = request("rhythm", HA_REDIRECT);
    req.state = "return & continue".into();
    let browser = start(&s, req).await;
    let listed = pending(&s).await;
    let row = &listed[0];
    assert_eq!(row.as_object().unwrap().len(), 6);
    assert_eq!(row["destination"], "Home Assistant");
    assert_eq!(row["kind"], "home_assistant");
    assert_eq!(row["remote"], false);
    assert_eq!(
        row["expires_at_epoch_ms"].as_u64().unwrap() - row["created_at_epoch_ms"].as_u64().unwrap(),
        300_000
    );
    let id = row["id"].as_str().unwrap();
    assert!(body(poll(&s, id).await).await.contains("Request expired"));
    assert!(body(poll(&s, &browser.secret).await)
        .await
        .contains(&browser.match_code));
    assert_eq!(
        decide(
            &s,
            id,
            json!({"decision":"approve","match_code":browser.match_code})
        )
        .await,
        (StatusCode::OK, json!({"status":"approved"}))
    );
    assert!(s.lock().unwrap().api_auth.agent_clients.is_empty());
    let redirect = poll(&s, &browser.secret).await;
    let url = reqwest::Url::parse(redirect.headers()["location"].to_str().unwrap()).unwrap();
    assert!(url
        .query_pairs()
        .any(|(k, v)| k == "state" && v == "return & continue"));
    let code = grant(&url);
    let tokens = exchange_grant(&s, "rhythm", HA_REDIRECT, Some("rhythm"), &code, None).unwrap();
    assert_eq!(tokens["expires_in"], 3600);
    assert!(exchange_grant(&s, "rhythm", HA_REDIRECT, Some("rhythm"), &code, None).is_err());
    assert!(refresh(&s, "other", &tokens).is_err());
    let rotated = refresh(&s, "rhythm", &tokens).unwrap();
    assert_ne!(tokens["refresh_token"], rotated["refresh_token"]);
    let id = s.lock().unwrap().api_auth.agent_clients[0].id.clone();
    let verify = |v: &Value| {
        s.lock()
            .unwrap()
            .api_auth
            .verify_token_role(v["access_token"].as_str().unwrap())
    };
    assert_eq!(verify(&tokens), None);
    assert_eq!(verify(&rotated), Some(ApiTokenRole::Agent));
    assert_ne!(id, "rhythm");
    assert_eq!(
        manage_agent_token(&s, owner(false).map(|v| v.0), Some(&id), false).status,
        204
    );
    assert_eq!(verify(&rotated), None);
    assert!(refresh(&s, "rhythm", &rotated).is_err());
}

#[tokio::test]
async fn registrations_sharing_a_client_id_redeem_and_revoke_independently() {
    let s = state();
    let a = grant(&approve_browser(&s, request("rhythm", HA_REDIRECT), false).await);
    let b = grant(&approve_browser(&s, request("rhythm", HA_REDIRECT), true).await);
    let b_token = exchange_grant(&s, "rhythm", HA_REDIRECT, Some("b"), &b, None).unwrap();
    let a_token = exchange_grant(&s, "rhythm", HA_REDIRECT, Some("a"), &a, None).unwrap();
    let auth = s.lock().unwrap().api_auth.clone();
    assert_eq!(auth.agent_tokens.len(), 2);
    assert!(
        !auth
            .verify_token_info(a_token["access_token"].as_str().unwrap())
            .unwrap()
            .unattended_changes
    );
    assert!(
        auth.verify_token_info(b_token["access_token"].as_str().unwrap())
            .unwrap()
            .unattended_changes
    );
    assert!(refresh(&s, "a", &b_token).is_err());
    assert!(refresh(&s, "b", &a_token).is_err());
    let a_id = &auth.agent_clients[0].id;
    assert_eq!(
        manage_agent_token(&s, owner(false).map(|v| v.0), Some(a_id), false).status,
        204
    );
    assert!(refresh(&s, "a", &a_token).is_err());
    assert!(refresh(&s, "b", &b_token).is_ok());
}

#[tokio::test]
async fn wrong_codes_decline_and_an_explicit_decline_needs_no_code() {
    let s = state();
    let browser = start(&s, request("rhythm", HA_REDIRECT)).await;
    let id = pending(&s).await[0]["id"].as_str().unwrap().to_owned();
    for attempts in (0..3).rev() {
        assert_eq!(
            decide(&s, &id, json!({"decision":"approve","match_code":"00"})).await,
            (
                StatusCode::CONFLICT,
                json!({"status":"code_mismatch", "attempts_left":attempts})
            )
        );
    }
    assert!(pending(&s).await.is_empty());
    assert_eq!(
        decide(
            &s,
            &id,
            json!({"decision":"approve","match_code":browser.match_code})
        )
        .await,
        (StatusCode::CONFLICT, json!({"status":"expired"}))
    );
    assert!(body(poll(&s, &browser.secret).await)
        .await
        .contains("Request declined"));
    let browser = start(&s, request("rhythm", HA_REDIRECT)).await;
    let id = pending(&s).await[0]["id"].as_str().unwrap().to_owned();
    assert_eq!(
        decide(&s, &id, json!({"decision":"decline"})).await,
        (StatusCode::OK, json!({"status":"declined"}))
    );
    assert!(body(poll(&s, &browser.secret).await)
        .await
        .contains("Request declined"));
    assert!(s.lock().unwrap().api_auth.agent_clients.is_empty());
}

#[tokio::test]
async fn public_registration_requires_valid_redirect_and_binds_pkce() {
    let uri = "http://127.0.0.1:7777/cb?client=one";
    let register_uris = |uris: Vec<&str>| {
        dynamic_register(Json(
            serde_json::from_value(json!({"redirect_uris":uris})).unwrap(),
        ))
    };
    for bad in [
        vec![],
        vec![uri; 9],
        vec!["http://192.168.1.5/cb"],
        vec!["https://u@host/cb"],
        vec!["https://host/cb#fragment"],
        vec!["https://@host/cb"],
        vec!["file:///tmp/cb"],
    ] {
        assert_eq!(
            register_uris(bad).await.unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }
    let valid = vec![
        uri,
        "http://homeassistant.local:8123/auth/external/callback",
        "http://[::1]:7777/cb",
        "https://example.com/cb",
    ];
    let registration = register_uris(valid.clone()).await.unwrap();
    assert_eq!(registration.status(), StatusCode::CREATED);
    let reg = json_body(registration).await;
    assert_eq!(reg["token_endpoint_auth_method"], "none");
    assert_eq!(reg["redirect_uris"], json!(valid));
    let client = reg["client_id"].as_str().unwrap();
    let s = state();
    assert_eq!(
        consent(State(s.clone()), None, Query(request(client, uri)))
            .await
            .unwrap_err(),
        StatusCode::BAD_REQUEST
    );
    let mut req = request(client, uri);
    let verifier = "v".repeat(43);
    req.code_challenge = Some(URL_SAFE_NO_PAD.encode(Sha256::digest(&verifier)));
    req.code_challenge_method = Some("S256".into());
    let url = approve_browser(&s, req, false).await;
    assert!(url.query_pairs().any(|(k, v)| k == "client" && v == "one"));
    assert!(!url.query_pairs().any(|(k, _)| k == "state"));
    let code = grant(&url);
    assert!(exchange_grant(&s, client, HA_REDIRECT, None, &code, Some(&verifier)).is_err());
    assert!(exchange_grant(
        &s,
        client,
        uri,
        Some("should-not-bind"),
        &code,
        Some(&"w".repeat(43))
    )
    .is_err());
    assert!(exchange_grant(&s, client, uri, None, &code, Some(&verifier)).is_ok());
}

#[tokio::test]
async fn remote_access_requires_opt_in_and_only_local_owners_can_decide() {
    let s = state();
    let response = consent(
        State(s.clone()),
        owner(true),
        Query(request("rhythm", HA_REDIRECT)),
    )
    .await
    .unwrap();
    assert_eq!(response.status(), StatusCode::FORBIDDEN);
    assert!(pending(&s).await.is_empty());
    s.lock().unwrap().pro_settings.remote_mcp = true;
    let uri = "http://HomeASSISTANT.local:8123/auth/external/callback";
    assert_eq!(
        consent(State(s.clone()), owner(true), Query(request("rhythm", uri)))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    assert_eq!(
        list_connections(State(s.clone()), owner(true))
            .await
            .unwrap_err(),
        StatusCode::FORBIDDEN
    );
    let list = pending(&s).await;
    assert_eq!(list[0]["remote"], true);
    assert_eq!(list[0]["destination"], "homeassistant.local");
    assert_eq!(list[0]["kind"], "home_assistant");
    assert_eq!(
        decide_connection(
            State(s),
            owner(true),
            axum::extract::Path(list[0]["id"].as_str().unwrap().to_owned()),
            Json(serde_json::from_value(json!({"decision":"decline"})).unwrap())
        )
        .await
        .unwrap_err(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn pending_limit_replaces_repeated_requests_and_bounds_authorization_inputs() {
    let s = state();
    for client in ["", &"a".repeat(129)] {
        assert_eq!(
            consent(State(s.clone()), None, Query(request(client, HA_REDIRECT)))
                .await
                .unwrap_err(),
            StatusCode::BAD_REQUEST
        );
    }
    let mut req = request("rhythm", HA_REDIRECT);
    req.state = "a".repeat(8193);
    assert_eq!(
        consent(State(s.clone()), None, Query(req))
            .await
            .unwrap_err(),
        StatusCode::BAD_REQUEST
    );
    let mut browsers = vec![];
    for client in ["a", "b", "c", "d"] {
        browsers.push(start(&s, request(client, HA_REDIRECT)).await);
    }
    assert_eq!(
        consent(State(s.clone()), None, Query(request("e", HA_REDIRECT)))
            .await
            .unwrap()
            .status(),
        StatusCode::TOO_MANY_REQUESTS
    );
    let replacement = start(&s, request("b", HA_REDIRECT)).await;
    assert_eq!(pending(&s).await.len(), 4);
    assert_ne!(replacement.secret, browsers[1].secret);
    let expired = body(poll(&s, &browsers[1].secret).await).await;
    assert!(expired.contains("Request expired") && !expired.contains("refresh"));
}

#[tokio::test]
async fn older_app_receives_the_activation_page_only_while_activation_is_live() {
    let s = state();
    let response = register(State(s.clone()), owner(false), axum::body::Bytes::new())
        .await
        .unwrap();
    let reg = json_body(response).await;
    let request = || {
        let mut req = request(reg["client_id"].as_str().unwrap(), HA_REDIRECT);
        req.state = "fixture".into();
        req
    };
    let html = body(
        consent(State(s.clone()), None, Query(request()))
            .await
            .unwrap(),
    )
    .await;
    assert!(html.contains("Activation code"));
    assert!(pending(&s).await.is_empty());
    s.lock().unwrap().api_auth.agent_clients[0].expires_at = 0;
    start(&s, request()).await;
    assert_eq!(pending(&s).await.len(), 1);
}

#[tokio::test]
async fn approved_browser_expires_when_the_owner_credentials_are_reset() {
    for reset in ["factory", "clear", "replace"] {
        let state = state();
        crate::auth::install_ephemeral_owner_token(&state, &"a".repeat(64)).unwrap();
        let browser = start(&state, request("rhythm", HA_REDIRECT)).await;
        let requests = pending(&state).await;
        let id = requests[0]["id"].as_str().unwrap();
        let (status, _) = decide(
            &state,
            id,
            json!({"decision":"approve","match_code":browser.match_code}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        match reset {
            "factory" => {
                let response = crate::handlers::handle_post_factory_reset(&state);
                assert_eq!(response.status, 200, "{}", response.body);
            }
            "clear" => crate::auth::clear_api_auth(&state).unwrap(),
            _ => crate::auth::install_ephemeral_owner_token(&state, &"b".repeat(64)).unwrap(),
        }
        let result = poll(&state, &browser.secret).await;
        assert!(!result.headers().contains_key("location"), "{reset}");
        assert!(body(result).await.contains("Request expired"), "{reset}");
        assert!(state.lock().unwrap().api_auth.agent_clients.is_empty());
    }
}

#[tokio::test]
async fn metadata_uses_the_direct_or_configured_tunnel_origin() {
    use crate::remote_access::StoredRemoteAccessConfig;
    use crate::storage::{FileStorage, Storage};
    let s = state();
    let root = "/.well-known/oauth-authorization-server";
    let scoped = "/.well-known/oauth-authorization-server/api/assistant/mcp";
    let mut headers = HeaderMap::new();
    headers.insert("host", "[::1]:54448".parse().unwrap());
    headers.insert("x-forwarded-host", "other.example".parse().unwrap());
    headers.insert("x-forwarded-proto", "https".parse().unwrap());
    let response = metadata(
        State(s.clone()),
        None,
        root.parse().unwrap(),
        headers.clone(),
    )
    .await;
    assert_eq!(response.headers()["cache-control"], "no-store");
    let document = json_body(response).await;
    assert_eq!(document["issuer"], "http://[::1]:54448");
    assert_eq!(
        document["registration_endpoint"],
        "http://[::1]:54448/api/assistant/oauth/register"
    );
    assert_eq!(
        json_body(
            metadata(
                State(s.clone()),
                None,
                scoped.parse().unwrap(),
                headers.clone()
            )
            .await
        )
        .await["issuer"],
        "http://[::1]:54448/api/assistant/mcp"
    );
    let dir = tempfile::tempdir().unwrap();
    let storage = std::sync::Arc::new(FileStorage::new(dir.path().to_str().unwrap()).unwrap());
    storage
        .save_remote_access_config(&StoredRemoteAccessConfig {
            schema_version: 1,
            enabled: true,
            hostname: "hub.example".into(),
            connector_token: "fixture".into(),
            tunnel_id: None,
            tunnel_name: None,
            updated_at_epoch_ms: 0,
        })
        .unwrap();
    s.lock().unwrap().storage = Some(storage);
    let tunnel = json_body(
        metadata(
            State(s.clone()),
            owner(true),
            root.parse().unwrap(),
            headers.clone(),
        )
        .await,
    )
    .await;
    assert_eq!(tunnel["issuer"], "https://hub.example");
    assert_eq!(
        tunnel["token_endpoint"],
        "https://hub.example/api/assistant/oauth/token"
    );
    for bad_host in [
        "bad.example/other",
        "evil@elsewhere.example",
        "example.com:99999",
        "example%2f.test",
    ] {
        headers.insert("host", bad_host.parse().unwrap());
        let doc = json_body(
            metadata(
                State(s.clone()),
                None,
                root.parse().unwrap(),
                headers.clone(),
            )
            .await,
        )
        .await;
        assert!(doc.get("issuer").is_none());
        assert_eq!(doc["token_endpoint"], "/api/assistant/oauth/token");
    }
}
