//! Service attribution uses the call's context, never concurrent REST changes.
use futures_util::{SinkExt, StreamExt};
use rhythm_ha::{
    reqwest_transport::ReqwestHaTransport,
    transport::{HaConnectionConfig, HaTransport},
};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio_tungstenite::tungstenite::Message;

fn service_server(
    connections: Vec<Vec<Value>>,
) -> (HaConnectionConfig, std::thread::JoinHandle<Vec<Value>>) {
    let (tx, rx) = std::sync::mpsc::channel();
    let server = std::thread::spawn(move || {
        tokio::runtime::Runtime::new().unwrap().block_on(async {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap().port()).unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(20), async {
                let mut calls = Vec::new();
                for responses in connections {
                    let (mut stream, _) = listener.accept().await.unwrap();
                    let mut prefix = [0; 4];
                    stream.peek(&mut prefix).await.unwrap();
                    if prefix == *b"POST" {
                        // HA's REST response includes unrelated concurrent changes.
                        // Supporting it here makes the old implementation fail for
                        // wrong attribution, rather than for a missing fake route.
                        let body = json!([
                            {"entity_id":"light.target", "context":{"id":"own-call"}},
                            {"entity_id":"light.other", "context":{"id":"other-user"}}
                        ]).to_string();
                        let response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body);
                        stream.write_all(response.as_bytes()).await.unwrap();
                        return calls;
                    }
                    let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
                    ws.send(Message::Text(json!({"type":"auth_required"}).to_string())).await.unwrap();
                    let auth: Value = serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                    assert_eq!(auth, json!({"type":"auth", "access_token":"test-token"}));
                    if responses.first().is_some_and(|reply| reply["type"] == "auth_invalid") {
                        ws.send(Message::Text(responses[0].to_string())).await.unwrap();
                        continue;
                    }
                    ws.send(Message::Text(json!({"type":"auth_ok"}).to_string())).await.unwrap();
                    for response in responses {
                        let call: Value = serde_json::from_str(&ws.next().await.unwrap().unwrap().into_text().unwrap()).unwrap();
                        assert_eq!(call["type"], "call_service");
                        if response["type"] == "test_timeout" {
                            calls.push(call);
                            tokio::time::sleep(std::time::Duration::from_secs(6)).await;
                            break;
                        }
                        // Unsolicited traffic is not a matching service result.
                        ws.send(Message::Text(json!({"type":"event", "event":{"context":{"id":"other-user"}}}).to_string())).await.unwrap();
                        let mut response = response;
                        response["id"] = call["id"].clone();
                        ws.send(Message::Text(response.to_string())).await.unwrap();
                        calls.push(call);
                    }
                }
                calls
            }).await.expect("bounded fake HA service session")
        })
    });
    let config = HaConnectionConfig {
        host: "127.0.0.1".into(),
        port: rx.recv().unwrap(),
        token: "test-token".into(),
        use_ssl: false,
    };
    (config, server)
}

fn accepted(context: &str) -> Value {
    json!({"type":"result", "success":true, "result":{"context":{"id":context}, "response":null}})
}

#[test]
fn own_context_excludes_concurrent_changes_and_reuses_authenticated_session() {
    let (config, server) = service_server(vec![vec![accepted("own-call"), accepted("next-call")]]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    let data = json!({"entity_id":["light.target"], "brightness_pct":50});
    assert_eq!(
        transport
            .call_service_contexts("light", "turn_on", &data)
            .unwrap(),
        vec!["own-call"]
    );
    assert_eq!(
        transport
            .call_service_contexts("light", "turn_on", &data)
            .unwrap(),
        vec!["next-call"]
    );
    let calls = server.join().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["domain"], "light");
    assert_eq!(calls[0]["service"], "turn_on");
    assert_eq!(calls[0]["service_data"], data);
    assert!(calls[1]["id"].as_u64() > calls[0]["id"].as_u64());
}

#[test]
fn rejected_service_is_not_retried_and_next_action_reconnects() {
    let rejected = json!({"type":"result", "success":false, "error":{"code":"failed", "message":"synthetic service failure"}});
    let (config, server) = service_server(vec![vec![rejected], vec![accepted("recovered")]]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    let data = json!({"entity_id":["light.target"]});
    assert!(transport
        .call_service_contexts("light", "turn_on", &data)
        .is_err());
    assert_eq!(
        transport
            .call_service_contexts("light", "turn_off", &data)
            .unwrap(),
        vec!["recovered"]
    );
    let calls = server.join().unwrap();
    assert_eq!(calls.len(), 2, "failed side effects must not be replayed");
    assert_eq!(calls[1]["service"], "turn_off");
}

#[test]
fn missing_service_context_is_not_an_attributed_success() {
    let (config, server) = service_server(vec![vec![
        json!({"type":"result", "success":true, "result":null}),
    ]]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    assert!(transport
        .call_service_contexts("light", "turn_on", &json!({"entity_id":["light.target"]}))
        .is_err());
    server.join().unwrap();
}

#[test]
fn closed_idle_socket_fails_once_without_replay_then_recovers() {
    let (config, server) =
        service_server(vec![vec![accepted("first")], vec![accepted("recovered")]]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    let data = json!({"entity_id":["light.target"]});
    assert!(transport
        .call_service_contexts("light", "turn_on", &data)
        .is_ok());
    assert!(transport
        .call_service_contexts("light", "turn_on", &data)
        .is_err());
    assert_eq!(
        transport
            .call_service_contexts("light", "turn_off", &data)
            .unwrap(),
        vec!["recovered"]
    );
    let calls = server.join().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["service"], "turn_off");
}

#[test]
fn authentication_failure_never_sends_a_service_and_next_action_can_reconnect() {
    let (config, server) = service_server(vec![
        vec![json!({"type":"auth_invalid"})],
        vec![accepted("recovered")],
    ]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    let data = json!({"entity_id":["light.target"]});
    assert!(transport
        .call_service_contexts("light", "turn_on", &data)
        .is_err());
    assert!(transport
        .call_service_contexts("light", "turn_off", &data)
        .is_ok());
    let calls = server.join().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["service"], "turn_off");
}

#[test]
fn timed_out_service_is_not_retried_and_next_action_reconnects() {
    let (config, server) = service_server(vec![
        vec![json!({"type":"test_timeout"})],
        vec![accepted("recovered")],
    ]);
    let transport = ReqwestHaTransport::new(config).unwrap();
    let data = json!({"entity_id":["light.target"]});
    let error = transport
        .call_service_contexts("light", "turn_on", &data)
        .unwrap_err();
    assert!(error.to_string().contains("deadline"));
    assert!(transport
        .call_service_contexts("light", "turn_off", &data)
        .is_ok());
    let calls = server.join().unwrap();
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["service"], "turn_off");
}
