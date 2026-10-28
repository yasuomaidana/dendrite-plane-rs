mod fake_plane;

use std::time::{Duration, Instant};

use fake_plane::{Fake, Reply, json};
use plane_client::{Client, Config, Error, ListFields, RELATION_WORKERS, Status};

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
