use dendrite_plane::{PROTOCOL, Server};
use plane_client::Config;
use serde_json::{Value, json};

fn call(server: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    serde_json::from_str(&server.handle_line(&line).expect("a response")).unwrap()
}

fn offline() -> Server {
    // Nothing listens on this port: only protocol-level behavior is exercised.
    Server::with_config(Config::new("http://127.0.0.1:9", "ws", "proj", "k").unwrap())
}

#[test]
fn health_reports_name_protocol_and_whether_a_token_is_configured() {
    let r = call(&mut offline(), "health", json!({}));
    assert_eq!(r["result"]["name"], "dendrite-plane");
    assert_eq!(r["result"]["protocol"], PROTOCOL);
    assert_eq!(r["result"]["configured"], true);
}

#[test]
fn protocol_errors_use_the_standard_codes() {
    let mut s = offline();
    assert_eq!(
        serde_json::from_str::<Value>(&s.handle_line("{nope").unwrap()).unwrap()["error"]["code"],
        -32700
    );
    assert_eq!(
        serde_json::from_str::<Value>(&s.handle_line(r#"{"id":1}"#).unwrap()).unwrap()["error"]["code"],
        -32600
    );
    assert_eq!(
        call(&mut s, "no.such.method", json!({}))["error"]["code"],
        -32601
    );
    assert_eq!(
        call(&mut s, "issue.resolve", json!({}))["error"]["code"],
        -32602
    );
    assert_eq!(
        call(
            &mut s,
            "issue.set_status",
            json!({"ref": "DEN-1", "status": "weird"})
        )["error"]["code"],
        -32602
    );
    assert_eq!(
        call(&mut s, "issues.list", json!({"fields": "huge"}))["error"]["code"],
        -32602
    );
}

#[test]
fn notifications_get_no_reply() {
    assert!(
        offline()
            .handle_line(r#"{"jsonrpc":"2.0","method":"health"}"#)
            .is_none()
    );
}

#[test]
fn transport_failures_are_engine_errors_with_a_kind() {
    let r = call(&mut offline(), "states.list", json!({}));
    assert_eq!(r["error"]["code"], -32000);
    assert_eq!(r["error"]["data"]["kind"], "transport");
}

#[test]
fn a_project_is_required_somewhere() {
    let mut s = Server::with_config(Config::new("http://127.0.0.1:9", "ws", "", "k").unwrap());
    let r = call(&mut s, "states.list", json!({}));
    assert_eq!(r["error"]["code"], -32602);
    assert!(
        r["error"]["message"]
            .as_str()
            .unwrap()
            .contains("project_id")
    );
}
