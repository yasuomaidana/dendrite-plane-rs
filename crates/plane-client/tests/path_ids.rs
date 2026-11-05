mod fake_plane;

use fake_plane::{Fake, json};
use plane_client::{Client, Config, Error};
use rstest::rstest;

#[rstest]
#[case("../../../../states/abc")]
#[case("a/b")]
#[case("a?x=1")]
#[case("..")]
#[case("a b")]
fn path_ids_with_separators_are_refused_before_any_request(#[case] bad: &str) {
    let fake = Fake::start(|_| json("{}"));
    let c = Client::new(Config::new(&fake.url, "ws", "proj", "test-key").unwrap());
    let uuid = "00000000-0000-4000-8000-000000000001";
    assert!(matches!(
        c.update_comment(uuid, bad, "<p>x</p>"),
        Err(Error::Invalid(_))
    ));
    assert!(matches!(c.relations(bad), Err(Error::Invalid(_))));
    let batch = c.relations_batch(&[bad.to_string()], true);
    assert_eq!(batch.failed.len(), 1);
    assert!(batch.relations.is_empty());
}
