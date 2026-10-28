use crate::types::Issue;

/// Resolves a user-supplied ticket reference to an issue, with the exact rules of Go's
/// `plane.ResolveIssue`: a UUID, a bare Plane sequence number, a `[ID]`-prefixed title, a whole-token
/// mention, then the sequence of a ticket whose title carries no `[ID]` prefix yet. It never matches
/// by substring, so `DEN-10` cannot resolve to `[DEN-1014]`.
pub fn resolve_issue<'a>(issues: &'a [Issue], reference: &str) -> Option<&'a Issue> {
    let reference = reference.trim();
    if reference.is_empty() {
        return None;
    }
    if let Ok(n) = reference.parse::<i64>() {
        return issues.iter().find(|i| i.sequence_id == n);
    }
    let prefix = format!("[{reference}]");
    if let Some(i) = issues
        .iter()
        .find(|i| i.id == reference || i.name.trim().starts_with(&prefix))
    {
        return Some(i);
    }
    if let Some(i) = issues.iter().find(|i| has_token(&i.name, reference)) {
        return Some(i);
    }
    let seq = ticket_sequence(reference)?;
    issues
        .iter()
        .find(|i| i.sequence_id == seq && !has_bracketed_id(&i.name))
}

fn is_id_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '-'
}

/// True when `id` occurs in `name` delimited by anything other than letters, digits or `-`.
fn has_token(name: &str, id: &str) -> bool {
    let mut from = 0;
    while let Some(pos) = name[from..].find(id) {
        let start = from + pos;
        let end = start + id.len();
        let before_ok = name[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !is_id_char(c));
        let after_ok = name[end..].chars().next().is_none_or(|c| !is_id_char(c));
        if before_ok && after_ok {
            return true;
        }
        from = start + name[start..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// `DEN-324` -> 324. The identifier must start with a letter and continue with letters or digits.
fn ticket_sequence(reference: &str) -> Option<i64> {
    let (ident, num) = reference.rsplit_once('-')?;
    let mut chars = ident.chars();
    if !chars.next()?.is_ascii_alphabetic() || !chars.all(|c| c.is_ascii_alphanumeric()) {
        return None;
    }
    if num.is_empty() || !num.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    num.parse().ok()
}

/// True when the title starts with a `[ID-n]` prefix.
fn has_bracketed_id(name: &str) -> bool {
    let Some(rest) = name.trim_start().strip_prefix('[') else {
        return false;
    };
    rest.split_once(']')
        .is_some_and(|(inside, _)| ticket_sequence(inside).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, seq: i64, name: &str) -> Issue {
        Issue {
            id: id.into(),
            sequence_id: seq,
            name: name.into(),
            ..Issue::default()
        }
    }

    #[test]
    fn bracketed_prefix_wins_and_substrings_never_match() {
        let issues = vec![
            issue("u-1014", 300, "[DEN-1014] feat(agentjobs): worktrees"),
            issue("u-100", 301, "DEN-100 legacy name without brackets"),
            issue("u-10", 302, "[DEN-10] the real one"),
        ];
        assert_eq!(resolve_issue(&issues, "DEN-10").unwrap().id, "u-10");
        assert_eq!(resolve_issue(&issues, "DEN-100").unwrap().id, "u-100");
        assert_eq!(resolve_issue(&issues, "u-1014").unwrap().id, "u-1014");
        for bad in ["DEN-1", "DEN-101", "EN-10"] {
            assert!(
                resolve_issue(&issues, bad).is_none(),
                "{bad} matched by substring"
            );
        }
    }

    #[test]
    fn sequence_only_resolves_tickets_without_their_own_id() {
        let issues = vec![
            issue("u-a", 324, "Fresh ticket without id prefix"),
            issue("u-b", 10, "[DEN-1014] legacy id differs from sequence"),
        ];
        assert_eq!(resolve_issue(&issues, "DEN-324").unwrap().id, "u-a");
        assert_eq!(resolve_issue(&issues, "324").unwrap().id, "u-a");
        assert!(
            resolve_issue(&issues, "DEN-10").is_none(),
            "titled issue matched by sequence"
        );
        assert_eq!(
            resolve_issue(&issues, "10").unwrap().id,
            "u-b",
            "a bare number is always the sequence"
        );
    }

    #[test]
    fn empty_and_unknown_references_resolve_to_nothing() {
        let issues = vec![issue("u", 1, "[DEN-1] x")];
        assert!(resolve_issue(&issues, "  ").is_none());
        assert!(resolve_issue(&issues, "KAKEBO-9").is_none());
    }
}
