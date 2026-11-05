use serde::{Deserialize, Serialize};

/// A Plane work item. Only the fields Dendrite uses; `description_html` is present only when the
/// full field set was requested.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub sequence_id: i64,
    /// UUID of the workflow state.
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub estimate_point: Option<i64>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_html: Option<String>,
    /// Present on single-issue reads and on create/update replies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub external_source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description_stripped: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_detail: Option<StateDetail>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub labels: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub assignees: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub label_details: Vec<LabelDetail>,
}

/// A project label.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Label {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub color: String,
}

/// The current authenticated user.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub display_name: String,
}

/// An issue comment.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Comment {
    pub id: String,
    #[serde(default)]
    pub comment_html: String,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
}

/// The state Plane embeds in a single-issue read.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct StateDetail {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub group: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct LabelDetail {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
}

/// Fields for a new issue, as Go's `plane.CreateIssueRequest` minus module and blocker links
/// (those move with DEN-355 and DEN-356).
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct CreateIssue {
    pub name: String,
    #[serde(default)]
    pub description_html: String,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub estimate_point: Option<i64>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub external_id: Option<String>,
    #[serde(default)]
    pub external_source: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

/// Fields to change on an issue; unset ones are left out of the PATCH.
#[derive(Debug, Clone, Default, PartialEq, Deserialize)]
pub struct UpdateIssue {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description_html: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub estimate_point: Option<i64>,
    #[serde(default)]
    pub priority: Option<String>,
    #[serde(default)]
    pub start_date: Option<String>,
    #[serde(default)]
    pub target_date: Option<String>,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub assignees: Option<Vec<String>>,
    #[serde(default)]
    pub labels: Option<Vec<String>>,
}

/// Go's `plane.IsEmptyContext`: true when a description has no text once tags (`<...>`) and
/// `&nbsp;` are removed. A `<` with no closing `>` is ordinary text.
pub fn is_empty_context(html_or_text: &str) -> bool {
    let mut rest = html_or_text;
    let mut text = String::with_capacity(rest.len());
    while let Some(open) = rest.find('<') {
        text.push_str(&rest[..open]);
        match rest[open..].find('>') {
            Some(close) => {
                text.push(' ');
                rest = &rest[open + close + 1..];
            }
            None => {
                text.push_str(&rest[open..]);
                rest = "";
            }
        }
    }
    text.push_str(rest);
    text.replace("&nbsp;", " ").trim().is_empty()
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct RelationRef {
    pub id: String,
    #[serde(default)]
    pub name: String,
}

/// The four relation lists Plane reports for one issue.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Relations {
    #[serde(default)]
    pub blocking: Vec<RelationRef>,
    #[serde(default)]
    pub blocked_by: Vec<RelationRef>,
    #[serde(default)]
    pub duplicate: Vec<RelationRef>,
    #[serde(default)]
    pub relates_to: Vec<RelationRef>,
}

/// Where a ticket is in its life. Mirrors Dendrite's Go `tracker.Status`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Status {
    Backlog,
    Todo,
    InProgress,
    InReview,
    Blocked,
    Done,
    Cancelled,
}

impl Status {
    /// The state name on a canonical Dendrite board.
    pub fn display_name(self) -> &'static str {
        match self {
            Status::Backlog => "Backlog",
            Status::Todo => "To Do",
            Status::InProgress => "In Progress",
            Status::InReview => "To Verify",
            Status::Blocked => "Blocked",
            Status::Done => "Done",
            Status::Cancelled => "Cancelled",
        }
    }

    /// Accepts the same spellings as the Go `ParseStatus`: case-insensitive, with `-`, `_` or a
    /// space as equivalent separators.
    pub fn parse(s: &str) -> Option<Status> {
        let norm: String = s
            .trim()
            .to_lowercase()
            .replace(['-', '_'], " ")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        Some(match norm.as_str() {
            "backlog" => Status::Backlog,
            "todo" | "to do" => Status::Todo,
            "in progress" | "wip" => Status::InProgress,
            "in review" | "review" | "to verify" | "verify" => Status::InReview,
            "blocked" => Status::Blocked,
            "done" | "complete" | "completed" => Status::Done,
            "cancelled" | "canceled" => Status::Cancelled,
            _ => return None,
        })
    }

    /// Finished work that never holds anything back and needs no relation lookup.
    pub fn is_closed(self) -> bool {
        matches!(self, Status::Done | Status::Cancelled)
    }
}

/// Maps a Plane workflow state (group and display name) to a [`Status`]. This is the single
/// mapping, identical to Go's `plane.StatusFor`: Cancelled and Blocked are matched before the group
/// checks because Plane files Blocked under "started".
pub fn status_for(group: &str, name: &str) -> Status {
    let g = group.to_lowercase();
    let n = name.to_lowercase();
    if g == "cancelled" || n.contains("cancel") {
        Status::Cancelled
    } else if n.contains("block") {
        Status::Blocked
    } else if g == "completed" || n.contains("done") {
        Status::Done
    } else if n.contains("review") || n.contains("verif") {
        Status::InReview
    } else if g == "started" || n.contains("progress") || n.contains("active") {
        Status::InProgress
    } else if g == "backlog" || n.contains("backlog") {
        Status::Backlog
    } else if g == "unstarted" || n.contains("todo") || n.contains("to do") {
        Status::Todo
    } else {
        Status::Backlog
    }
}

/// True for a canonical 8-4-4-4-12 hexadecimal UUID, the only form that may be used as an issue id
/// without a lookup.
pub fn is_uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.trim().split('-').collect();
    parts.len() == 5
        && parts
            .iter()
            .zip([8, 4, 4, 4, 12])
            .all(|(p, n)| p.len() == n && p.chars().all(|c| c.is_ascii_hexdigit()))
}

/// True when `s` is safe to place in a URL path segment: letters, digits, `-` and `_` only.
pub fn is_path_segment(s: &str) -> bool {
    !s.is_empty()
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// The origin (`scheme://host[:port]`) of `url` when it is a plain `http`/`https` URL whose host is
/// loopback (`localhost`, `127.0.0.0/8`, `::1`), and `None` for anything else: another host,
/// userinfo (`http://127.0.0.1@evil.example`), a path, query or fragment, or a bad port.
pub fn loopback_origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    if scheme != "http" && scheme != "https" {
        return None;
    }
    let authority = rest.trim_end_matches('/');
    if authority.is_empty() || authority.contains(['/', '?', '#', '@', '\\']) {
        return None;
    }
    let (host, port) = match authority.strip_prefix('[') {
        Some(v6) => {
            let (host, tail) = v6.split_once(']')?;
            (
                format!("[{host}]"),
                tail.strip_prefix(':').or(tail.is_empty().then_some("")),
            )
        }
        None => match authority.split_once(':') {
            Some((h, p)) => (h.to_string(), Some(p)),
            None => (authority.to_string(), Some("")),
        },
    };
    let port = port?;
    if !port.is_empty()
        && !(port.chars().all(|c| c.is_ascii_digit()) && port.parse::<u16>().is_ok())
    {
        return None;
    }
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let loopback = bare.eq_ignore_ascii_case("localhost")
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback());
    if !loopback {
        return None;
    }
    let colon = if port.is_empty() { "" } else { ":" };
    Some(format!(
        "{scheme}://{}{colon}{port}",
        host.to_ascii_lowercase()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_for_matches_the_go_table() {
        let cases = [
            ("backlog", "Backlog", Status::Backlog),
            ("unstarted", "To Do", Status::Todo),
            ("unstarted", "Todo", Status::Todo),
            ("started", "In Progress", Status::InProgress),
            ("started", "To Verify", Status::InReview),
            ("started", "In Review", Status::InReview),
            ("started", "Blocked", Status::Blocked),
            ("completed", "Done", Status::Done),
            ("cancelled", "Cancelled", Status::Cancelled),
            ("cancelled", "Canceled", Status::Cancelled),
            ("", "something odd", Status::Backlog),
        ];
        for (g, n, want) in cases {
            assert_eq!(status_for(g, n), want, "{g}/{n}");
        }
    }

    #[test]
    fn parse_accepts_lifecycle_spellings() {
        assert_eq!(Status::parse("In-Progress"), Some(Status::InProgress));
        assert_eq!(Status::parse(" to_verify "), Some(Status::InReview));
        assert_eq!(Status::parse("Canceled"), Some(Status::Cancelled));
        assert_eq!(Status::parse("blocked"), Some(Status::Blocked));
        assert_eq!(Status::parse("nonsense"), None);
    }

    #[test]
    fn every_status_name_round_trips_through_the_mapper() {
        for s in [
            Status::Backlog,
            Status::Todo,
            Status::InProgress,
            Status::InReview,
            Status::Blocked,
            Status::Done,
            Status::Cancelled,
        ] {
            assert_eq!(Status::parse(s.display_name()), Some(s));
        }
    }

    #[rstest::rstest]
    #[case("123e4567-e89b-12d3-a456-426614174000", true)]
    #[case("-/../../../../archive/-/-/-/-----", false)]
    #[case("123e4567-e89b-12d3-a456-42661417400g", false)]
    #[case("not-a-uuid", false)]
    #[case("", false)]
    fn is_uuid_accepts_only_canonical_uuids(#[case] input: &str, #[case] want: bool) {
        assert_eq!(is_uuid(input), want);
    }

    #[rstest::rstest]
    #[case("dendrite-ws", true)]
    #[case("proj_1", true)]
    #[case("a/b", false)]
    #[case("..", false)]
    #[case("a?b", false)]
    #[case("", false)]
    fn is_path_segment_rejects_separators(#[case] input: &str, #[case] want: bool) {
        assert_eq!(is_path_segment(input), want);
    }

    #[rstest::rstest]
    #[case("http://127.0.0.1:8090", Some("http://127.0.0.1:8090"))]
    #[case("http://127.0.0.1:8090/", Some("http://127.0.0.1:8090"))]
    #[case("http://localhost", Some("http://localhost"))]
    #[case("HTTP://x", None)]
    #[case("https://LOCALHOST:443", Some("https://localhost:443"))]
    #[case("http://[::1]:3000", Some("http://[::1]:3000"))]
    #[case("http://127.5.5.5:1", Some("http://127.5.5.5:1"))]
    #[case("http://evil.example", None)]
    #[case("http://127.0.0.1@evil.example", None)]
    #[case("http://evil.example@127.0.0.1", None)]
    #[case("http://127.0.0.1.evil.example", None)]
    #[case("http://localhost.evil.example:80", None)]
    #[case("http://127.0.0.1:99999", None)]
    #[case("http://127.0.0.1:80x", None)]
    #[case("http://127.0.0.1/api", None)]
    #[case("http://127.0.0.1?x=1", None)]
    #[case("ftp://127.0.0.1", None)]
    #[case("file:///etc/passwd", None)]
    #[case("", None)]
    fn loopback_origin_accepts_only_plain_loopback_urls(
        #[case] input: &str,
        #[case] want: Option<&str>,
    ) {
        assert_eq!(loopback_origin(input).as_deref(), want);
    }
}
