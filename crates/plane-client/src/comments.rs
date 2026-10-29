use serde::Deserialize;

use crate::client::Client;
use crate::error::{Error, Result};
use crate::types::{Comment, User};

#[derive(Deserialize)]
#[serde(untagged)]
enum CommentsResponse {
    Paged { results: Vec<Comment> },
    Bare(Vec<Comment>),
}

impl Client {
    /// Returns the currently authenticated user.
    pub fn me(&self) -> Result<User> {
        let url = format!("{}/api/v1/users/me/", self.cfg.base_url);
        self.get(&url, "get current user")
    }

    /// Adds a comment, wrapped in `<p>` exactly as the Go client does (the text is not escaped).
    pub fn comment(&self, reference: &str, comment: &str) -> Result<()> {
        if comment.trim().is_empty() {
            return Err(Error::Invalid("comment cannot be empty".into()));
        }
        let id = self.issue_id(reference)?;
        let url = self.project_url(true, &format!("issues/{id}/comments/"));
        let (status, _) = self.send(
            "POST",
            &url,
            serde_json::json!({ "comment_html": format!("<p>{comment}</p>") }),
        )?;
        if status == 404 {
            return Err(Error::TicketNotFound(reference.to_string()));
        }
        Self::check(status, "add comment")
    }

    /// Lists comments on an issue, oldest first.
    pub fn list_comments(&self, reference: &str) -> Result<Vec<Comment>> {
        let r = reference.trim();
        if r.is_empty() {
            return Err(Error::Invalid("ticket reference cannot be empty".into()));
        }
        let id = self.issue_id(r)?;
        let url = self.project_url(true, &format!("issues/{id}/comments/"));
        let mut resp = self
            .agent
            .get(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("Accept", "application/json")
            .call()?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Err(Error::TicketNotFound(reference.to_string()));
        }
        Self::check(status, "list comments")?;
        let body: CommentsResponse = resp
            .body_mut()
            .read_json()
            .map_err(|e| Error::Decode(e.to_string()))?;
        let mut comments = match body {
            CommentsResponse::Paged { results } => results,
            CommentsResponse::Bare(list) => list,
        };
        comments.sort_by(|a, b| a.created_at.cmp(&b.created_at));
        Ok(comments)
    }

    /// Updates a comment, sending `comment_html` as-is (no `<p>` wrapping).
    pub fn update_comment(
        &self,
        reference: &str,
        comment_id: &str,
        comment_html: &str,
    ) -> Result<()> {
        let r = reference.trim();
        if r.is_empty() {
            return Err(Error::Invalid("ticket reference cannot be empty".into()));
        }
        let cid = comment_id.trim();
        if cid.is_empty() {
            return Err(Error::Invalid("comment id cannot be empty".into()));
        }
        if comment_html.trim().is_empty() {
            return Err(Error::Invalid("comment cannot be empty".into()));
        }
        let id = self.issue_id(r)?;
        let url = self.project_url(true, &format!("issues/{id}/comments/{cid}/"));
        let (status, _) = self.send(
            "PATCH",
            &url,
            serde_json::json!({ "comment_html": comment_html }),
        )?;
        if status == 404 {
            if self.get_issue(&id).is_err() {
                return Err(Error::TicketNotFound(reference.to_string()));
            }
            return Err(Error::CommentNotFound(comment_id.to_string()));
        }
        Self::check(status, "update comment")
    }
}
