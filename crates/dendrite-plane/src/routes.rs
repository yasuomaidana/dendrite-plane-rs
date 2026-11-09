//! The JSON-RPC method table: one small handler per method, grouped so no router grows large.

use plane_client::{Client, Error, ListFields, Status, is_uuid};
use serde_json::{Value, json};

use super::{
    BatchParams, CommentParams, CommentUpdateParams, CreateParams, Fields, InvalidateParams,
    LabelsPatchParams, METHOD_NOT_FOUND, PROTOCOL, RefParams, RpcError, Server, SetStatusParams,
    SnapshotParams, Target, UpdateParams, parse,
};

const NEW_LABEL_COLOR: &str = "#7C3AED";

impl Server {
    pub(super) fn dispatch(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "health" => Ok(self.health()),
            _ => self.dispatch_read(method, params),
        }
    }

    fn dispatch_read(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "board.snapshot" => self.snapshot(params),
            "issues.list" => self.issues_list(params),
            "states.list" => self.states_list(params),
            "issue.resolve" => self.issue_resolve(params),
            "issue.get" => self.on_issue(params, "issue", Client::get_issue),
            "issue.comments" => self.on_issue(params, "comments", Client::list_comments),
            "user.me" => self.user_me(params),
            _ => self.dispatch_write(method, params),
        }
    }

    fn dispatch_write(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "issue.create" => self.issue_create(params),
            "issue.update" => self.issue_update(params),
            "issue.labels_patch" => self.labels_patch(params),
            "issue.comment_update" => self.comment_update(params),
            "issue.comment" => self.comment(params),
            "issue.set_status" => self.set_status(params),
            _ => self.dispatch_project(method, params),
        }
    }

    fn dispatch_project(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        match method {
            "relations.batch" => self.relations_batch(params),
            "cache.invalidate" => self.cache_invalidate(params),
            "project.archive" => self.end_project(params, Client::archive_project),
            "project.delete" => self.end_project(params, Client::delete_project),
            other => Err(RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("unknown method {other:?}"),
                kind: None,
            }),
        }
    }

    fn health(&self) -> Value {
        json!({
            "name": "dendrite-plane",
            "version": env!("CARGO_PKG_VERSION"),
            "protocol": PROTOCOL,
            "features": [],
            "configured": self.base.is_ok(),
        })
    }

    fn snapshot(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: SnapshotParams = parse(params)?;
        let snap = self.client(&p.target)?.snapshot(p.fresh)?;
        Ok(serde_json::to_value(snap).expect("snapshot serializes"))
    }

    fn issues_list(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: Fields = parse(params)?;
        let fields = match p.fields.as_deref() {
            None | Some("min") => ListFields::Min,
            Some("full") => ListFields::Full,
            Some(other) => {
                return Err(RpcError::params(format!(
                    "fields must be \"min\" or \"full\", got {other:?}"
                )));
            }
        };
        let issues = self.client(&p.target)?.list_issues(fields)?;
        Ok(json!({ "issues": issues }))
    }

    fn states_list(&mut self, params: Value) -> Result<Value, RpcError> {
        let t: Target = parse(params)?;
        Ok(json!({ "states": self.client(&t)?.list_states()? }))
    }

    fn issue_resolve(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: RefParams = parse(params)?;
        let issues = self.client(&p.target)?.list_issues(ListFields::Min)?;
        let issue = plane_client::resolve_issue(&issues, &p.reference)
            .ok_or_else(|| Error::TicketNotFound(p.reference.clone()))?;
        Ok(json!({ "issue": issue }))
    }

    fn user_me(&mut self, params: Value) -> Result<Value, RpcError> {
        let t: Target = parse(params)?;
        Ok(json!({ "user": self.client(&t)?.me()? }))
    }

    fn issue_create(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: CreateParams = parse(params)?;
        let (issue, created) = self.client(&p.target)?.create_issue(&p.issue)?;
        Ok(json!({ "issue": issue, "created": created }))
    }

    fn issue_update(&mut self, params: Value) -> Result<Value, RpcError> {
        let mut p: UpdateParams = parse(params)?;
        let client = self.client(&p.target)?;
        if let Some(labels) = p.changes.labels.take() {
            p.changes.labels = Some(label_ids(client, &labels)?);
        }
        let issue = client.update_issue(&p.reference, &p.changes)?;
        Ok(json!({ "issue": issue }))
    }

    fn labels_patch(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: LabelsPatchParams = parse(params)?;
        let (issue, labels) =
            self.client(&p.target)?
                .patch_labels(&p.reference, &p.add, &p.remove_prefixes)?;
        Ok(json!({ "issue": issue, "labels": labels }))
    }

    fn comment_update(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: CommentUpdateParams = parse(params)?;
        self.client(&p.target)?
            .update_comment(&p.reference, &p.comment_id, &p.comment_html)?;
        Ok(json!({ "ok": true }))
    }

    fn comment(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: CommentParams = parse(params)?;
        self.client(&p.target)?.comment(&p.reference, &p.comment)?;
        Ok(json!({ "ok": true }))
    }

    fn set_status(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: SetStatusParams = parse(params)?;
        let status = Status::parse(&p.status)
            .ok_or_else(|| RpcError::params(format!("unknown status {:?}", p.status)))?;
        let (issue, state) = self.client(&p.target)?.set_status(&p.reference, status)?;
        Ok(json!({ "issue": issue, "state": state, "status": status }))
    }

    fn relations_batch(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: BatchParams = parse(params)?;
        let b = self
            .client(&p.target)?
            .relations_batch(&p.issue_ids, p.fresh);
        Ok(
            json!({ "relations": b.relations, "errors": b.failed.len(), "cached": b.cached, "fetched": b.fetched }),
        )
    }

    fn cache_invalidate(&mut self, params: Value) -> Result<Value, RpcError> {
        let p: InvalidateParams = parse(params)?;
        let client = self.client(&p.target)?;
        if p.issue_ids.is_empty() {
            client.clear_relations_cache();
        } else {
            client.invalidate_relations(&p.issue_ids);
        }
        Ok(json!({ "ok": true }))
    }
}

/// Label ids for `labels`: UUIDs pass through, names are looked up or created.
fn label_ids(client: &Client, labels: &[String]) -> Result<Vec<String>, RpcError> {
    labels
        .iter()
        .map(|l| {
            if is_uuid(l) {
                Ok(l.clone())
            } else {
                Ok(client.ensure_label(l, NEW_LABEL_COLOR)?.id)
            }
        })
        .collect()
}
