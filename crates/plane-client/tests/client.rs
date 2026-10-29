mod fake_plane;

use std::time::{Duration, Instant};

use fake_plane::{Fake, Reply, json};
use plane_client::{
    Client, Config, CreateIssue, Error, ListFields, RELATION_WORKERS, Status, UpdateIssue,
    is_empty_context,
};

fn client(fake: &Fake) -> Client {
    Client::new(Config::new(&fake.url, "ws", "proj", "test-key").unwrap())
}

const STATES: &str = r#"{"results":[
  {"id":"s-todo","name":"To Do","group":"unstarted"},
  {"id":"s-prog","name":"In Progress","group":"started"},
  {"id":"s-done","name":"Done","group":"completed"}]}"#;

fn issue(id: &str, seq: i64, state: &str, name: &str) -> String {
    format!(r#"{{"id":"{id}","sequence_id":{seq},"state":"{state}","name":"{name}"}}"#)
}

#[test]
fn empty_token_is_refused_and_never_defaulted() {
    assert!(matches!(
        Config::new("http://x", "ws", "p", "  "),
        Err(Error::MissingToken)
    ));
}

#[test]
fn list_issues_follows_cursors_and_asks_for_small_pages_with_fields() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("cursor=c2") {
            json(&format!(
                r#"{{"results":[{}],"next_page_results":false,"next_cursor":""}}"#,
                issue("b", 2, "s-todo", "[DEN-2] two")
            ))
        } else {
            json(&format!(
                r#"{{"results":[{}],"next_page_results":true,"next_cursor":"c2"}}"#,
                issue("a", 1, "s-todo", "[DEN-1] one")
            ))
        }
    });
    let issues = client(&fake).list_issues(ListFields::Min).unwrap();
    assert_eq!(
        issues.iter().map(|i| i.id.as_str()).collect::<Vec<_>>(),
        ["a", "b"]
    );

    let reqs = fake.recorded();
    assert_eq!(reqs.len(), 2);
    for r in &reqs {
        assert!(
            r.path_and_query.starts_with(
                "/api/v1/workspaces/ws/projects/proj/issues/?per_page=100&fields=id,name"
            ),
            "{}",
            r.path_and_query
        );
        assert_eq!(r.api_key.as_deref(), Some("test-key"));
    }
    assert!(reqs[1].path_and_query.ends_with("&cursor=c2"));

    let full = Fake::start(|_| json(r#"{"results":[],"next_page_results":false}"#));
    client(&full).list_issues(ListFields::Full).unwrap();
    assert!(!full.recorded()[0].path_and_query.contains("fields="));
}

#[test]
fn snapshot_skips_closed_tickets_and_fetches_relations_concurrently() {
    let n_open = 48;
    let fake = Fake::start(move |req| {
        let p = &req.path_and_query;
        if p.contains("/states/") {
            json(STATES)
        } else if p.contains("/issue-relation/") {
            if p.contains("/issues/open-0/") {
                return Reply {
                    status: 500,
                    body: "boom".into(),
                    delay: Duration::ZERO,
                };
            }
            Reply {
                status: 200,
                body: r#"{"blocked_by":[{"id":"x","name":"[DEN-1] x"}]}"#.into(),
                delay: Duration::from_millis(25),
            }
        } else {
            let mut results: Vec<String> = (0..n_open)
                .map(|i| issue(&format!("open-{i}"), i + 10, "s-todo", "open"))
                .collect();
            results.push(issue("closed-1", 1, "s-done", "[DEN-1] finished"));
            json(&format!(
                r#"{{"results":[{}],"next_page_results":false}}"#,
                results.join(",")
            ))
        }
    });

    let started = Instant::now();
    let snap = client(&fake).snapshot(false).unwrap();
    let elapsed = started.elapsed();

    assert_eq!(snap.issues.len(), n_open as usize + 1);
    assert_eq!(snap.statuses["closed-1"], Status::Done);
    assert_eq!(snap.statuses["open-3"], Status::Todo);
    assert_eq!(
        snap.relations.len(),
        n_open as usize - 1,
        "the failing issue is absent, the closed one never asked"
    );
    assert_eq!(snap.relation_errors, 1);
    assert!(!snap.relations.contains_key("closed-1"));
    assert!(
        fake.recorded().iter().all(|r| !r
            .path_and_query
            .contains("/issues/closed-1/issue-relation/")),
        "relations were requested for a Done ticket"
    );
    // The relation route is the internal one: the public v1 API has none.
    assert!(fake.recorded().iter().any(|r| {
        r.path_and_query
            .starts_with("/api/workspaces/ws/projects/proj/issues/open-1/issue-relation/")
    }));

    let peak = fake
        .peak_in_flight
        .load(std::sync::atomic::Ordering::SeqCst);
    assert!(
        (2..=RELATION_WORKERS + 1).contains(&peak),
        "peak concurrency {peak}"
    );
    let serial = Duration::from_millis(25 * n_open as u64);
    assert!(
        elapsed < serial / 2,
        "took {elapsed:?}, serial would be {serial:?}"
    );
}

#[test]
fn set_status_resolves_once_and_patches_the_matching_state() {
    let fake = Fake::start(|req| {
        let p = &req.path_and_query;
        if req.method == "PATCH" {
            json("{}")
        } else if p.contains("/states/") {
            json(STATES)
        } else {
            json(&format!(
                r#"{{"results":[{},{}],"next_page_results":false}}"#,
                issue("iss-fresh", 324, "s-todo", "Fresh ticket"),
                issue("iss-old", 300, "s-todo", "[DEN-1014] legacy")
            ))
        }
    });
    let (issue, state) = client(&fake)
        .set_status("DEN-324", Status::InProgress)
        .unwrap();
    assert_eq!(
        (issue.id.as_str(), state.id.as_str()),
        ("iss-fresh", "s-prog")
    );

    let reqs = fake.recorded();
    let patch = reqs.iter().find(|r| r.method == "PATCH").unwrap();
    assert_eq!(
        patch.path_and_query,
        "/api/v1/workspaces/ws/projects/proj/issues/iss-fresh/"
    );
    assert!(patch.body.contains("s-prog"), "{}", patch.body);
    assert_eq!(
        reqs.iter()
            .filter(|r| r.path_and_query.contains("/issues/?"))
            .count(),
        1,
        "the issue list was fetched more than once"
    );

    let err = client(&fake)
        .set_status("DEN-999", Status::Done)
        .unwrap_err();
    assert!(
        matches!(err, Error::TicketNotFound(ref t) if t == "DEN-999"),
        "{err}"
    );
    let err = client(&fake)
        .set_status("DEN-324", Status::Blocked)
        .unwrap_err();
    assert!(matches!(err, Error::StateNotFound(_)), "{err}");
}

#[test]
fn rejected_keys_and_server_errors_are_distinct() {
    let denied = Fake::start(|_| Reply {
        status: 401,
        body: "no".into(),
        delay: Duration::ZERO,
    });
    assert!(matches!(
        client(&denied).list_states().unwrap_err(),
        Error::Unauthorized(401)
    ));
    let broken = Fake::start(|_| Reply {
        status: 503,
        body: "down".into(),
        delay: Duration::ZERO,
    });
    assert!(matches!(
        client(&broken).list_states().unwrap_err(),
        Error::Http { status: 503, .. }
    ));
}

#[test]
fn relations_are_cached_until_they_expire_are_invalidated_or_bypassed() {
    let fake = Fake::start(|req| {
        let p = &req.path_and_query;
        if p.contains("/issue-relation/") {
            json(r#"{"blocked_by":[]}"#)
        } else if p.contains("/states/") {
            json(STATES)
        } else {
            json(&format!(
                r#"{{"results":[{},{}],"next_page_results":false}}"#,
                issue("a", 1, "s-todo", "[DEN-1] one"),
                issue("b", 2, "s-prog", "[DEN-2] two")
            ))
        }
    });
    let relation_calls = |f: &Fake| {
        f.recorded()
            .iter()
            .filter(|r| r.path_and_query.contains("/issue-relation/"))
            .count()
    };
    let c = client(&fake);

    let first = c.snapshot(false).unwrap();
    assert_eq!(
        (
            first.relations_fetched,
            first.relations_cached,
            relation_calls(&fake)
        ),
        (2, 0, 2)
    );

    let second = c.snapshot(false).unwrap();
    assert_eq!(
        (
            second.relations_fetched,
            second.relations_cached,
            relation_calls(&fake)
        ),
        (0, 2, 2),
        "a warm snapshot must not ask Plane for relations"
    );
    assert_eq!(second.relations.len(), 2);

    c.invalidate_relations(&["a".to_string()]);
    let third = c.snapshot(false).unwrap();
    assert_eq!(
        (
            third.relations_fetched,
            third.relations_cached,
            relation_calls(&fake)
        ),
        (1, 1, 3),
        "only the invalidated issue is refetched"
    );

    let fresh = c.snapshot(true).unwrap();
    assert_eq!(
        (fresh.relations_fetched, relation_calls(&fake)),
        (2, 5),
        "fresh bypasses the cache"
    );

    c.clear_relations_cache();
    assert_eq!(c.snapshot(false).unwrap().relations_fetched, 2);

    let short = client(&fake).with_relations_ttl(Duration::from_millis(30));
    short.snapshot(false).unwrap();
    std::thread::sleep(Duration::from_millis(60));
    assert_eq!(
        short.snapshot(false).unwrap().relations_fetched,
        2,
        "expired entries are refetched"
    );
}

#[test]
fn list_issues_drops_rows_plane_repeats_for_each_module_an_issue_belongs_to() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("cursor=c2") {
            // "a" shows up again on the next page, as Plane does for an issue in two modules.
            json(&format!(
                r#"{{"results":[{},{}],"next_page_results":false}}"#,
                issue("a", 1, "s-todo", "[DEN-1] one"),
                issue("b", 2, "s-todo", "[DEN-2] two")
            ))
        } else {
            json(&format!(
                r#"{{"results":[{},{}],"next_page_results":true,"next_cursor":"c2"}}"#,
                issue("a", 1, "s-todo", "[DEN-1] one"),
                issue("c", 3, "s-todo", "[DEN-3] three")
            ))
        }
    });
    let ids: Vec<String> = client(&fake)
        .list_issues(ListFields::Min)
        .unwrap()
        .into_iter()
        .map(|i| i.id)
        .collect();
    assert_eq!(ids, ["a", "c", "b"], "first occurrence wins, order is kept");
}

#[test]
fn a_cache_file_is_shared_between_clients_like_two_cli_runs() {
    let fake = Fake::start(|req| {
        let p = &req.path_and_query;
        if p.contains("/issue-relation/") {
            json(r#"{"blocked_by":[]}"#)
        } else if p.contains("/states/") {
            json(STATES)
        } else {
            json(&format!(
                r#"{{"results":[{}],"next_page_results":false}}"#,
                issue("a", 1, "s-todo", "[DEN-1] one")
            ))
        }
    });
    let dir = std::env::temp_dir().join(format!("plane-cache-test-{}", std::process::id()));
    let file = dir.join("rel.json");
    let _ = std::fs::remove_dir_all(&dir);
    let relation_calls = |f: &Fake| {
        f.recorded()
            .iter()
            .filter(|r| r.path_and_query.contains("/issue-relation/"))
            .count()
    };

    let first = client(&fake).with_cache_file(file.clone());
    assert_eq!(first.snapshot(false).unwrap().relations_fetched, 1);
    assert!(file.exists(), "the cache was not written");

    // A second process starts cold in memory but finds the file.
    let second = client(&fake).with_cache_file(file.clone());
    let warm = second.snapshot(false).unwrap();
    assert_eq!(
        (
            warm.relations_fetched,
            warm.relations_cached,
            relation_calls(&fake)
        ),
        (0, 1, 1)
    );

    // One process invalidates; the other sees it without being told.
    second.invalidate_relations(&["a".to_string()]);
    let after = first.snapshot(false).unwrap();
    assert_eq!(
        (after.relations_fetched, relation_calls(&fake)),
        (1, 2),
        "the first client served a relation set the second one invalidated"
    );

    // An unreadable file only costs sharing.
    std::fs::write(&file, b"not json").unwrap();
    let third = client(&fake).with_cache_file(file);
    assert_eq!(third.snapshot(false).unwrap().relations_fetched, 1);
    let _ = std::fs::remove_dir_all(&dir);
}

const UUID: &str = "11111111-2222-3333-4444-555555555555";

fn body_of(fake: &Fake, method: &str) -> serde_json::Value {
    let r = fake
        .recorded()
        .into_iter()
        .find(|r| r.method == method)
        .unwrap_or_else(|| panic!("no {method} request"));
    serde_json::from_str(&r.body).unwrap()
}

fn new_issue(name: &str) -> CreateIssue {
    CreateIssue {
        name: name.into(),
        description_html: "<p>goal</p>".into(),
        ..CreateIssue::default()
    }
}

#[test]
fn empty_context_matches_the_go_rule() {
    for empty in [
        "",
        "  ",
        "<p></p>",
        "<div><p>&nbsp;</p></div>",
        "<br/> &nbsp; ",
    ] {
        assert!(is_empty_context(empty), "{empty:?}");
    }
    for filled in ["x", "<p>x</p>", "a < b", "<p>&nbsp;y</p>"] {
        assert!(!is_empty_context(filled), "{filled:?}");
    }
}

#[test]
fn get_issue_reads_a_uuid_directly_and_resolves_a_ticket_id_through_one_list() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("per_page=100") {
            json(&format!(
                r#"{{"results":[{}],"next_page_results":false,"next_cursor":""}}"#,
                issue(UUID, 7, "s-todo", "[DEN-7] seven")
            ))
        } else {
            json(&format!(
                r#"{{"id":"{UUID}","name":"[DEN-7] seven","description_stripped":"body",
                "state_detail":{{"name":"To Do","group":"unstarted"}},
                "label_details":[{{"name":"bug"}}]}}"#
            ))
        }
    });
    let c = client(&fake);
    let got = c.get_issue("DEN-7").unwrap();
    assert_eq!(got.description_stripped.as_deref(), Some("body"));
    assert_eq!(got.label_details[0].name, "bug");
    assert_eq!(got.state_detail.unwrap().group, "unstarted");

    let before = fake.recorded().len();
    c.get_issue(UUID).unwrap();
    assert_eq!(fake.recorded().len(), before + 1, "a UUID needs no list");
}

#[test]
fn get_issue_maps_404_and_unknown_ids_to_not_found() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("per_page=100") {
            json(r#"{"results":[],"next_page_results":false,"next_cursor":""}"#)
        } else {
            Reply {
                status: 404,
                body: "{}".into(),
                delay: Duration::ZERO,
            }
        }
    });
    let c = client(&fake);
    assert!(matches!(c.get_issue(UUID), Err(Error::TicketNotFound(_))));
    assert!(matches!(
        c.get_issue("DEN-9"),
        Err(Error::TicketNotFound(_))
    ));
}

#[test]
fn create_sends_the_go_body_clamps_the_estimate_and_reports_created() {
    let fake = Fake::start(|_| json(&format!(r#"{{"id":"{UUID}","name":"n","sequence_id":3}}"#)));
    let mut req = new_issue("n");
    req.estimate_point = Some(99);
    req.priority = Some("high".into());
    req.start_date = Some("2026-10-27".into());
    req.labels = vec!["l1".into()];
    req.state = Some(String::new());
    let (issue, created) = client(&fake).create_issue(&req).unwrap();
    assert!(created);
    assert_eq!(issue.id, UUID);

    let rec = &fake.recorded()[0];
    assert_eq!(rec.method, "POST");
    assert_eq!(
        rec.path_and_query,
        "/api/v1/workspaces/ws/projects/proj/issues/"
    );
    let b = body_of(&fake, "POST");
    assert_eq!(b["estimate_point"], 12);
    assert_eq!(b["priority"], "high");
    assert_eq!(b["start_date"], "2026-10-27");
    assert_eq!(b["labels"][0], "l1");
    assert_eq!(b["description_html"], "<p>goal</p>");
    assert!(b.get("state").is_none(), "an empty state is left out");
    assert!(b.get("external_id").is_none());
}

#[test]
fn create_refuses_an_empty_name_or_description_without_calling_plane() {
    let fake = Fake::start(|_| json("{}"));
    let c = client(&fake);
    assert!(matches!(
        c.create_issue(&new_issue("  ")),
        Err(Error::Invalid(_))
    ));
    let mut empty = new_issue("n");
    empty.description_html = "<p>&nbsp;</p>".into();
    assert!(matches!(c.create_issue(&empty), Err(Error::Invalid(_))));
    assert!(fake.recorded().is_empty());
}

#[test]
fn create_with_a_known_external_id_returns_the_existing_issue_without_posting() {
    let fake = Fake::start(|req| {
        assert_eq!(req.method, "GET", "nothing may be posted");
        json(
            r#"{"results":[{"id":"old","name":"x","sequence_id":1,"external_id":"OXI-101"}],
            "next_page_results":false,"next_cursor":""}"#,
        )
    });
    let mut req = new_issue("x");
    req.external_id = Some("OXI-101".into());
    let (issue, created) = client(&fake).create_issue(&req).unwrap();
    assert_eq!((issue.id.as_str(), created), ("old", false));
}

#[test]
fn create_treats_a_409_as_an_existing_issue() {
    let listed = std::sync::atomic::AtomicUsize::new(0);
    let fake = Fake::start(move |req| {
        if req.method == "POST" {
            return Reply {
                status: 409,
                body: "{}".into(),
                delay: Duration::ZERO,
            };
        }
        // The first list (before the POST) misses; the one after the 409 finds it.
        let n = listed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let rows = if n == 0 {
            ""
        } else {
            r#"{"id":"raced","external_id":"E1"}"#
        };
        json(&format!(
            r#"{{"results":[{rows}],"next_page_results":false,"next_cursor":""}}"#
        ))
    });
    let mut req = new_issue("x");
    req.external_id = Some("E1".into());
    let (issue, created) = client(&fake).create_issue(&req).unwrap();
    assert_eq!((issue.id.as_str(), created), ("raced", false));
}

#[test]
fn update_patches_only_set_fields_and_maps_errors() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("missing") {
            Reply {
                status: 404,
                body: "{}".into(),
                delay: Duration::ZERO,
            }
        } else {
            json(&format!(r#"{{"id":"{UUID}","name":"renamed"}}"#))
        }
    });
    let c = client(&fake);
    let upd = UpdateIssue {
        name: Some("renamed".into()),
        estimate_point: Some(-4),
        description_html: Some(String::new()),
        ..UpdateIssue::default()
    };
    assert_eq!(c.update_issue(UUID, &upd).unwrap().name, "renamed");
    let rec = &fake.recorded()[0];
    assert_eq!(rec.method, "PATCH");
    assert_eq!(
        rec.path_and_query,
        format!("/api/v1/workspaces/ws/projects/proj/issues/{UUID}/")
    );
    let b = body_of(&fake, "PATCH");
    assert_eq!(
        b,
        serde_json::json!({"name": "renamed", "estimate_point": 0})
    );

    let gone = "aaaaaaaa-missing-aaaa-aaaa-aaaaaaaaaaaa";
    assert!(matches!(
        c.update_issue(gone, &upd),
        Err(Error::TicketNotFound(_))
    ));
}

#[test]
fn comment_posts_the_go_html_and_maps_errors() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("per_page=100") {
            json(&format!(
                r#"{{"results":[{}],"next_page_results":false,"next_cursor":""}}"#,
                issue(UUID, 5, "s-todo", "[DEN-5] five")
            ))
        } else {
            json("{}")
        }
    });
    let c = client(&fake);
    c.comment("DEN-5", "hello <b>x</b>").unwrap();
    let post = fake
        .recorded()
        .into_iter()
        .find(|r| r.method == "POST")
        .unwrap();
    assert_eq!(
        post.path_and_query,
        format!("/api/v1/workspaces/ws/projects/proj/issues/{UUID}/comments/")
    );
    assert_eq!(
        body_of(&fake, "POST")["comment_html"],
        "<p>hello <b>x</b></p>"
    );
    assert!(matches!(c.comment("DEN-5", "  "), Err(Error::Invalid(_))));
    assert!(matches!(
        c.comment("DEN-77", "hi"),
        Err(Error::TicketNotFound(_))
    ));
}

#[test]
fn archive_project_posts_to_archive_url_and_maps_errors() {
    let fake = Fake::start(|req| {
        if req.method == "POST"
            && req.path_and_query == "/api/v1/workspaces/ws/projects/proj/archive/"
        {
            Reply {
                status: 204,
                body: String::new(),
                delay: Duration::ZERO,
            }
        } else if req.path_and_query.contains("missing") {
            Reply {
                status: 404,
                body: "{}".into(),
                delay: Duration::ZERO,
            }
        } else {
            Reply {
                status: 401,
                body: "unauthorized".into(),
                delay: Duration::ZERO,
            }
        }
    });

    let c = client(&fake);
    c.archive_project().unwrap();
    let reqs = fake.recorded();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(
        reqs[0].path_and_query,
        "/api/v1/workspaces/ws/projects/proj/archive/"
    );
    assert_eq!(reqs[0].body, "{}");
    assert_eq!(reqs[0].api_key.as_deref(), Some("test-key"));

    let missing = Client::new(Config::new(&fake.url, "ws", "missing", "test-key").unwrap());
    let err = missing.archive_project().unwrap_err();
    assert_eq!(err.kind(), "not_found");
    assert!(matches!(err, Error::ProjectNotFound(ref p) if p == "missing"));

    let denied = Client::new(Config::new(&fake.url, "ws", "other", "wrong-key").unwrap());
    let err = denied.archive_project().unwrap_err();
    assert_eq!(err.kind(), "unauthorized");
    assert!(matches!(err, Error::Unauthorized(401)));
}

#[test]
fn delete_project_sends_delete_and_maps_errors() {
    let fake = Fake::start(|req| {
        if req.method == "DELETE" && req.path_and_query == "/api/v1/workspaces/ws/projects/proj/" {
            Reply {
                status: 204,
                body: String::new(),
                delay: Duration::ZERO,
            }
        } else if req.path_and_query.contains("missing") {
            Reply {
                status: 404,
                body: "{}".into(),
                delay: Duration::ZERO,
            }
        } else {
            Reply {
                status: 401,
                body: "unauthorized".into(),
                delay: Duration::ZERO,
            }
        }
    });

    let c = client(&fake);
    c.delete_project().unwrap();
    let reqs = fake.recorded();
    assert_eq!(reqs.len(), 1);
    assert_eq!(reqs[0].method, "DELETE");
    assert_eq!(
        reqs[0].path_and_query,
        "/api/v1/workspaces/ws/projects/proj/"
    );
    assert_eq!(reqs[0].api_key.as_deref(), Some("test-key"));

    let missing = Client::new(Config::new(&fake.url, "ws", "missing", "test-key").unwrap());
    let err = missing.delete_project().unwrap_err();
    assert_eq!(err.kind(), "not_found");
    assert!(matches!(err, Error::ProjectNotFound(ref p) if p == "missing"));

    let denied = Client::new(Config::new(&fake.url, "ws", "other", "wrong-key").unwrap());
    let err = denied.delete_project().unwrap_err();
    assert_eq!(err.kind(), "unauthorized");
    assert!(matches!(err, Error::Unauthorized(401)));
}

#[test]
fn ensure_label_creating_and_reusing() {
    let mut state = fake_plane::FakeState::default();
    state.labels.push(serde_json::json!({
        "id": "lbl-existing",
        "name": "bug",
        "color": "#FF0000"
    }));
    let (fake, _fake_state) = Fake::with_routes(state);
    let c = client(&fake);

    // Reusing existing label
    let reused = c.ensure_label("bug", "#00FF00").unwrap();
    assert_eq!(reused.id, "lbl-existing");
    assert_eq!(reused.name, "bug");
    assert_eq!(reused.color, "#FF0000");

    // Creating a new label
    let created = c.ensure_label("feature", "#0000FF").unwrap();
    assert_eq!(created.name, "feature");
    assert_eq!(created.color, "#0000FF");

    // Verify it is now in state and can be reused
    let reused_new = c.ensure_label("feature", "#0000FF").unwrap();
    assert_eq!(reused_new.id, created.id);

    // Empty input fails
    assert!(matches!(
        c.ensure_label("  ", "#fff"),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn patch_labels_replaces_agent_and_tier_on_second_call_without_duplicates() {
    let mut state = fake_plane::FakeState::default();
    state.labels.push(serde_json::json!({
        "id": "lbl-bug",
        "name": "bug",
        "color": "#ff0000"
    }));
    state.labels.push(serde_json::json!({
        "id": "lbl-agent-old",
        "name": "agent:coder",
        "color": "#7C3AED"
    }));
    state.labels.push(serde_json::json!({
        "id": "lbl-tier-old",
        "name": "tier:1",
        "color": "#7C3AED"
    }));

    let issue_val = serde_json::json!({
        "id": UUID,
        "name": "[DEN-1] test ticket",
        "sequence_id": 1,
        "state": "s-todo",
        "labels": ["lbl-bug", "lbl-agent-old", "lbl-tier-old"],
        "label_details": [
            {"id": "lbl-bug", "name": "bug"},
            {"id": "lbl-agent-old", "name": "agent:coder"},
            {"id": "lbl-tier-old", "name": "tier:1"}
        ]
    });
    state.issues.insert(UUID.to_string(), issue_val);

    let (fake, _fake_state) = Fake::with_routes(state);
    let c = client(&fake);

    let add = vec!["agent:planner".to_string(), "tier:2".to_string()];
    let remove_prefixes = vec!["agent:".to_string(), "tier:".to_string()];

    // First call: replaces agent:coder with agent:planner, tier:1 with tier:2, keeps bug
    let (_iss1, labels1) = c.patch_labels(UUID, &add, &remove_prefixes).unwrap();
    assert_eq!(labels1, vec!["bug", "agent:planner", "tier:2"]);

    // Second call with same arguments: must produce exact same label list with NO duplicates!
    let (_iss2, labels2) = c.patch_labels(UUID, &add, &remove_prefixes).unwrap();
    assert_eq!(labels2, vec!["bug", "agent:planner", "tier:2"]);

    // Unknown ticket fails
    let err = c
        .patch_labels("missing-uuid", &add, &remove_prefixes)
        .unwrap_err();
    assert_eq!(err.kind(), "not_found");

    // Empty input fails
    let err = c.patch_labels("   ", &add, &remove_prefixes).unwrap_err();
    assert_eq!(err.kind(), "invalid");
}

#[test]
fn me_returns_user_and_handles_auth_error() {
    let state = fake_plane::FakeState::default();
    let (fake, _) = Fake::with_routes(state);
    let c = client(&fake);
    let user = c.me().unwrap();
    assert_eq!(user.id, "usr-1");
    assert_eq!(user.email, "agent@example.com");
    assert_eq!(user.display_name, "Agent Dendrite");

    let denied = Client::new(Config::new(&fake.url, "ws", "p", "wrong-key").unwrap());
    let err = denied.me().unwrap_err();
    assert_eq!(err.kind(), "unauthorized");
}

#[test]
fn list_comments_returns_oldest_first_and_maps_unknown_ticket() {
    let mut state = fake_plane::FakeState::default();
    state.issues.insert(
        UUID.to_string(),
        serde_json::json!({
            "id": UUID,
            "name": "[DEN-1] test ticket",
            "sequence_id": 1,
            "state": "s-todo"
        }),
    );
    state.comments.insert(
        UUID.to_string(),
        vec![
            serde_json::json!({
                "id": "c-2",
                "comment_html": "<p>second</p>",
                "created_at": "2026-10-28T12:02:00Z",
                "updated_at": "2026-10-28T12:02:00Z"
            }),
            serde_json::json!({
                "id": "c-1",
                "comment_html": "<p>first</p>",
                "created_at": "2026-10-28T12:01:00Z",
                "updated_at": "2026-10-28T12:01:00Z"
            }),
        ],
    );

    let (fake, _) = Fake::with_routes(state);
    let c = client(&fake);
    let comments = c.list_comments(UUID).unwrap();
    assert_eq!(comments.len(), 2);
    // Oldest first: c-1 then c-2
    assert_eq!(comments[0].id, "c-1");
    assert_eq!(comments[1].id, "c-2");

    // Unknown ticket
    let err = c.list_comments("unknown-uuid").unwrap_err();
    assert_eq!(err.kind(), "not_found");

    // Empty input
    let err = c.list_comments("   ").unwrap_err();
    assert_eq!(err.kind(), "invalid");
}

#[test]
fn update_comment_patches_as_is_and_maps_unknown_comment_or_ticket() {
    let mut state = fake_plane::FakeState::default();
    state.issues.insert(
        UUID.to_string(),
        serde_json::json!({
            "id": UUID,
            "name": "[DEN-1] test ticket",
            "sequence_id": 1,
            "state": "s-todo"
        }),
    );
    state.comments.insert(
        UUID.to_string(),
        vec![serde_json::json!({
            "id": "c-1",
            "comment_html": "original",
            "created_at": "2026-10-28T12:00:00Z",
            "updated_at": "2026-10-28T12:00:00Z"
        })],
    );

    let (fake, _) = Fake::with_routes(state);
    let c = client(&fake);

    // Update comment with raw html (sent as-is without <p> wrapping)
    c.update_comment(UUID, "c-1", "plain text without tags")
        .unwrap();

    let reqs = fake.recorded();
    let patch_req = reqs
        .iter()
        .find(|r| r.method == "PATCH" && r.path_and_query.contains("/comments/c-1/"))
        .unwrap();
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&patch_req.body).unwrap()["comment_html"],
        "plain text without tags"
    );

    // Unknown comment id on existing ticket
    let err = c.update_comment(UUID, "c-999", "new").unwrap_err();
    assert_eq!(err.kind(), "not_found");
    assert!(matches!(err, Error::CommentNotFound(_)));

    // Unknown ticket
    let err = c
        .update_comment("missing-ticket", "c-1", "new")
        .unwrap_err();
    assert_eq!(err.kind(), "not_found");
    assert!(matches!(err, Error::TicketNotFound(_)));

    // Empty input checks
    assert!(matches!(
        c.update_comment("  ", "c-1", "x"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        c.update_comment(UUID, "  ", "x"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(
        c.update_comment(UUID, "c-1", "  "),
        Err(Error::Invalid(_))
    ));
}

#[test]
fn update_issue_replaces_assignees_and_labels_and_preserves_unset() {
    let fake = Fake::start(|_| json(&format!(r#"{{"id":"{UUID}","name":"ok"}}"#)));
    let c = client(&fake);

    let upd = UpdateIssue {
        name: Some("test".into()),
        assignees: Some(vec!["u-1".into(), "u-2".into()]),
        labels: Some(vec![]), // empty replacement list
        ..UpdateIssue::default()
    };
    c.update_issue(UUID, &upd).unwrap();

    let b = body_of(&fake, "PATCH");
    assert_eq!(b["name"], "test");
    assert_eq!(b["assignees"], serde_json::json!(["u-1", "u-2"]));
    assert_eq!(b["labels"], serde_json::json!([]));
    assert!(b.get("description_html").is_none());
}
