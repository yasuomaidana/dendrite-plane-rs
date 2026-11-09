//! Every method of the table against an in-process Plane, so each handler's success path runs.

#[path = "../../plane-client/tests/fake_plane.rs"]
mod fake_plane;

use dendrite_plane::Server;
use fake_plane::{Fake, json};
use plane_client::Config;
use rstest::{fixture, rstest};
use serde_json::{Value, json};

const UUID: &str = "11111111-2222-3333-4444-555555555555";
const ISSUE: &str = r#"{"id":"11111111-2222-3333-4444-555555555555","sequence_id":1,"state":"s-todo","name":"[DEN-1] one"}"#;

fn call(server: &mut Server, method: &str, params: Value) -> Value {
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params}).to_string();
    serde_json::from_str(&server.handle_line(&line).expect("a response")).unwrap()
}

/// A Plane that knows one open issue, three states and an empty label set.
#[fixture]
fn plane() -> (Fake, Server) {
    let fake = Fake::start(|req| {
        let (p, post) = (&req.path_and_query, req.method == "POST");
        if p.contains("/states/") {
            json(
                r#"{"results":[{"id":"s-todo","name":"To Do","group":"unstarted"},{"id":"s-done","name":"Done","group":"completed"}]}"#,
            )
        } else if p.contains("/issue-relation/") || p.contains("/comments/") {
            json("{}")
        } else if p.contains("/labels/") {
            json(if post {
                r#"{"id":"lbl","name":"agent:x"}"#
            } else {
                r#"{"results":[]}"#
            })
        } else if post || p.contains(&format!("/issues/{UUID}/")) {
            json(ISSUE)
        } else {
            json(&format!(
                r#"{{"results":[{ISSUE}],"next_page_results":false,"next_cursor":""}}"#
            ))
        }
    });
    let server = Server::with_config(Config::new(&fake.url, "ws", "proj", "k").unwrap());
    (fake, server)
}

#[rstest]
#[case("board.snapshot", json!({}), "statuses")]
#[case("board.snapshot", json!({"fresh": true}), "statuses")]
#[case("issues.list", json!({"fields": "full"}), "issues")]
#[case("issue.resolve", json!({"ref": "DEN-1"}), "issue")]
#[case("issue.create", json!({"name": "n", "description_html": "<p>x</p>"}), "created")]
#[case("issue.update", json!({"ref": UUID, "name": "renamed"}), "issue")]
#[case("issue.update", json!({"ref": UUID, "labels": ["agent:x", UUID]}), "issue")]
#[case("issue.set_status", json!({"ref": UUID, "status": "done"}), "state")]
#[case("issue.comment", json!({"ref": UUID, "comment": "hi"}), "ok")]
#[case("issue.comment_update", json!({"ref": UUID, "comment_id": "c-1", "comment_html": "<p>x</p>"}), "ok")]
#[case("relations.batch", json!({"issue_ids": ["a", "b"], "fresh": true}), "relations")]
#[case("cache.invalidate", json!({}), "ok")]
#[case("cache.invalidate", json!({"issue_ids": ["a"]}), "ok")]
fn each_method_answers_with_a_result(
    plane: (Fake, Server),
    #[case] method: &str,
    #[case] params: Value,
    #[case] key: &str,
) {
    let (_fake, mut server) = plane;
    let r = call(&mut server, method, params);
    assert!(r.get("error").is_none(), "{method}: {r}");
    assert!(r["result"].to_string().contains(key), "{method}: {r}");
}

#[rstest]
fn resolving_an_unknown_ticket_is_a_not_found_error(plane: (Fake, Server)) {
    let (_fake, mut server) = plane;
    let r = call(&mut server, "issue.resolve", json!({"ref": "DEN-999"}));
    assert_eq!(r["error"]["data"]["kind"], "not_found");
}
