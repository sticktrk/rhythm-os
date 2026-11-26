//! Wake resident clients so they can read the hub's authenticated owner queues.
use crate::activity_cloud::{load_config, StoredActivityCloudConfig};
use crate::server_event::ServerEvent;
use crate::state::{AppState, SharedState};
use reqwest::Url;
use serde_json::{json, Value};
use std::sync::Mutex;
use std::time::{Duration, Instant};

const RING_INTERVAL: Duration = Duration::from_secs(30);
static LAST_RINGS: Mutex<[Option<Instant>; 2]> = Mutex::new([None; 2]);

fn admit_ring(last: &mut [Option<Instant>; 2], kind: &str, now: Instant) -> bool {
    let slot = match kind {
        "connection" => &mut last[0],
        "plan" => &mut last[1],
        _ => return false,
    };
    if slot.is_some_and(|previous| now.duration_since(previous) < RING_INTERVAL) {
        return false;
    }
    *slot = Some(now);
    true
}

fn attention_url(ingest_url: &str) -> Option<Url> {
    let url = Url::parse(ingest_url).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.path().ends_with("/server-activity-ingest")
    {
        return None;
    }
    url.join("hub-attention").ok()
}

fn ring_body(config: &StoredActivityCloudConfig, kind: &str, id: &str, expires: u64) -> Value {
    json!({
        "home_id": config.home_id, "hub_id": config.hub_id,
        "server_instance_id": config.server_instance_id,
        "kind": kind, "request_id": id, "expires_at_epoch_ms": expires,
    })
}

fn emit(s: &AppState, kind: &'static str, id: &str, expires: u64, state: &'static str) {
    s.emit_event(ServerEvent::AssistantAttention {
        kind,
        request_id: id.into(),
        state,
        expires_at_epoch_ms: expires,
    });
}

pub(crate) fn resolved(s: &AppState, kind: &'static str, id: &str, expires: u64) {
    emit(s, kind, id, expires, "resolved");
}

pub(crate) fn requested(
    state: &SharedState,
    s: &AppState,
    kind: &'static str,
    id: &str,
    expires: u64,
) {
    emit(s, kind, id, expires, "requested");
    let Some(runtime) = tokio::runtime::Handle::try_current()
        .ok()
        .or_else(|| s.tokio_handle.clone())
    else {
        return;
    };
    let state = state.clone();
    let id = id.to_owned();
    runtime.spawn(async move {
        // Reading configuration needs the state mutex. The initiating handler still holds it.
        let Ok(Some(config)) = load_config(&state) else {
            return;
        };
        if !config.is_usable() {
            return;
        }
        let Some(url) = attention_url(&config.ingest_url) else {
            return;
        };
        if !admit_ring(
            &mut LAST_RINGS.lock().unwrap_or_else(|p| p.into_inner()),
            kind,
            Instant::now(),
        ) {
            return;
        }
        let Ok(client) = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .build()
        else {
            log::warn!("Could not initialize assistant notification client");
            return;
        };
        let response = client
            .post(url)
            .bearer_auth(&config.upload_token)
            .json(&ring_body(&config, kind, &id, expires))
            .send()
            .await;
        match response {
            Ok(response) if !response.status().is_success() => {
                log::warn!("Assistant notification returned HTTP {}", response.status());
            }
            Err(_) => log::warn!("Assistant notification could not reach the cloud"),
            _ => {}
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sibling_url_and_ring_payload() {
        assert_eq!(
            attention_url("https://example.org/functions/v1/server-activity-ingest")
                .unwrap()
                .as_str(),
            "https://example.org/functions/v1/hub-attention"
        );
        for url in [
            "https://example.org/upload",
            "https://example.org/?a=/server-activity-ingest",
            "server-activity-ingest",
        ] {
            assert!(attention_url(url).is_none());
        }
        let config: StoredActivityCloudConfig = serde_json::from_value(json!({
            "hub_id":"hub","home_id":"home","server_instance_id":"instance",
            "ingest_url":"http://localhost/server-activity-ingest","upload_token":"private","updated_at_epoch_ms":0
        })).unwrap();
        assert_eq!(
            ring_body(&config, "connection", "ticket", 42),
            json!({
                "home_id":"home","hub_id":"hub","server_instance_id":"instance","kind":"connection","request_id":"ticket","expires_at_epoch_ms":42
            })
        );
    }

    #[test]
    fn rings_have_separate_thirty_second_limits() {
        let start = Instant::now();
        let mut last = [None; 2];
        assert!(admit_ring(&mut last, "plan", start));
        assert!(admit_ring(&mut last, "connection", start));
        assert!(!admit_ring(
            &mut last,
            "plan",
            start + Duration::from_secs(29)
        ));
        assert!(admit_ring(&mut last, "plan", start + RING_INTERVAL));
    }
}
