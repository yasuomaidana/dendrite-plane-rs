//! Less-travelled paths: error kinds, bare list shapes, 404 mappings, label fallbacks, cache I/O.

mod fake_plane;

use fake_plane::{Fake, Reply, json};
use plane_client::{Client, Config, Error};
use rstest::{fixture, rstest};

const UUID: &str = "11111111-2222-3333-4444-555555555555";

fn client(fake: &Fake) -> Client {
    Client::new(Config::new(&fake.url, "ws", "proj", "k").unwrap())
}

#[fixture]
fn status(#[default(404)] code: u16) -> Reply {
    Reply {
        status: code,
        body: "{}".into(),
        delay: std::time::Duration::ZERO,
    }
}

#[rstest]
#[case(Error::MissingToken, "config")]
#[case(Error::Http { status: 500, what: "x".into() }, "http")]
#[case(Error::Decode("x".into()), "decode")]
fn error_kinds_are_stable(#[case] error: Error, #[case] kind: &str) {
    assert_eq!(error.kind(), kind);
}

#[test]
fn bare_array_lists_are_accepted() {
    let fake = Fake::start(|req| {
        let p = &req.path_and_query;
        if p.contains("/states/") {
            json(r#"[{"id":"s","name":"Done","group":"completed"}]"#)
        } else if p.contains("/labels/") {
            json(r#"[{"id":"l","name":"agent:a"}]"#)
        } else {
            json(r#"[{"id":"c-1","comment_html":"<p>hi</p>"}]"#)
        }
    });
    let c = client(&fake);
    assert_eq!(c.list_states().unwrap()[0].id, "s");
    assert_eq!(c.list_labels().unwrap()[0].name, "agent:a");
    assert_eq!(c.list_comments(UUID).unwrap().len(), 1);
}

#[rstest]
#[case("")]
#[case("  #123456 ")]
fn ensure_label_posts_with_a_default_or_trimmed_colour(#[case] color: &str) {
    let fake = Fake::start(|req| {
        if req.method == "POST" {
            json(r#"{"id":"new","name":"fresh"}"#)
        } else {
            json(r#"{"results":[]}"#)
        }
    });
    client(&fake).ensure_label("fresh", color).unwrap();
    let post = fake.recorded().into_iter().find(|r| r.method == "POST");
    let want = if color.is_empty() {
        "#7C3AED"
    } else {
        "#123456"
    };
    assert!(post.unwrap().body.contains(want));
}

#[rstest]
fn ensure_label_resolves_a_409_to_the_label_that_won_the_race(#[with(409)] status: Reply) {
    let listed = std::sync::atomic::AtomicUsize::new(0);
    let fake = Fake::start(move |req| {
        if req.method == "POST" {
            return status.clone();
        }
        let n = listed.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let rows = if n == 0 {
            ""
        } else {
            r#"{"id":"won","name":"x"}"#
        };
        json(&format!(r#"{{"results":[{rows}]}}"#))
    });
    assert_eq!(client(&fake).ensure_label("x", "").unwrap().id, "won");
}

#[test]
fn a_404_names_the_ticket_for_comment_and_update_calls() {
    let fake = Fake::start(|_| status(404));
    let c = client(&fake);
    assert!(matches!(
        c.comment(UUID, "hi"),
        Err(Error::TicketNotFound(_))
    ));
    assert!(matches!(
        c.update_comment(UUID, "c-1", "<p>x</p>"),
        Err(Error::TicketNotFound(_))
    ));
    assert!(matches!(
        c.update_issue(UUID, &plane_client::UpdateIssue::default()),
        Err(Error::TicketNotFound(_))
    ));
}

#[test]
fn patch_labels_falls_back_to_the_issue_label_details() {
    let fake = Fake::start(|req| {
        let p = &req.path_and_query;
        if p.contains("/labels/") {
            json(r#"{"results":[{"id":"l1","name":"agent:a"}]}"#)
        } else {
            json(&format!(
                r#"{{"id":"{UUID}","name":"t","labels":[],"label_details":[{{"id":"l1","name":"agent:a"}},{{"id":"","name":""}}]}}"#
            ))
        }
    });
    let (_, names) = client(&fake).patch_labels(UUID, &[], &[]).unwrap();
    assert_eq!(names, ["agent:a"]);
}

#[test]
fn an_unwritable_cache_file_costs_sharing_but_not_the_result() {
    let fake = Fake::start(|req| {
        if req.path_and_query.contains("/issue-relation/") {
            json(r#"{"blocked_by":[]}"#)
        } else {
            status(404)
        }
    });
    // A regular file stands where the cache directory should be, so the write cannot succeed.
    let blocker = std::env::temp_dir().join(format!("plane-gaps-{}", std::process::id()));
    std::fs::write(&blocker, "x").unwrap();
    let c = client(&fake).with_cache_file(blocker.join("cache.json"));
    let batch = c.relations_batch(&["a".to_string()], false);
    let _ = std::fs::remove_file(&blocker);
    assert_eq!((batch.fetched, batch.failed.len()), (1, 0));
}

#[test]
fn comment_calls_report_transport_failures() {
    // Nothing listens on this port.
    let c = Client::new(Config::new("http://127.0.0.1:9", "ws", "proj", "k").unwrap());
    assert!(matches!(c.comment(UUID, "hi"), Err(Error::Transport(_))));
    assert!(matches!(
        c.update_comment(UUID, "c-1", "<p>x</p>"),
        Err(Error::Transport(_))
    ));
}

#[rstest]
fn ensure_label_reports_a_409_that_names_no_existing_label(#[with(409)] status: Reply) {
    let fake = Fake::start(move |req| {
        if req.method == "POST" {
            status.clone()
        } else {
            json(r#"{"results":[]}"#)
        }
    });
    assert!(matches!(
        client(&fake).ensure_label("x", ""),
        Err(Error::Http { status: 409, .. })
    ));
}

#[test]
fn unreadable_bodies_are_decode_errors() {
    let fake = Fake::start(|_| json("not json"));
    let c = client(&fake);
    let new = plane_client::CreateIssue {
        name: "n".into(),
        description_html: "<p>x</p>".into(),
        ..Default::default()
    };
    assert!(matches!(c.get_issue(UUID), Err(Error::Decode(_))));
    assert!(matches!(c.list_comments(UUID), Err(Error::Decode(_))));
    assert!(matches!(c.create_issue(&new), Err(Error::Decode(_))));
    assert!(matches!(c.list_states(), Err(Error::Decode(_))));
    assert!(matches!(c.ensure_label("x", ""), Err(Error::Decode(_))));
}
