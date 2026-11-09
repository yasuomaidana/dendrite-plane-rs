//! The real binary: environment configuration, the stdin/stdout loop and its exits.

#[path = "../../plane-client/tests/fake_plane.rs"]
mod fake_plane;

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};

use fake_plane::{Fake, json};
use rstest::{fixture, rstest};

fn spawn(fake: &Fake, cache_dir: &std::path::Path) -> std::process::Child {
    Command::new(env!("CARGO_BIN_EXE_dendrite-plane"))
        .env("PLANE_API_KEY", "k")
        .env("PLANE_BASE_URL", &fake.url)
        .env("PLANE_PROJECT_ID", "proj")
        .env("DENDRITE_PLANE_RELATIONS_TTL_SECS", "60")
        .env("DENDRITE_PLANE_CACHE_DIR", cache_dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

#[fixture]
fn plane() -> Fake {
    Fake::start(|_| json(r#"{"blocked_by":[]}"#))
}

#[rstest]
fn answers_each_line_skips_blank_ones_and_caches_relations_on_disk(plane: Fake) {
    let (fake, dir) = (
        plane,
        std::env::temp_dir().join(format!("dplane-bin-{}", std::process::id())),
    );
    let mut child = spawn(&fake, &dir);
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id":1,"method":"health"}}"#).unwrap();
    writeln!(stdin, "   ").unwrap();
    writeln!(
        stdin,
        r#"{{"id":2,"method":"relations.batch","params":{{"issue_ids":["a"]}}}}"#
    )
    .unwrap();
    drop(stdin);
    let replies: Vec<String> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap)
        .collect();
    assert!(child.wait().unwrap().success());
    assert_eq!(replies.len(), 2, "{replies:?}");
    assert!(replies[1].contains(r#""fetched":1"#), "{}", replies[1]);
    assert!(std::fs::read_dir(&dir).unwrap().next().is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[rstest]
fn stops_when_the_reader_of_its_replies_is_gone(plane: Fake) {
    let (fake, dir) = (plane, std::env::temp_dir());
    let mut child = spawn(&fake, &dir);
    drop(child.stdout.take());
    let mut stdin = child.stdin.take().unwrap();
    // The write may itself fail once the child has exited; either way the child must stop.
    let _ = writeln!(stdin, r#"{{"id":1,"method":"health"}}"#);
    drop(stdin);
    assert!(child.wait().unwrap().success());
}

#[test]
fn without_a_token_health_answers_and_plane_calls_name_the_fix() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_dendrite-plane"))
        .env_remove("PLANE_API_KEY")
        .env_remove("PLANE_BASE_URL")
        .env_remove("DENDRITE_PLANE_CACHE_DIR")
        .env_remove("DENDRITE_PLANE_RELATIONS_TTL_SECS")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, r#"{{"id":1,"method":"health"}}"#).unwrap();
    writeln!(stdin, r#"{{"id":2,"method":"states.list"}}"#).unwrap();
    drop(stdin);
    let replies: Vec<String> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(Result::unwrap)
        .collect();
    assert!(child.wait().unwrap().success());
    assert!(replies[0].contains(r#""configured":false"#), "{replies:?}");
    assert!(replies[1].contains(r#""kind":"config""#), "{replies:?}");
}
