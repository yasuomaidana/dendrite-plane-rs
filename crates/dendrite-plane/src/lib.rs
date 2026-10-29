//! JSON-RPC 2.0 sidecar protocol for Plane. One JSON message per line on stdin and stdout; logs go
//! to stderr. The API token comes only from the process environment (`PLANE_API_KEY`) and never
//! travels over the wire.

use std::collections::HashMap;

use plane_client::{Client, Config, CreateIssue, Error, ListFields, Status, UpdateIssue};
use serde::Deserialize;
use serde_json::{Value, json};

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> std::result::Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::params(e.to_string()))
}

/// Protocol version. Clients must reject a server whose major version differs from theirs.
pub const PROTOCOL: &str = "1.3.0";

const PARSE_ERROR: i64 = -32700;
const INVALID_REQUEST: i64 = -32600;
const METHOD_NOT_FOUND: i64 = -32601;
const INVALID_PARAMS: i64 = -32602;
const ENGINE_ERROR: i64 = -32000;

struct RpcError {
    code: i64,
    message: String,
    kind: Option<&'static str>,
}

impl RpcError {
    fn params(message: impl Into<String>) -> RpcError {
        RpcError {
            code: INVALID_PARAMS,
            message: message.into(),
            kind: None,
        }
    }
}

impl From<Error> for RpcError {
    fn from(e: Error) -> RpcError {
        RpcError {
            code: ENGINE_ERROR,
            message: e.to_string(),
            kind: Some(e.kind()),
        }
    }
}

/// Which project a call is about. Every field is optional: unset ones fall back to the
/// environment (`PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG`, `PLANE_PROJECT_ID`).
#[derive(Debug, Default, Deserialize)]
struct Target {
    base_url: Option<String>,
    workspace: Option<String>,
    project_id: Option<String>,
}

#[derive(Deserialize)]
struct Fields {
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    fields: Option<String>,
}

#[derive(Deserialize)]
struct RefParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
}

#[derive(Deserialize)]
struct CreateParams {
    #[serde(flatten)]
    target: Target,
    #[serde(flatten)]
    issue: CreateIssue,
}

#[derive(Deserialize)]
struct UpdateParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(flatten)]
    changes: UpdateIssue,
}

#[derive(Deserialize)]
struct LabelsPatchParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
    #[serde(default)]
    add: Vec<String>,
    #[serde(default)]
    remove_prefixes: Vec<String>,
}

#[derive(Deserialize)]
struct CommentParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
    comment: String,
}

#[derive(Deserialize)]
struct CommentUpdateParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
    comment_id: String,
    comment_html: String,
}

#[derive(Deserialize)]
struct SetStatusParams {
    #[serde(flatten)]
    target: Target,
    #[serde(rename = "ref")]
    reference: String,
    status: String,
}

#[derive(Deserialize)]
struct BatchParams {
    #[serde(flatten)]
    target: Target,
    issue_ids: Vec<String>,
    /// Bypass the relations cache.
    #[serde(default)]
    fresh: bool,
}

#[derive(Deserialize)]
struct SnapshotParams {
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    fresh: bool,
}

#[derive(Deserialize)]
struct InvalidateParams {
    #[serde(flatten)]
    target: Target,
    /// Issues whose relations changed; empty or absent clears the whole project cache.
    #[serde(default)]
    issue_ids: Vec<String>,
}

pub struct Server {
    base: std::result::Result<Config, Error>,
    relations_ttl: std::time::Duration,
    cache_dir: Option<std::path::PathBuf>,
    clients: HashMap<(String, String, String), Client>,
}

impl Server {
    /// Reads the environment once. A missing token is not fatal at startup: `health` still answers,
    /// and every Plane call reports the configuration error with its fix.
    pub fn from_env() -> Server {
        let ttl = std::env::var("DENDRITE_PLANE_RELATIONS_TTL_SECS")
            .ok()
            .and_then(|v| v.trim().parse::<u64>().ok())
            .map_or(
                plane_client::DEFAULT_RELATIONS_TTL,
                std::time::Duration::from_secs,
            );
        let cache_dir = std::env::var_os("DENDRITE_PLANE_CACHE_DIR")
            .filter(|v| !v.is_empty())
            .map(std::path::PathBuf::from);
        Server {
            base: Config::from_env(),
            relations_ttl: ttl,
            cache_dir,
            clients: HashMap::new(),
        }
    }

    pub fn with_config(cfg: Config) -> Server {
        Server {
            base: Ok(cfg),
            relations_ttl: plane_client::DEFAULT_RELATIONS_TTL,
            cache_dir: None,
            clients: HashMap::new(),
        }
    }

    fn target_config(&self, target: &Target) -> std::result::Result<Config, RpcError> {
        let base = self
            .base
            .as_ref()
            .map_err(|_| RpcError::from(Error::MissingToken))?;
        let project = target
            .project_id
            .clone()
            .filter(|p| !p.is_empty())
            .unwrap_or_else(|| base.project_id.clone());
        if project.is_empty() {
            return Err(RpcError::params(
                "project_id is required (or set PLANE_PROJECT_ID)",
            ));
        }
        Ok(base.with_project(
            target.base_url.as_deref(),
            target.workspace.as_deref(),
            &project,
        ))
    }

    fn client(&mut self, target: &Target) -> std::result::Result<&Client, RpcError> {
        let cfg = self.target_config(target)?;
        let key = (
            cfg.base_url.clone(),
            cfg.workspace.clone(),
            cfg.project_id.clone(),
        );
        let ttl = self.relations_ttl;
        let cache_file = self
            .cache_dir
            .as_ref()
            .map(|d| d.join(format!("plane-relations-{:016x}.json", fnv1a(&key))));
        Ok(self.clients.entry(key).or_insert_with(|| {
            let client = Client::new(cfg).with_relations_ttl(ttl);
            match cache_file {
                Some(path) => client.with_cache_file(path),
                None => client,
            }
        }))
    }

    /// Handles one line. Returns `None` for notifications (requests without an `id`).
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_response(
                    Value::Null,
                    &RpcError {
                        code: PARSE_ERROR,
                        message: e.to_string(),
                        kind: None,
                    },
                ));
            }
        };
        let id = req.get("id").cloned();
        let Some(method) = req.get("method").and_then(Value::as_str) else {
            return Some(error_response(
                id.unwrap_or(Value::Null),
                &RpcError {
                    code: INVALID_REQUEST,
                    message: "missing method".into(),
                    kind: None,
                },
            ));
        };
        let params = req.get("params").cloned().unwrap_or_else(|| json!({}));
        let result = self.dispatch(method, params);
        let id = id?;
        Some(match result {
            Ok(v) => json!({"jsonrpc": "2.0", "id": id, "result": v}).to_string(),
            Err(e) => error_response(id, &e),
        })
    }

    /// A call on one issue reference: parses the params, runs `call` and returns its result under `key`.
    fn on_issue<T: serde::Serialize>(
        &mut self,
        params: Value,
        key: &str,
        call: impl FnOnce(&Client, &str) -> plane_client::Result<T>,
    ) -> std::result::Result<Value, RpcError> {
        let p: RefParams = parse(params)?;
        let result = call(self.client(&p.target)?, &p.reference)?;
        let mut out = serde_json::Map::new();
        out.insert(key.to_string(), json!(result));
        Ok(Value::Object(out))
    }

    /// Ends a project (archive or delete): runs `end`, drops the cached relations and the cached client.
    fn end_project(
        &mut self,
        params: Value,
        end: impl FnOnce(&Client) -> plane_client::Result<()>,
    ) -> std::result::Result<Value, RpcError> {
        let t: Target = parse(params)?;
        let cfg = self.target_config(&t)?;
        let key = (cfg.base_url, cfg.workspace, cfg.project_id);
        {
            let client = self.client(&t)?;
            end(client)?;
            client.clear_relations_cache();
        }
        self.clients.remove(&key);
        Ok(json!({ "ok": true }))
    }

    fn dispatch(&mut self, method: &str, params: Value) -> std::result::Result<Value, RpcError> {
        match method {
            "health" => Ok(json!({
                "name": "dendrite-plane",
                "version": env!("CARGO_PKG_VERSION"),
                "protocol": PROTOCOL,
                "features": [],
                "configured": self.base.is_ok(),
            })),
            "board.snapshot" => {
                let p: SnapshotParams = parse(params)?;
                let snap = self.client(&p.target)?.snapshot(p.fresh)?;
                Ok(serde_json::to_value(snap).expect("snapshot serializes"))
            }
            "issues.list" => {
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
            "states.list" => {
                let t: Target = parse(params)?;
                Ok(json!({ "states": self.client(&t)?.list_states()? }))
            }
            "issue.resolve" => {
                let p: RefParams = parse(params)?;
                let issues = self.client(&p.target)?.list_issues(ListFields::Min)?;
                let issue = plane_client::resolve_issue(&issues, &p.reference)
                    .ok_or_else(|| Error::TicketNotFound(p.reference.clone()))?;
                Ok(json!({ "issue": issue }))
            }
            "issue.get" => self.on_issue(params, "issue", Client::get_issue),
            "issue.create" => {
                let p: CreateParams = parse(params)?;
                let (issue, created) = self.client(&p.target)?.create_issue(&p.issue)?;
                Ok(json!({ "issue": issue, "created": created }))
            }
            "issue.update" => {
                let mut p: UpdateParams = parse(params)?;
                let client = self.client(&p.target)?;
                if let Some(labels) = p.changes.labels {
                    let mut resolved = Vec::with_capacity(labels.len());
                    for l in &labels {
                        if is_uuid(l) {
                            resolved.push(l.clone());
                        } else {
                            let lbl = client.ensure_label(l, "#7C3AED")?;
                            resolved.push(lbl.id);
                        }
                    }
                    p.changes.labels = Some(resolved);
                }
                let issue = client.update_issue(&p.reference, &p.changes)?;
                Ok(json!({ "issue": issue }))
            }
            "issue.labels_patch" => {
                let p: LabelsPatchParams = parse(params)?;
                let (issue, labels) = self.client(&p.target)?.patch_labels(
                    &p.reference,
                    &p.add,
                    &p.remove_prefixes,
                )?;
                Ok(json!({ "issue": issue, "labels": labels }))
            }
            "user.me" => {
                let t: Target = parse(params)?;
                let user = self.client(&t)?.me()?;
                Ok(json!({ "user": user }))
            }
            "issue.comments" => self.on_issue(params, "comments", Client::list_comments),
            "issue.comment_update" => {
                let p: CommentUpdateParams = parse(params)?;
                self.client(&p.target)?.update_comment(
                    &p.reference,
                    &p.comment_id,
                    &p.comment_html,
                )?;
                Ok(json!({ "ok": true }))
            }
            "issue.comment" => {
                let p: CommentParams = parse(params)?;
                self.client(&p.target)?.comment(&p.reference, &p.comment)?;
                Ok(json!({ "ok": true }))
            }
            "issue.set_status" => {
                let p: SetStatusParams = parse(params)?;
                let status = Status::parse(&p.status)
                    .ok_or_else(|| RpcError::params(format!("unknown status {:?}", p.status)))?;
                let (issue, state) = self.client(&p.target)?.set_status(&p.reference, status)?;
                Ok(json!({ "issue": issue, "state": state, "status": status }))
            }
            "relations.batch" => {
                let p: BatchParams = parse(params)?;
                let b = self
                    .client(&p.target)?
                    .relations_batch(&p.issue_ids, p.fresh);
                Ok(
                    json!({ "relations": b.relations, "errors": b.failed.len(), "cached": b.cached, "fetched": b.fetched }),
                )
            }
            "cache.invalidate" => {
                let p: InvalidateParams = parse(params)?;
                let client = self.client(&p.target)?;
                if p.issue_ids.is_empty() {
                    client.clear_relations_cache();
                } else {
                    client.invalidate_relations(&p.issue_ids);
                }
                Ok(json!({ "ok": true }))
            }
            "project.archive" => self.end_project(params, Client::archive_project),
            "project.delete" => self.end_project(params, Client::delete_project),
            other => Err(RpcError {
                code: METHOD_NOT_FOUND,
                message: format!("unknown method {other:?}"),
                kind: None,
            }),
        }
    }
}

fn error_response(id: Value, e: &RpcError) -> String {
    let mut err = json!({"code": e.code, "message": e.message});
    if let Some(kind) = e.kind {
        err["data"] = json!({"kind": kind});
    }
    json!({"jsonrpc": "2.0", "id": id, "error": err}).to_string()
}

/// FNV-1a, enough to name a per-project cache file without pulling in a hashing crate.
fn fnv1a(key: &(String, String, String)) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for part in [&key.0, &key.1, &key.2] {
        for byte in part.bytes().chain(std::iter::once(0)) {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    hash
}

fn is_uuid(s: &str) -> bool {
    let r = s.trim();
    if r.len() != 36 || r.matches('-').count() != 4 {
        return false;
    }
    let parts: Vec<&str> = r.split('-').collect();
    if parts.len() != 5
        || parts[0].len() != 8
        || parts[1].len() != 4
        || parts[2].len() != 4
        || parts[3].len() != 4
        || parts[4].len() != 12
    {
        return false;
    }
    r.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}
