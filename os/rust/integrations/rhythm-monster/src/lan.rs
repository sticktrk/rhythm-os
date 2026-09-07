use crate::{crypto::LightLanCrypto, LightCredentials, LightError, LightProperty, LightResult};
use async_trait::async_trait;
use axum::{
    extract::{ConnectInfo, DefaultBodyLimit, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rand::{distributions::Alphanumeric, Rng};
use serde_json::{json, Value};
use std::{
    collections::{HashMap, VecDeque},
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::oneshot;

#[async_trait]
pub trait LightTransport: Send + Sync {
    async fn read(&self, property: LightProperty) -> LightResult<Value>;
    /// Success means a signed device readback matched every write.
    async fn write(&self, values: Vec<(LightProperty, Value)>) -> LightResult<()>;
}
pub struct LightLanClient {
    credentials: LightCredentials,
    lane: tokio::sync::Mutex<()>,
    http: reqwest::Client,
    port: std::sync::atomic::AtomicU16,
}
impl LightLanClient {
    pub fn new(credentials: LightCredentials) -> LightResult<Self> {
        credentials.validate()?;
        Ok(Self {
            credentials,
            port: std::sync::atomic::AtomicU16::new(0),
            lane: tokio::sync::Mutex::new(()),
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(Duration::from_secs(5))
                .build()
                .map_err(|_| LightError::Unavailable)?,
        })
    }
    async fn exchange(
        &self,
        values: &[(LightProperty, Value)],
        property: LightProperty,
    ) -> LightResult<Value> {
        // Use the routed LAN interface; never advertise 0.0.0.0 as callback IP.
        let route = tokio::net::UdpSocket::bind("0.0.0.0:0")
            .await
            .map_err(|_| LightError::Unavailable)?;
        route
            .connect((self.credentials.ip, 80))
            .await
            .map_err(|_| LightError::Unavailable)?;
        let local = route
            .local_addr()
            .map_err(|_| LightError::Unavailable)?
            .ip();
        let socket = tokio::net::TcpSocket::new_v4().map_err(|_| LightError::Unavailable)?;
        socket
            .set_reuseaddr(true)
            .map_err(|_| LightError::Unavailable)?;
        // Reuse the last callback port so the device keeps one session slot,
        // but never fail an exchange because a timed-out server task is still
        // releasing it: fall back to an ephemeral port instead.
        let remembered = self.port.load(std::sync::atomic::Ordering::Relaxed);
        if remembered == 0 || socket.bind(SocketAddr::new(local, remembered)).is_err() {
            socket
                .bind(SocketAddr::new(local, 0))
                .map_err(|_| LightError::Unavailable)?;
        }
        let listener = socket.listen(16).map_err(|_| LightError::Unavailable)?;
        let address = listener.local_addr().map_err(|_| LightError::Unavailable)?;
        self.port
            .store(address.port(), std::sync::atomic::Ordering::Relaxed);
        let (send, recv) = oneshot::channel();
        let (closed, close_recv) = oneshot::channel();
        let id = rand::thread_rng().gen_range(1..i32::MAX as u32);
        let mut queue = VecDeque::new();
        if !values.is_empty() {
            queue.push_back(json!({"properties":values.iter().map(|(p,v)|json!({"property":{
                "name":p.name(),"base_type":p.base_type(),"value":v,"dsn":self.credentials.dsn}})).collect::<Vec<_>>()}));
        }
        queue.push_back(json!({"cmds":[{"cmd":{"cmd_id":id,"method":"GET",
            "resource":format!("property.json?name={}",property.name()),"data":"","uri":"/local_lan/property/datapoint.json"}}]}));
        let state = Arc::new(Mutex::new(Callback {
            credentials: self.credentials.clone(),
            crypto: None,
            queue,
            id,
            property,
            read_sent: false,
            closed: Some(closed),
            send: Some(send),
        }));
        let router = Router::new()
            .route("/local_lan/key_exchange.json", post(key_exchange))
            .route("/local_lan/commands.json", get(commands))
            .route(
                "/local_lan/property/datapoint.json",
                post(observation).put(observation),
            )
            .layer(DefaultBodyLimit::max(65536))
            .with_state(state);
        let (shutdown, shutdown_rx) = oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = axum::serve(
                listener,
                router.into_make_service_with_connect_info::<SocketAddr>(),
            )
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await;
        });
        let mut guard = Abort(task, Some(shutdown));
        let response=self.http.post(format!("http://{}/local_reg.json",self.credentials.ip))
            .query(&[("dsn",self.credentials.dsn.as_str())])
            .json(&json!({"local_reg":{"ip":address.ip().to_string(),"port":address.port(),"uri":"/local_lan","notify":1}}))
            .send().await.map_err(|_|LightError::Unavailable)?;
        if response.status() != reqwest::StatusCode::ACCEPTED {
            return Err(if response.status() == reqwest::StatusCode::UNAUTHORIZED {
                LightError::Authentication
            } else {
                LightError::Unavailable
            });
        }
        let result = recv.await.map_err(|_| LightError::Unavailable)?;
        self.http.put(format!("http://{}/local_reg.json", self.credentials.ip))
            .json(&json!({"local_reg":{"ip":address.ip().to_string(),"port":address.port(),"uri":"/local_lan","notify":1}}))
            .send().await.map_err(|_| LightError::Unavailable)?;
        close_recv.await.map_err(|_| LightError::Unavailable)?;
        // Allow the final encrypted session-deletion response to flush.
        tokio::time::sleep(Duration::from_millis(100)).await;
        if let Some(shutdown) = guard.1.take() {
            let _ = shutdown.send(());
        }
        if tokio::time::timeout(Duration::from_secs(1), &mut guard.0)
            .await
            .is_err()
        {
            guard.0.abort();
            let _ = (&mut guard.0).await;
        }
        result
    }
}
struct Abort(tokio::task::JoinHandle<()>, Option<oneshot::Sender<()>>);
impl Drop for Abort {
    fn drop(&mut self) {
        if let Some(shutdown) = self.1.take() {
            let _ = shutdown.send(());
        }
        self.0.abort();
    }
}
#[async_trait]
impl LightTransport for LightLanClient {
    async fn read(&self, p: LightProperty) -> LightResult<Value> {
        tokio::time::timeout(Duration::from_secs(15), async {
            let _lane = self.lane.lock().await;
            self.exchange(&[], p).await
        })
        .await
        .map_err(|_| LightError::Timeout)?
    }
    async fn write(&self, values: Vec<(LightProperty, Value)>) -> LightResult<()> {
        if values.is_empty() || values.len() > 6 {
            return Err(LightError::InvalidInput);
        }
        for (p, v) in &values {
            p.validate(v)?;
        }
        tokio::time::timeout(Duration::from_secs(30), async {
            let _lane = self.lane.lock().await;
            // This firmware accepts one property per command. Batches stall its queue.
            for (p, v) in &values {
                let observed = self.exchange(&[(*p, v.clone())], *p).await?;
                if &observed != v {
                    return Err(LightError::Readback);
                }
            }
            Ok(())
        })
        .await
        .map_err(|_| LightError::Timeout)?
    }
}
struct Callback {
    credentials: LightCredentials,
    crypto: Option<LightLanCrypto>,
    queue: VecDeque<Value>,
    id: u32,
    property: LightProperty,
    read_sent: bool,
    closed: Option<oneshot::Sender<()>>,
    send: Option<oneshot::Sender<LightResult<Value>>>,
}
type Shared = Arc<Mutex<Callback>>;
type Response = Result<Json<Value>, StatusCode>;
fn allowed(s: &Callback, peer: SocketAddr) -> Result<(), StatusCode> {
    if peer.ip() != IpAddr::V4(s.credentials.ip) {
        Err(StatusCode::FORBIDDEN)
    } else {
        Ok(())
    }
}
async fn key_exchange(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Json(body): Json<Value>,
) -> Response {
    let mut s = s.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    allowed(&s, peer)?;
    if s.crypto.is_some() {
        return Err(StatusCode::CONFLICT);
    }
    let k = &body["key_exchange"];
    if k["key_id"].as_u64() != Some(s.credentials.local_key_id as u64)
        || k["ver"] != 1
        || k["proto"] != 1
    {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let r1 = k["random_1"].as_str().ok_or(StatusCode::BAD_REQUEST)?;
    let t1 = k["time_1"].as_u64().ok_or(StatusCode::BAD_REQUEST)?;
    let r2: String = rand::thread_rng()
        .sample_iter(Alphanumeric)
        .take(16)
        .map(char::from)
        .collect();
    let t2 = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .as_micros() as u64;
    s.crypto = Some(
        LightLanCrypto::new(s.credentials.local_key.expose(), r1, &r2, t1, t2)
            .map_err(|_| StatusCode::BAD_REQUEST)?,
    );
    Ok(Json(json!({"random_2":r2,"time_2":t2})))
}
async fn commands(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let mut s = s.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    allowed(&s, peer)?;
    if s.crypto.is_none() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let data = s.queue.pop_front().unwrap_or(json!({}));
    if data["cmds"][0]["cmd"]["method"] == "DELETE" {
        if let Some(closed) = s.closed.take() {
            let _ = closed.send(());
        }
    } else if data.get("cmds").is_some() {
        s.read_sent = true;
    }
    let status = if s.queue.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::PARTIAL_CONTENT
    };
    Ok((
        status,
        Json(
            s.crypto
                .as_mut()
                .unwrap()
                .encrypt(data)
                .map_err(|_| StatusCode::BAD_REQUEST)?,
        ),
    ))
}
fn property_value(data: &Value, name: &str) -> Option<Value> {
    if data["name"] == name {
        return data.get("value").cloned();
    }
    if let Some(p) = data.get("property") {
        return property_value(p, name);
    }
    None
}
async fn observation(
    State(s): State<Shared>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    Query(query): Query<HashMap<String, String>>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let mut s = s.lock().map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    allowed(&s, peer)?;
    let plain = s
        .crypto
        .as_mut()
        .ok_or(StatusCode::UNAUTHORIZED)?
        .decrypt(&body)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    if s.read_sent && query.get("cmd_id").and_then(|v| v.parse::<u32>().ok()) == Some(s.id) {
        if let Some(value) = property_value(&plain, s.property.name()) {
            if let Some(send) = s.send.take() {
                let _ = send.send(Ok(value));
            }
            s.queue.push_back(json!({"cmds":[{"cmd":{"cmd_id":0,"method":"DELETE","resource":"local_reg.json","data":"delete_session","uri":"/local_lan"}}]}));
        }
    }
    Ok((
        if s.queue.is_empty() {
            StatusCode::OK
        } else {
            StatusCode::PARTIAL_CONTENT
        },
        Json(json!({})),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::LightSecret;

    #[tokio::test]
    async fn device_continues_after_write_and_only_matching_signed_read_completes() {
        let (send, mut recv) = oneshot::channel();
        let state = Arc::new(Mutex::new(Callback {
            credentials: LightCredentials {
                dsn: "ACFIXTURE123456".into(),
                ip: "192.168.1.2".parse().unwrap(),
                local_key: LightSecret::new("synthetic-fixture-key".into()),
                local_key_id: 1,
            },
            crypto: Some(
                LightLanCrypto::new(
                    "synthetic-fixture-key",
                    "AAAAAAAAAAAAAAAA",
                    "BBBBBBBBBBBBBBBB",
                    1,
                    2,
                )
                .unwrap(),
            ),
            queue: VecDeque::from([json!({"properties":[]}), json!({"cmds":[]})]),
            id: 42,
            property: LightProperty::Power,
            read_sent: false,
            closed: None,
            send: Some(send),
        }));
        let peer: SocketAddr = "192.168.1.2:1234".parse().unwrap();
        assert_eq!(
            commands(
                State(state.clone()),
                ConnectInfo("192.168.1.3:1234".parse().unwrap())
            )
            .await
            .unwrap_err(),
            StatusCode::FORBIDDEN
        );
        assert_eq!(
            commands(State(state.clone()), ConnectInfo(peer))
                .await
                .unwrap()
                .0,
            StatusCode::PARTIAL_CONTENT
        );
        assert_eq!(
            commands(State(state.clone()), ConnectInfo(peer))
                .await
                .unwrap()
                .0,
            StatusCode::OK
        );
        let mut device = LightLanCrypto::new(
            "synthetic-fixture-key",
            "BBBBBBBBBBBBBBBB",
            "AAAAAAAAAAAAAAAA",
            2,
            1,
        )
        .unwrap();
        let wrong = device.encrypt(json!({"name":"power","value":1})).unwrap();
        let _ = observation(
            State(state.clone()),
            ConnectInfo(peer),
            Query(HashMap::from([("cmd_id".into(), "41".into())])),
            Json(wrong),
        )
        .await
        .unwrap();
        assert!(matches!(
            recv.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ));
        let right = device
            .encrypt(json!({"property":{"name":"power","value":1}}))
            .unwrap();
        let _ = observation(
            State(state.clone()),
            ConnectInfo(peer),
            Query(HashMap::from([("cmd_id".into(), "42".into())])),
            Json(right),
        )
        .await
        .unwrap();
        assert_eq!(
            commands(State(state), ConnectInfo(peer)).await.unwrap().0,
            StatusCode::OK
        );
        assert_eq!(recv.await.unwrap().unwrap(), json!(1));
    }
}
