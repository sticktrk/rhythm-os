use super::*;
use axum::{body::to_bytes, http::Method};
use rhythm_os::{
    auth,
    state::AppState,
    storage::{FileStorage, Storage},
};
use serde_json::Value;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Mutex,
};
use tower::ServiceExt;

const ADMIN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: std::path::PathBuf,
    state: SharedState,
    access: Arc<MobileAccess>,
}
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "rhythm-addon-mobile-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let state = load(&root);
        Self {
            root,
            state,
            access: Arc::new(MobileAccess::new(ADMIN).unwrap()),
        }
    }
    fn admin(&self) -> Router {
        create_admin_router(self.state.clone(), self.access.clone())
    }
    fn mobile(&self) -> Router {
        create_mobile_router(self.state.clone(), self.access.clone())
    }
    async fn enroll(&self) -> Value {
        let (status, code) = json_request(
            self.admin(),
            Method::POST,
            "/api/addon/enrollment",
            Some(ADMIN),
            json!({}),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let (status, token) = json_request(
            self.mobile(),
            Method::POST,
            "/api/addon/enrollment/exchange",
            None,
            code,
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        token
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn load(root: &std::path::Path) -> SharedState {
    let mut state = AppState::default();
    state.platform_context = "ha_addon";
    state.platform_type = "desktop";
    state.data_dir = root.to_str().unwrap().into();
    state.storage = Some(Arc::new(FileStorage::new(state.data_dir.as_str()).unwrap()));
    state.event_tx = Some(tokio::sync::broadcast::channel(16).0);
    rhythm_os_runtime_modules::install_default_light_runtime_modules(&mut state).unwrap();
    rhythm_os::storage::load_persisted_state(&mut state);
    state.require_api_auth = true;
    Arc::new(Mutex::new(state))
}

fn request(method: Method, path: &str, token: Option<&str>, body: Value) -> Request<Body> {
    let mut request = Request::builder()
        .method(method)
        .uri(path)
        .header("content-type", "application/json");
    if let Some(token) = token {
        request = request.header("authorization", format!("Bearer {token}"));
    }
    request.body(Body::from(body.to_string())).unwrap()
}
async fn json_request(
    router: Router,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Value,
) -> (StatusCode, Value) {
    let response = router
        .oneshot(request(method, path, token, body))
        .await
        .unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 4 * 1024 * 1024)
        .await
        .unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn each_listener_requires_its_own_credential_and_rejects_ingress_header_forgery() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let token = phone["token"].as_str().unwrap();
    for (router, credential, expected) in [
        (f.mobile(), None, StatusCode::UNAUTHORIZED),
        (f.mobile(), Some(ADMIN), StatusCode::UNAUTHORIZED),
        (f.mobile(), Some(token), StatusCode::OK),
        (f.admin(), Some(token), StatusCode::UNAUTHORIZED),
        (f.admin(), Some(ADMIN), StatusCode::OK),
    ] {
        let mut req = request(Method::GET, "/api/light-breaker", credential, json!({}));
        req.headers_mut()
            .insert("x-hass-is-admin", "true".parse().unwrap());
        req.headers_mut().insert(
            "x-ingress-path",
            "/api/hassio_ingress/fake".parse().unwrap(),
        );
        assert_eq!(router.oneshot(req).await.unwrap().status(), expected);
    }
    assert!(!f.state.lock().unwrap().api_auth.verify_token(ADMIN));
    let (_, status) =
        json_request(f.mobile(), Method::GET, "/api/auth/status", None, json!({})).await;
    assert_eq!(status["requires_auth"], true);
    assert_eq!(status["claim_available"], false);
    assert_eq!(
        json_request(f.mobile(), Method::POST, "/api/auth/claim", None, json!({}))
            .await
            .0,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        json_request(
            f.mobile(),
            Method::POST,
            "/api/addon/enrollment",
            Some(token),
            json!({})
        )
        .await
        .0,
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn enrollment_is_one_use_instance_bound_and_uncacheable() {
    let f = Fixture::new();
    let response = f
        .admin()
        .oneshot(request(
            Method::POST,
            "/api/addon/enrollment",
            Some(ADMIN),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.headers()["cache-control"], "no-store");
    let code: Value =
        serde_json::from_slice(&to_bytes(response.into_body(), 4096).await.unwrap()).unwrap();
    assert_eq!(code["code"].as_str().unwrap().len(), 64);
    let mut wrong_instance = code.clone();
    wrong_instance["server_instance_id"] = json!("some-other-installation");
    assert_eq!(
        json_request(
            f.mobile(),
            Method::POST,
            "/api/addon/enrollment/exchange",
            None,
            wrong_instance
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json_request(
            f.mobile(),
            Method::POST,
            "/api/addon/enrollment/exchange",
            None,
            code.clone()
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        json_request(
            f.mobile(),
            Method::POST,
            "/api/addon/enrollment/exchange",
            None,
            code
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn restart_preserves_phone_and_tunnel_but_discards_pending_enrollment_and_admin_secret() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let (_, pending) = json_request(
        f.admin(),
        Method::POST,
        "/api/addon/enrollment",
        Some(ADMIN),
        json!({}),
    )
    .await;
    let storage = FileStorage::new(f.root.to_str().unwrap()).unwrap();
    let tunnel = rhythm_os::remote_access::StoredRemoteAccessConfig {
        schema_version: 1,
        enabled: false,
        hostname: "fixture.devices.rhythm.lighting".into(),
        connector_token: "fixture-connector-secret".into(),
        tunnel_id: Some("fixture-id".into()),
        tunnel_name: None,
        updated_at_epoch_ms: 1,
    };
    storage.save_remote_access_config(&tunnel).unwrap();
    let restarted = load(&f.root);
    let access = Arc::new(MobileAccess::new(&"b".repeat(64)).unwrap());
    let router = create_mobile_router(restarted.clone(), access.clone());
    assert_eq!(
        json_request(
            router.clone(),
            Method::GET,
            "/api/light-breaker",
            phone["token"].as_str(),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
    assert_eq!(
        restarted.lock().unwrap().server_instance_id,
        phone["server_instance_id"]
    );
    assert_eq!(
        storage.load_remote_access_config().unwrap().unwrap(),
        tunnel
    );
    assert_eq!(
        json_request(
            router,
            Method::POST,
            "/api/addon/enrollment/exchange",
            None,
            pending
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        json_request(
            create_admin_router(restarted, access),
            Method::GET,
            "/api/light-breaker",
            Some(ADMIN),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn revocation_is_durable_and_closes_an_existing_sse_stream() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let stream = f
        .mobile()
        .oneshot(request(
            Method::GET,
            "/api/events",
            phone["token"].as_str(),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(stream.status(), StatusCode::OK);
    assert!(stream.headers()["content-type"]
        .to_str()
        .unwrap()
        .starts_with("text/event-stream"));
    let path = format!(
        "/api/addon/mobile-tokens/{}",
        phone["token_id"].as_str().unwrap()
    );
    let (status, body) =
        json_request(f.admin(), Method::DELETE, &path, Some(ADMIN), json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["revoked"], true);
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        to_bytes(stream.into_body(), 4096),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(
        json_request(
            f.mobile(),
            Method::GET,
            "/api/light-breaker",
            phone["token"].as_str(),
            json!({})
        )
        .await
        .0,
        StatusCode::UNAUTHORIZED
    );
    assert!(!load(&f.root)
        .lock()
        .unwrap()
        .api_auth
        .verify_token(phone["token"].as_str().unwrap()));
}

#[tokio::test]
async fn revocation_drops_events_that_wake_an_already_waiting_stream() {
    use std::future::{poll_fn, Future};
    use std::task::Poll;

    let f = Fixture::new();
    let phone = f.enroll().await;
    let response = f
        .mobile()
        .oneshot(request(
            Method::GET,
            "/api/events",
            phone["token"].as_str(),
            json!({}),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let tx = f.state.lock().unwrap().event_tx.clone().unwrap();
    let event = |address: &str| rhythm_os::server_event::ServerEvent::HubStatus {
        hub_type: Some("homeassistant".into()),
        address: Some(address.into()),
        connected: true,
    };
    tx.send(event("before-revocation")).unwrap();
    let body = to_bytes(response.into_body(), 4096);
    tokio::pin!(body);
    // Read the valid event and leave the stream suspended in recv(), after its
    // credential check. No timing or task-scheduling race is needed.
    poll_fn(|cx| {
        assert!(body.as_mut().poll(cx).is_pending());
        Poll::Ready(())
    })
    .await;
    let path = format!(
        "/api/addon/mobile-tokens/{}",
        phone["token_id"].as_str().unwrap()
    );
    assert_eq!(
        json_request(f.admin(), Method::DELETE, &path, Some(ADMIN), json!({}))
            .await
            .0,
        StatusCode::OK
    );
    tx.send(event("after-revocation")).unwrap();
    let bytes = tokio::time::timeout(std::time::Duration::from_secs(2), body)
        .await
        .unwrap()
        .unwrap();
    let received = std::str::from_utf8(&bytes).unwrap();
    assert!(received.contains("before-revocation"));
    assert!(!received.contains("after-revocation"), "{received}");
}

#[tokio::test]
async fn support_scope_and_unavailable_appliance_operations_remain_closed() {
    let f = Fixture::new();
    let support = auth::issue_local_support_token(&f.state, Some("fixture".into())).unwrap();
    for (method, path) in [
        (Method::POST, "/api/factory-reset"),
        (Method::PUT, "/api/remote-access/config"),
        (Method::DELETE, "/api/remote-access/config"),
        (Method::POST, "/api/addon/enrollment"),
        (Method::GET, "/api/addon/mobile-tokens"),
        (Method::DELETE, "/api/addon/mobile-tokens/any"),
        (Method::GET, "/api/backup"),
        (Method::PUT, "/api/backup"),
        (Method::PUT, "/api/hub/credentials"),
        (Method::POST, "/api/devices/pair"),
    ] {
        assert_eq!(
            json_request(
                f.mobile(),
                method.clone(),
                path,
                Some(&support.token),
                json!({})
            )
            .await
            .0,
            StatusCode::FORBIDDEN,
            "{method} {path}"
        );
    }
    assert_eq!(
        json_request(
            f.mobile(),
            Method::GET,
            "/api/light-breaker",
            Some(&support.token),
            json!({})
        )
        .await
        .0,
        StatusCode::OK
    );
}

#[tokio::test]
async fn actual_tcp_listener_classifies_tunnel_and_still_requires_bearer_on_loopback() {
    let f = Fixture::new();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = f.mobile();
    let server = tokio::spawn(async move {
        axum::serve(
            listener,
            router.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    let client = reqwest::Client::new();
    let status: Value = client
        .get(format!("http://{address}/api/auth/status"))
        .header("cf-connecting-ip", "203.0.113.1")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(status["via_remote_access"], true);
    assert_eq!(status["requires_auth"], true);
    assert_eq!(status["claim_available"], false);
    let denied = client
        .get(format!("http://{address}/api/light-breaker"))
        .send()
        .await
        .unwrap();
    assert_eq!(denied.status(), StatusCode::UNAUTHORIZED);
    server.abort();
}

// In-process router calls always present an already-buffered Body. Exercise
// headers arriving before the body, as Dart's real HTTP client sends them.
#[tokio::test]
async fn early_responses_consume_split_request_bodies_and_keep_connections_usable() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn response(stream: &mut tokio::net::TcpStream) -> (u16, Value) {
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
            assert!(headers.len() < 8192);
        }
        let headers = String::from_utf8(headers).unwrap();
        let status = headers.split_whitespace().nth(1).unwrap().parse().unwrap();
        let length: usize = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("content-length")
                    .then(|| value.trim().parse().unwrap())
            })
            .unwrap();
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        (status, serde_json::from_slice(&body).unwrap())
    }

    let f = Fixture::new();
    for (router, method, path, token, expected) in [
        (f.admin(), "POST", "/api/addon/enrollment", Some(ADMIN), 200),
        (f.admin(), "PUT", "/api/hub/credentials", Some(ADMIN), 403),
        (f.admin(), "POST", "/api/addon/enrollment", None, 401),
        (f.mobile(), "PUT", "/api/hub/credentials", None, 403),
    ] {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        for chunked in [false, true] {
            let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
            let framing = if chunked {
                "Transfer-Encoding: chunked"
            } else {
                "Content-Length: 2"
            };
            let authorization = token.map_or(String::new(), |token| {
                format!("Authorization: Bearer {token}\r\n")
            });
            stream.write_all(format!(
                "{method} {path} HTTP/1.1\r\nHost: localhost\r\n{authorization}Content-Type: application/json\r\n{framing}\r\n\r\n"
            ).as_bytes()).await.unwrap();
            assert!(
                tokio::time::timeout(std::time::Duration::from_millis(30), stream.read_u8())
                    .await
                    .is_err(),
                "{method} {path} responded before consuming the body (chunked={chunked})"
            );
            stream
                .write_all(if chunked {
                    b"2\r\n{}\r\n0\r\n\r\n"
                } else {
                    b"{}"
                })
                .await
                .unwrap();
            let (status, body) =
                tokio::time::timeout(std::time::Duration::from_secs(2), response(&mut stream))
                    .await
                    .unwrap();
            assert_eq!(status, expected);
            if expected == 200 {
                assert!(body["code"].is_string());
            }
            stream.write_all(format!(
                "GET /api/addon/status HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {ADMIN}\r\n\r\n"
            ).as_bytes()).await.unwrap();
            let (status, _) =
                tokio::time::timeout(std::time::Duration::from_secs(2), response(&mut stream))
                    .await
                    .unwrap();
            assert_eq!(status, 200, "the next request must reuse the connection");
        }
        server.abort();
    }
}

#[tokio::test]
async fn rejected_request_bodies_cannot_rotate_enrollment() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let f = Fixture::new();
    let (status, code) = json_request(
        f.admin(),
        Method::POST,
        "/api/addon/enrollment",
        Some(ADMIN),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let oversized = Request::builder()
        .method(Method::POST)
        .uri("/api/addon/enrollment")
        .header("authorization", format!("Bearer {ADMIN}"))
        .body(Body::from(vec![b' '; 2 * 1024 * 1024 + 1]))
        .unwrap();
    let response = f.admin().oneshot(oversized).await.unwrap();
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(response.headers()["connection"], "close");

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let router = f.admin();
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    for (body, expected) in [("invalid-chunk\r\n", 400), ("", 408)] {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(format!(
            "POST /api/addon/enrollment HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {ADMIN}\r\nTransfer-Encoding: chunked\r\n\r\n{body}"
        ).as_bytes()).await.unwrap();
        let mut response = Vec::new();
        tokio::time::timeout(
            std::time::Duration::from_secs(17),
            stream.read_to_end(&mut response),
        )
        .await
        .unwrap()
        .unwrap();
        let response = String::from_utf8(response).unwrap();
        assert!(
            response.starts_with(&format!("HTTP/1.1 {expected}")),
            "{response}"
        );
        assert!(response.to_ascii_lowercase().contains("connection: close"));
        assert!(response
            .to_ascii_lowercase()
            .contains("cache-control: no-store"));
    }
    server.abort();
    // All three rejected requests must leave the previously issued code intact.
    let (status, _) = json_request(
        f.mobile(),
        Method::POST,
        "/api/addon/enrollment/exchange",
        None,
        code,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn capabilities_advertise_mobile_and_ha_without_full_backup_or_native_setup() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let (_, addon) = json_request(
        f.mobile(),
        Method::GET,
        "/api/addon/status",
        None,
        json!({}),
    )
    .await;
    let (_, state) = json_request(
        f.mobile(),
        Method::GET,
        "/api/state",
        phone["token"].as_str(),
        json!({}),
    )
    .await;
    assert_eq!(state["capabilities"]["deployment"], addon["capabilities"]);
    assert_eq!(addon["capabilities"]["direct_mobile_control"], true);
    assert_eq!(addon["capabilities"]["portable_settings"], true);
    assert_eq!(addon["capabilities"]["full_backup_export"], false);
    assert_eq!(addon["capabilities"]["full_backup_import"], false);
    assert!(!state["capabilities"]["features"]
        .as_array()
        .unwrap()
        .iter()
        .any(|f| f.as_str().unwrap().contains("matter")));
}

#[tokio::test]
async fn profile_import_and_reset_pause_addon_control() {
    let f = Fixture::new();
    f.state.lock().unwrap().light_breaker_enabled = true;
    let (status, _) = json_request(
        f.admin(),
        Method::POST,
        "/api/profile-bundle/reset",
        Some(ADMIN),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!f.state.lock().unwrap().light_breaker_enabled);
    assert!(!load(&f.root).lock().unwrap().light_breaker_enabled);
}

#[tokio::test]
async fn lighting_settings_require_auth_and_restore_without_cloning_installation() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    for (method, path) in [
        (Method::GET, "/api/lighting-settings"),
        (Method::POST, "/api/lighting-settings/preview"),
        (Method::PUT, "/api/lighting-settings"),
    ] {
        let (status, _) = json_request(f.mobile(), method, path, None, json!({})).await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, mut source) = json_request(
        f.mobile(),
        Method::GET,
        "/api/lighting-settings",
        phone["token"].as_str(),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(source["kind"], "lighting_settings");
    assert!(source.get("installation").is_none());
    source["profile"]["power_save"] = json!(true);
    f.state.lock().unwrap().light_breaker_enabled = true;
    let identity = f.state.lock().unwrap().server_instance_id.clone();
    let (status, preview) = json_request(
        f.mobile(),
        Method::POST,
        "/api/lighting-settings/preview",
        phone["token"].as_str(),
        source,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        f.state.lock().unwrap().light_breaker_enabled,
        "preview must not pause control"
    );
    let (status, result) = json_request(
        f.mobile(),
        Method::PUT,
        "/api/lighting-settings",
        phone["token"].as_str(),
        json!({"settings": preview, "node_mappings": {}}),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{result}");
    assert_eq!(result["applied_nodes"], 0);
    let state = f.state.lock().unwrap();
    assert!(state.power_save);
    assert!(!state.light_breaker_enabled);
    assert_eq!(state.server_instance_id, identity);
    assert!(state
        .api_auth
        .verify_token(phone["token"].as_str().unwrap()));
    drop(state);
    let restarted = load(&f.root);
    let state = restarted.lock().unwrap();
    assert!(state.power_save);
    assert!(!state.light_breaker_enabled);
    assert_eq!(state.server_instance_id, identity);
}

#[tokio::test]
async fn lighting_preview_accepts_large_backups_but_does_not_export_integration_data() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let mut backup: Value = serde_json::to_value(
        rhythm_os::commands::build_backup_bundle_dto(&f.state, false).unwrap(),
    )
    .unwrap();
    backup["installation"]["integration_files"] = json!([{
        "path": "matter/ignored.json",
        "content": "private-fixture".repeat(170_000),
        "secret": true,
    }]);
    assert!(backup.to_string().len() > 2 * 1024 * 1024);
    let (status, settings) = json_request(
        f.mobile(),
        Method::POST,
        "/api/lighting-settings/preview",
        phone["token"].as_str(),
        backup,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    assert_eq!(settings["kind"], "lighting_settings");
    assert!(!settings.to_string().contains("private-fixture"));
    assert!(settings.get("installation").is_none());
}

#[tokio::test]
async fn factory_reset_clears_phone_and_selection_files_then_refuses_further_enrollment() {
    let f = Fixture::new();
    let phone = f.enroll().await;
    let controller =
        Arc::new(rhythm_os::remote_access::ChildProcessRemoteAccessController::new("/missing"));
    crate::install_factory_reset_hook(&f.state, f.access.clone(), controller).unwrap();
    // Exercise the actual HTTP reset, without terminating the test process.
    f.state.lock().unwrap().after_factory_reset_fn = None;
    for name in [
        "managed-ha-lights.json",
        "managed-ha-lights-v2.json",
        "managed-ha-lights-v1.rollback.json",
    ] {
        std::fs::write(f.root.join(name), "{}").unwrap();
    }
    let (status, _) = json_request(
        f.admin(),
        Method::POST,
        "/api/factory-reset",
        Some(ADMIN),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!f
        .state
        .lock()
        .unwrap()
        .api_auth
        .verify_token(phone["token"].as_str().unwrap()));
    assert!(!f.state.lock().unwrap().light_breaker_enabled);
    for name in [
        "auth.json",
        "managed-ha-lights.json",
        "managed-ha-lights-v2.json",
        "managed-ha-lights-v1.rollback.json",
    ] {
        assert!(!f.root.join(name).exists(), "{name} survived reset");
    }
    assert_eq!(
        json_request(
            f.admin(),
            Method::POST,
            "/api/addon/enrollment",
            Some(ADMIN),
            json!({})
        )
        .await
        .0,
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn reset_drains_admitted_writes_and_blocks_queued_writes_across_listeners() {
    let f = Fixture::new();
    let controller =
        Arc::new(rhythm_os::remote_access::ChildProcessRemoteAccessController::new("/missing"));
    crate::install_factory_reset_hook(&f.state, f.access.clone(), controller).unwrap();
    f.state.lock().unwrap().after_factory_reset_fn = None;

    // Model a mutation already admitted on the mobile listener. Reset on the
    // other listener must wait until its final durable write has completed.
    let admitted_write = f.access.mutation_gate.lock().await;
    let admin = f.admin();
    let reset = tokio::spawn(async move {
        json_request(
            admin,
            Method::POST,
            "/api/factory-reset",
            Some(ADMIN),
            json!({}),
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(!reset.is_finished());
    assert!(!f.access.is_resetting());
    let phone = auth::issue_local_owner_token(&f.state, None).unwrap();
    assert!(f.root.join("auth.json").exists());

    // This later mutation waits behind reset with a credential that was valid
    // when submitted. It must not recreate the credential file afterward.
    let mobile = f.mobile();
    let late_write = tokio::spawn(async move {
        json_request(
            mobile,
            Method::POST,
            "/api/auth/support-token",
            Some(&phone.token),
            json!({}),
        )
        .await
    });
    tokio::task::yield_now().await;
    drop(admitted_write);
    assert_eq!(reset.await.unwrap().0, StatusCode::OK);
    assert_eq!(late_write.await.unwrap().0, StatusCode::SERVICE_UNAVAILABLE);
    assert!(!f.root.join("auth.json").exists());
    assert!(f.state.lock().unwrap().api_auth.tokens.is_empty());
}

#[tokio::test]
async fn canceled_http_request_keeps_the_write_barrier_until_its_handler_finishes() {
    let access = Arc::new(MobileAccess::new(ADMIN).unwrap());
    let entered = Arc::new(tokio::sync::Notify::new());
    let finish = Arc::new(tokio::sync::Notify::new());
    let completed = Arc::new(AtomicU64::new(0));
    let router = Router::new()
        .route(
            "/write",
            post({
                let entered = entered.clone();
                let finish = finish.clone();
                let completed = completed.clone();
                move || async move {
                    entered.notify_one();
                    finish.notified().await;
                    completed.fetch_add(1, Ordering::SeqCst);
                    StatusCode::OK
                }
            }),
        )
        .layer(middleware::from_fn(serialize_mutations))
        .layer(Extension(access.clone()));
    let request = tokio::spawn(router.oneshot(request(Method::POST, "/write", None, json!({}))));
    entered.notified().await;
    request.abort();
    let _ = request.await;
    assert!(access.mutation_gate.try_lock().is_err());
    finish.notify_one();
    let _quiesced = access.mutation_gate.lock().await;
    assert_eq!(completed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn health_and_admin_status_expose_the_same_build_identity_without_secrets() {
    let f = Fixture::new();
    let (status, health) = json_request(f.mobile(), Method::GET, "/health", None, json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["status"], "healthy");
    assert_eq!(
        health["build"]["product_version"],
        env!("CARGO_PKG_VERSION")
    );
    for (field, value) in [
        ("image_version", option_env!("RHYTHM_IMAGE_VERSION")),
        ("product_revision", option_env!("RHYTHM_PRODUCT_REVISION")),
        (
            "packaging_revision",
            option_env!("RHYTHM_PACKAGING_REVISION"),
        ),
        (
            "build_inputs_sha256",
            option_env!("RHYTHM_BUILD_INPUTS_SHA256"),
        ),
    ] {
        assert_eq!(
            health["build"][field].as_str(),
            value.filter(|v| !v.is_empty()),
            "{field}"
        );
    }
    let (status, admin) = json_request(
        f.admin(),
        Method::GET,
        "/api/addon/status",
        Some(ADMIN),
        json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(health["build"], admin["build"]);
    assert!(health.get("server_instance_id").is_none());
    assert!(!health.to_string().contains(ADMIN));
    let (status, _) = json_request(f.admin(), Method::GET, "/health", None, json!({})).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}
