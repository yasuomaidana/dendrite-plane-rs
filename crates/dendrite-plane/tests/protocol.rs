#[path = "../../plane-client/tests/fake_plane.rs"]
mod fake_plane;

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

#[test]
fn the_protocol_is_1_2_0() {
    assert_eq!(PROTOCOL, "1.2.0");
}

#[test]
fn new_issue_methods_validate_params_and_report_invalid_without_calling_plane() {
    let mut s = offline();
    for method in ["issue.get", "issue.update", "issue.comment"] {
        assert_eq!(
            call(&mut s, method, json!({}))["error"]["code"],
            -32602,
            "{method}"
        );
    }
    assert_eq!(
        call(&mut s, "issue.create", json!({}))["error"]["code"],
        -32602
    );

    // Rejected by the client's own checks, so nothing listens and nothing is asked.
    let r = call(
        &mut s,
        "issue.create",
        json!({"name": "n", "description_html": "<p></p>"}),
    );
    assert_eq!(r["error"]["code"], -32000);
    assert_eq!(r["error"]["data"]["kind"], "invalid");
    let r = call(
        &mut s,
        "issue.comment",
        json!({"ref": "11111111-2222-3333-4444-555555555555", "comment": " "}),
    );
    assert_eq!(r["error"]["data"]["kind"], "invalid");
}

/// Every method the server answers has a schema in proto/, and no schema is orphaned.
#[test]
fn every_method_has_a_proto_schema() {
    let methods = [
        "health",
        "board.snapshot",
        "issues.list",
        "states.list",
        "issue.resolve",
        "issue.get",
        "issue.create",
        "issue.update",
        "issue.comment",
        "issue.set_status",
        "relations.batch",
        "cache.invalidate",
        "project.archive",
        "project.delete",
    ];
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../proto");
    for m in methods {
        let raw = std::fs::read_to_string(dir.join(format!("{m}.json")))
            .unwrap_or_else(|_| panic!("proto/{m}.json is missing"));
        let schema: Value = serde_json::from_str(&raw).unwrap_or_else(|e| panic!("{m}: {e}"));
        assert!(
            schema.get("params").is_some() && schema.get("result").is_some(),
            "{m}"
        );
    }
    let mut files: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .trim_end_matches(".json")
                .to_string()
        })
        .collect();
    files.sort();
    let mut want: Vec<String> = methods.iter().map(|m| m.to_string()).collect();
    want.sort();
    assert_eq!(files, want);
}

#[test]
fn project_archive_and_delete_return_ok_and_map_unknown_project() {
    let fake = fake_plane::Fake::start(|req| {
        let p = &req.path_and_query;
        let archive = req.method == "POST" && p.contains("/projects/proj/archive/");
        let delete = req.method == "DELETE" && p.contains("/projects/proj/");
        if archive || delete {
            fake_plane::Reply {
                status: 204,
                body: String::new(),
                delay: std::time::Duration::ZERO,
            }
        } else if p.contains("/projects/unknown/") {
            fake_plane::Reply {
                status: 404,
                body: "{}".into(),
                delay: std::time::Duration::ZERO,
            }
        } else {
            fake_plane::Reply {
                status: 401,
                body: "unauthorized".into(),
                delay: std::time::Duration::ZERO,
            }
        }
    });

    let mut server = Server::with_config(Config::new(&fake.url, "ws", "proj", "k").unwrap());

    // Both methods over the real stdio server return {"ok": true}
    let r = call(&mut server, "project.archive", json!({}));
    assert_eq!(r["result"]["ok"], true);

    let r = call(&mut server, "project.delete", json!({}));
    assert_eq!(r["result"]["ok"], true);

    // An unknown project (fake answers 404) returns an error with kind not_found
    let r = call(
        &mut server,
        "project.archive",
        json!({"project_id": "unknown"}),
    );
    assert_eq!(r["error"]["code"], -32000);
    assert_eq!(r["error"]["data"]["kind"], "not_found");

    let r = call(
        &mut server,
        "project.delete",
        json!({"project_id": "unknown"}),
    );
    assert_eq!(r["error"]["code"], -32000);
    assert_eq!(r["error"]["data"]["kind"], "not_found");

    // The health result reports the new protocol version
    let r = call(&mut server, "health", json!({}));
    assert_eq!(r["result"]["protocol"], "1.2.0");
    assert_eq!(r["result"]["protocol"], PROTOCOL);
}
