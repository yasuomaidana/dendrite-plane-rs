//! JSON-RPC 2.0 sidecar protocol for Plane. One JSON message per line on stdin and stdout; logs go
//! to stderr. The API token comes only from the process environment (`PLANE_API_KEY`) and never
//! travels over the wire.

use std::collections::HashMap;

mod routes;

use plane_client::{
    Client, Config, CreateIssue, Error, UpdateIssue, is_path_segment, loopback_origin,
};
use serde::Deserialize;
use serde_json::{Value, json};

fn parse<T: for<'de> Deserialize<'de>>(params: Value) -> Result<T, RpcError> {
    serde_json::from_value(params).map_err(|e| RpcError::params(e.to_string()))
}

/// Protocol version. Clients must reject a server whose major version differs from theirs.
pub const PROTOCOL: &str = "1.4.2";

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

/// Parameters of `project.archive` and `project.delete`.
#[derive(Deserialize)]
struct EndParams {
    #[serde(flatten)]
    target: Target,
    #[serde(default)]
    confirm: Option<bool>,
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

/// The origin a request may send the API key to. The key goes to whatever `base_url` names, so a
/// request may repeat the configured origin, or name a loopback one (a local stand-in) only when the
/// configured Plane is itself on loopback; any other host is refused. `None` keeps the configured one.
fn requested_origin(requested: Option<&str>, configured: &str) -> Result<Option<String>, RpcError> {
    let Some(url) = requested.filter(|u| !u.is_empty()) else {
        return Ok(None);
    };
    if url.trim_end_matches('/') == configured {
        return Ok(None);
    }
    if loopback_origin(configured).is_some()
        && let Some(origin) = loopback_origin(url)
    {
        return Ok(Some(origin));
    }
    Err(RpcError::params(
        "base_url may only be the configured PLANE_BASE_URL, or a loopback address when the configured one is loopback",
    ))
}

pub struct Server {
    base: Result<Config, Error>,
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

    fn target_config(&self, target: &Target) -> Result<Config, RpcError> {
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
        let origin = requested_origin(target.base_url.as_deref(), &base.base_url)?;
        if let Some(ws) = target.workspace.as_deref().filter(|w| !w.is_empty())
            && !is_path_segment(ws)
        {
            return Err(RpcError::params("workspace is not a valid slug"));
        }
        if !is_path_segment(&project) {
            return Err(RpcError::params("project_id is not a valid identifier"));
        }
        Ok(base.with_project(
            origin.as_deref(),
            target.workspace.as_deref().filter(|w| !w.is_empty()),
            &project,
        ))
    }

    fn client(&mut self, target: &Target) -> Result<&Client, RpcError> {
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
    ) -> Result<Value, RpcError> {
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
    ) -> Result<Value, RpcError> {
        let p: EndParams = parse(params)?;
        let t = p.target;
        // A destructive call names its project and says it means it; the default project is
        // never the victim of an empty request.
        if t.project_id.as_deref().is_none_or(str::is_empty) {
            return Err(RpcError::params("project_id is required for this method"));
        }
        if p.confirm != Some(true) {
            return Err(RpcError::params("confirm must be true for this method"));
        }
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

/// Compiles the Rust examples of the crate README as doctests, so they cannot drift from the code.
#[cfg(doctest)]
#[doc = include_str!("../README.md")]
struct ReadmeDoctests;
