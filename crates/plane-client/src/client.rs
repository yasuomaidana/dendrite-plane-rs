use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::resolve::resolve_issue;
use crate::types::{
    CreateIssue, Issue, Relations, State, Status, UpdateIssue, is_empty_context, status_for,
};

/// Relation requests in flight at once. Plane answers each in about 35ms, so a serial sweep of a
/// few hundred issues takes many seconds while sixteen workers finish in about one.
pub const RELATION_WORKERS: usize = 16;

/// How long a fetched relation set is trusted. Plane does not bump an issue's `updated_at` when a
/// relation is added or removed, so freshness cannot be detected cheaply; a short TTL plus
/// invalidation on our own writes keeps dependencies current without re-fetching hundreds of
/// issues on every call.
pub const DEFAULT_RELATIONS_TTL: Duration = Duration::from_secs(60);

/// The fields a list call needs for scheduling; leaves out the large description payloads.
const MIN_FIELDS: &str = "id,name,sequence_id,state,priority,estimate_point,start_date,target_date,updated_at,external_id";

#[derive(Debug, Clone)]
pub struct Config {
    pub base_url: String,
    pub workspace: String,
    pub project_id: String,
    api_key: String,
}

impl Config {
    pub fn new(base_url: &str, workspace: &str, project_id: &str, api_key: &str) -> Result<Config> {
        if api_key.trim().is_empty() {
            return Err(Error::MissingToken);
        }
        Ok(Config {
            base_url: base_url.trim_end_matches('/').to_string(),
            workspace: workspace.to_string(),
            project_id: project_id.to_string(),
            api_key: api_key.trim().to_string(),
        })
    }

    /// Reads `PLANE_API_KEY` (required, no default), `PLANE_BASE_URL` (default
    /// `http://localhost:$PLANE_API_PORT|8090`), `PLANE_WORKSPACE_SLUG` (default `dendrite-ws`) and
    /// `PLANE_PROJECT_ID`.
    pub fn from_env() -> Result<Config> {
        let key = std::env::var("PLANE_API_KEY").unwrap_or_default();
        let base = std::env::var("PLANE_BASE_URL")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                let port = std::env::var("PLANE_API_PORT")
                    .ok()
                    .filter(|s| !s.is_empty())
                    .unwrap_or_else(|| "8090".into());
                format!("http://localhost:{port}")
            });
        let ws = std::env::var("PLANE_WORKSPACE_SLUG")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "dendrite-ws".into());
        let project = std::env::var("PLANE_PROJECT_ID").unwrap_or_default();
        Config::new(&base, &ws, &project, &key)
    }

    /// The same Plane, another project; the token stays private to this crate.
    pub fn with_project(
        &self,
        base_url: Option<&str>,
        workspace: Option<&str>,
        project_id: &str,
    ) -> Config {
        Config {
            base_url: base_url.map_or_else(
                || self.base_url.clone(),
                |b| b.trim_end_matches('/').to_string(),
            ),
            workspace: workspace.map_or_else(|| self.workspace.clone(), str::to_string),
            project_id: project_id.to_string(),
            api_key: self.api_key.clone(),
        }
    }
}

/// How much of each issue a list call fetches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ListFields {
    /// Scheduling fields only (about a third of the bytes).
    #[default]
    Min,
    /// Everything Plane returns, including descriptions.
    Full,
}

/// Everything a scheduler needs from the board, fetched in one call.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Snapshot {
    /// Relation sets served from the cache and fetched from Plane for this snapshot.
    pub relations_cached: usize,
    pub relations_fetched: usize,
    pub issues: Vec<Issue>,
    /// Status of each issue, keyed by issue UUID.
    pub statuses: HashMap<String, Status>,
    /// Relations of every issue that is not Done or Cancelled, keyed by issue UUID.
    pub relations: HashMap<String, Relations>,
    /// How many relation lookups failed (those issues are simply absent from `relations`).
    pub relation_errors: usize,
}

/// Outcome of [`Client::relations_batch`].
#[derive(Debug, Default)]
pub struct RelationsBatch {
    pub relations: HashMap<String, Relations>,
    pub failed: Vec<(String, Error)>,
    pub cached: usize,
    pub fetched: usize,
}

/// One cached relation set and when it was fetched (milliseconds since the Unix epoch, so the
/// age survives across processes).
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheEntry {
    fetched_ms: u64,
    relations: Relations,
}

#[derive(Default)]
struct RelationCache {
    entries: HashMap<String, CacheEntry>,
    /// Modification time of the cache file when it was last read or written by this process.
    seen_mtime: Option<SystemTime>,
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

pub struct Client {
    cfg: Config,
    agent: ureq::Agent,
    relations_ttl: Duration,
    cache: Mutex<RelationCache>,
    /// Where the cache is shared between processes (short-lived CLI runs each start a sidecar).
    cache_file: Option<PathBuf>,
}

#[derive(Deserialize)]
struct Page {
    #[serde(default)]
    results: Vec<Issue>,
    #[serde(default)]
    next_page_results: bool,
    #[serde(default)]
    next_cursor: String,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum States {
    Paged { results: Vec<State> },
    Bare(Vec<State>),
}

impl Client {
    pub fn new(cfg: Config) -> Client {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        Client {
            cfg,
            agent,
            relations_ttl: DEFAULT_RELATIONS_TTL,
            cache: Mutex::new(RelationCache::default()),
            cache_file: None,
        }
    }

    /// Overrides how long cached relations are trusted (zero disables the cache).
    pub fn with_relations_ttl(mut self, ttl: Duration) -> Client {
        self.relations_ttl = ttl;
        self
    }

    /// Shares the relation cache between processes through a file. The file is read when another
    /// process changed it and rewritten atomically after every change here.
    pub fn with_cache_file(mut self, path: PathBuf) -> Client {
        self.cache_file = Some(path);
        self
    }

    /// Forgets cached relations for these issues; call it after changing their relations.
    pub fn invalidate_relations(&self, issue_ids: &[String]) {
        let mut cache = self.cache.lock().unwrap();
        self.reload_if_changed(&mut cache);
        for id in issue_ids {
            cache.entries.remove(id);
        }
        self.save(&mut cache);
    }

    pub fn clear_relations_cache(&self) {
        let mut cache = self.cache.lock().unwrap();
        cache.entries.clear();
        self.save(&mut cache);
    }

    fn file_mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(self.cache_file.as_ref()?)
            .and_then(|m| m.modified())
            .ok()
    }

    /// Replaces the in-memory entries with the file's when another process rewrote it.
    fn reload_if_changed(&self, cache: &mut RelationCache) {
        let Some(path) = &self.cache_file else { return };
        let mtime = self.file_mtime();
        if mtime.is_none() || mtime == cache.seen_mtime {
            return;
        }
        if let Ok(bytes) = std::fs::read(path)
            && let Ok(entries) = serde_json::from_slice::<HashMap<String, CacheEntry>>(&bytes)
        {
            cache.entries = entries;
        }
        cache.seen_mtime = mtime;
    }

    /// Writes the cache atomically (temporary file, then rename). Failures only cost sharing.
    fn save(&self, cache: &mut RelationCache) {
        let Some(path) = &self.cache_file else { return };
        let ttl_ms = (self.relations_ttl.as_millis() as u64).max(1);
        let now = now_ms();
        cache
            .entries
            .retain(|_, e| now.saturating_sub(e.fetched_ms) < ttl_ms);
        let Ok(bytes) = serde_json::to_vec(&cache.entries) else {
            return;
        };
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        if std::fs::write(&tmp, bytes).is_ok() && std::fs::rename(&tmp, path).is_ok() {
            cache.seen_mtime = self.file_mtime();
        } else {
            let _ = std::fs::remove_file(&tmp);
        }
    }

    fn project_url(&self, public_api: bool, tail: &str) -> String {
        let api = if public_api { "api/v1" } else { "api" };
        format!(
            "{}/{api}/workspaces/{}/projects/{}/{tail}",
            self.cfg.base_url, self.cfg.workspace, self.cfg.project_id
        )
    }

    fn check(status: u16, what: &str) -> Result<()> {
        match status {
            401 | 403 => Err(Error::Unauthorized(status)),
            s if s >= 400 => Err(Error::Http {
                status: s,
                what: what.to_string(),
            }),
            _ => Ok(()),
        }
    }

    fn get<T: DeserializeOwned>(&self, url: &str, what: &str) -> Result<T> {
        let mut resp = self
            .agent
            .get(url)
            .header("x-api-key", &self.cfg.api_key)
            .header("Accept", "application/json")
            .call()?;
        Self::check(resp.status().as_u16(), what)?;
        resp.body_mut()
            .read_json()
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// Every issue of the project, following Plane's cursor pagination.
    pub fn list_issues(&self, fields: ListFields) -> Result<Vec<Issue>> {
        let mut out = Vec::new();
        let mut seen = std::collections::HashSet::new();
        let mut cursor = String::new();
        loop {
            let mut tail = String::from("issues/?per_page=100");
            if fields == ListFields::Min {
                tail.push_str("&fields=");
                tail.push_str(MIN_FIELDS);
            }
            if !cursor.is_empty() {
                tail.push_str("&cursor=");
                tail.push_str(&cursor);
            }
            let page: Page = self.get(&self.project_url(true, &tail), "list issues")?;
            for issue in page.results {
                // Plane repeats an issue once per module it belongs to; keep the first row.
                if seen.insert(issue.id.clone()) {
                    out.push(issue);
                }
            }
            if !page.next_page_results || page.next_cursor.is_empty() {
                return Ok(out);
            }
            cursor = page.next_cursor;
        }
    }

    pub fn list_states(&self) -> Result<Vec<State>> {
        let states: States = self.get(&self.project_url(true, "states/"), "list states")?;
        Ok(match states {
            States::Paged { results } => results,
            States::Bare(list) => list,
        })
    }

    /// Relations of one issue. Plane's public v1 API has no relations route (it answers 404), so this
    /// uses the internal `/api/workspaces/...` path, which accepts the same API key.
    pub fn relations(&self, issue_id: &str) -> Result<Relations> {
        self.get(
            &self.project_url(false, &format!("issues/{issue_id}/issue-relation/")),
            "issue relations",
        )
    }

    /// Relations for many issues. Entries younger than the TTL come from the cache unless `fresh` is
    /// set; the rest are fetched by [`RELATION_WORKERS`] threads. Failures are reported next to the
    /// successes instead of aborting the batch.
    pub fn relations_batch(&self, issue_ids: &[String], fresh: bool) -> RelationsBatch {
        let mut out = RelationsBatch::default();
        let mut missing: Vec<String> = Vec::new();
        {
            let mut cache = self.cache.lock().unwrap();
            self.reload_if_changed(&mut cache);
            let (now, ttl_ms) = (now_ms(), self.relations_ttl.as_millis() as u64);
            for id in issue_ids {
                match cache.entries.get(id) {
                    Some(e) if !fresh && now.saturating_sub(e.fetched_ms) < ttl_ms => {
                        out.relations.insert(id.clone(), e.relations.clone());
                        out.cached += 1;
                    }
                    _ => missing.push(id.clone()),
                }
            }
        }
        if missing.is_empty() {
            return out;
        }

        let next = AtomicUsize::new(0);
        let ok = Mutex::new(HashMap::with_capacity(missing.len()));
        let failed = Mutex::new(Vec::new());
        let workers = RELATION_WORKERS.min(missing.len());
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| {
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(id) = missing.get(i) else { return };
                        match self.relations(id) {
                            Ok(r) => {
                                ok.lock().unwrap().insert(id.clone(), r);
                            }
                            Err(e) => failed.lock().unwrap().push((id.clone(), e)),
                        }
                    }
                });
            }
        });
        let fetched = ok.into_inner().unwrap();
        out.fetched = fetched.len();
        {
            let now = now_ms();
            let mut cache = self.cache.lock().unwrap();
            // Another process may have cached other issues meanwhile; keep them.
            self.reload_if_changed(&mut cache);
            for (id, rel) in &fetched {
                cache.entries.insert(
                    id.clone(),
                    CacheEntry {
                        fetched_ms: now,
                        relations: rel.clone(),
                    },
                );
            }
            self.save(&mut cache);
        }
        out.relations.extend(fetched);
        out.failed = failed.into_inner().unwrap();
        out
    }

    /// The whole board in one call: issues, their statuses, and relations for the open ones.
    pub fn snapshot(&self, fresh: bool) -> Result<Snapshot> {
        let issues = self.list_issues(ListFields::Min)?;
        let by_state: HashMap<String, Status> = self
            .list_states()?
            .into_iter()
            .map(|s| (s.id.clone(), status_for(&s.group, &s.name)))
            .collect();
        let statuses: HashMap<String, Status> = issues
            .iter()
            .map(|i| {
                (
                    i.id.clone(),
                    by_state.get(&i.state).copied().unwrap_or(Status::Backlog),
                )
            })
            .collect();
        let open: Vec<String> = issues
            .iter()
            .filter(|i| !statuses[&i.id].is_closed())
            .map(|i| i.id.clone())
            .collect();
        let batch = self.relations_batch(&open, fresh);
        Ok(Snapshot {
            issues,
            statuses,
            relations: batch.relations,
            relation_errors: batch.failed.len(),
            relations_cached: batch.cached,
            relations_fetched: batch.fetched,
        })
    }

    /// Moves a ticket to the workflow state that maps to `status`. The ticket may be `DEN-n`, its
    /// Plane number or its UUID; it is resolved against one issue list, never two.
    pub fn set_status(&self, reference: &str, status: Status) -> Result<(Issue, State)> {
        let issues = self.list_issues(ListFields::Min)?;
        let issue = resolve_issue(&issues, reference)
            .ok_or_else(|| Error::TicketNotFound(reference.to_string()))?
            .clone();
        let state = self
            .list_states()?
            .into_iter()
            .find(|s| status_for(&s.group, &s.name) == status)
            .ok_or_else(|| Error::StateNotFound(status.display_name().to_string()))?;
        let url = self.project_url(true, &format!("issues/{}/", issue.id));
        let (status, _) = self.send("PATCH", &url, serde_json::json!({ "state": state.id }))?;
        Self::check(status, "set ticket state")?;
        Ok((issue, state))
    }

    /// POST or PATCH a JSON body and return Plane's status with the raw reply body, so callers can
    /// map 404 and 409 themselves before [`Client::check`] turns the rest into errors.
    fn send(&self, method: &str, url: &str, body: serde_json::Value) -> Result<(u16, String)> {
        let req = match method {
            "PATCH" => self.agent.patch(url),
            _ => self.agent.post(url),
        };
        let mut resp = req
            .header("x-api-key", &self.cfg.api_key)
            .header("Accept", "application/json")
            .send_json(body)?;
        let status = resp.status().as_u16();
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        Ok((status, text))
    }

    fn decode<T: DeserializeOwned>(text: &str) -> Result<T> {
        serde_json::from_str(text).map_err(|e| Error::Decode(e.to_string()))
    }

    /// A UUID is used as is; anything else (`DEN-n`, a Plane number) is resolved against one issue
    /// list, like [`Client::set_status`].
    fn issue_id(&self, reference: &str) -> Result<String> {
        let r = reference.trim();
        if r.len() == 36 && r.matches('-').count() == 4 {
            return Ok(r.to_string());
        }
        let issues = self.list_issues(ListFields::Min)?;
        resolve_issue(&issues, r)
            .map(|i| i.id.clone())
            .ok_or_else(|| Error::TicketNotFound(reference.to_string()))
    }

    /// One issue with everything Plane returns for it (description, labels, state detail).
    pub fn get_issue(&self, reference: &str) -> Result<Issue> {
        let id = self.issue_id(reference)?;
        let url = self.project_url(true, &format!("issues/{id}/"));
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
        Self::check(status, "get issue")?;
        resp.body_mut()
            .read_json()
            .map_err(|e| Error::Decode(e.to_string()))
    }

    /// Creates an issue. With `external_id` set the call is idempotent: an issue already carrying
    /// that id is returned with `created == false` and nothing is posted. Plane's 409 for a
    /// duplicate external id is treated the same way. Module and blocker links are not made here.
    pub fn create_issue(&self, req: &CreateIssue) -> Result<(Issue, bool)> {
        if req.name.trim().is_empty() {
            return Err(Error::Invalid("ticket name cannot be empty".into()));
        }
        if is_empty_context(&req.description_html) {
            return Err(Error::Invalid(
                "ticket context/description cannot be empty".into(),
            ));
        }
        let external = req.external_id.as_deref().filter(|e| !e.is_empty());
        if let Some(ext) = external
            && let Some(found) = self.find_by_external_id(ext)?
        {
            return Ok((found, false));
        }
        let mut body = serde_json::Map::new();
        body.insert("name".into(), req.name.clone().into());
        body.insert(
            "description_html".into(),
            req.description_html.clone().into(),
        );
        put_text(&mut body, "state", &req.state);
        put_text(&mut body, "priority", &req.priority);
        put_estimate(&mut body, req.estimate_point);
        put_dates(&mut body, &req.start_date, &req.target_date, &req.parent);
        put_text(&mut body, "external_id", &req.external_id);
        put_text(&mut body, "external_source", &req.external_source);
        if !req.labels.is_empty() {
            body.insert("labels".into(), req.labels.clone().into());
        }
        let url = self.project_url(true, "issues/");
        let (status, text) = self.send("POST", &url, body.into())?;
        if status == 409
            && let Some(ext) = external
            && let Some(found) = self.find_by_external_id(ext)?
        {
            return Ok((found, false));
        }
        Self::check(status, "create issue")?;
        Ok((Self::decode(&text)?, true))
    }

    fn find_by_external_id(&self, external_id: &str) -> Result<Option<Issue>> {
        Ok(self
            .list_issues(ListFields::Min)?
            .into_iter()
            .find(|i| i.external_id.as_deref() == Some(external_id)))
    }

    /// Changes only the fields that are set (an empty string counts as unset, like the Go client).
    pub fn update_issue(&self, reference: &str, req: &UpdateIssue) -> Result<Issue> {
        let id = self.issue_id(reference)?;
        let mut body = serde_json::Map::new();
        put_text(&mut body, "name", &req.name);
        put_text(&mut body, "description_html", &req.description_html);
        put_text(&mut body, "state", &req.state);
        put_text(&mut body, "priority", &req.priority);
        put_estimate(&mut body, req.estimate_point);
        put_dates(&mut body, &req.start_date, &req.target_date, &req.parent);
        let url = self.project_url(true, &format!("issues/{id}/"));
        let (status, text) = self.send("PATCH", &url, body.into())?;
        if status == 404 {
            return Err(Error::TicketNotFound(reference.to_string()));
        }
        Self::check(status, "update issue")?;
        Self::decode(&text)
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

    /// Archives the project. Plane answers 204 No Content.
    pub fn archive_project(&self) -> Result<()> {
        let url = self.project_url(true, "archive/");
        let (status, _) = self.send("POST", &url, serde_json::json!({}))?;
        if status == 404 {
            return Err(Error::ProjectNotFound(self.cfg.project_id.clone()));
        }
        Self::check(status, "archive project")
    }

    /// Deletes the project permanently. Plane answers 204 No Content.
    pub fn delete_project(&self) -> Result<()> {
        let url = self.project_url(true, "");
        let resp = self
            .agent
            .delete(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("Accept", "application/json")
            .call()?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Err(Error::ProjectNotFound(self.cfg.project_id.clone()));
        }
        Self::check(status, "delete project")
    }
}

fn put_text(body: &mut serde_json::Map<String, serde_json::Value>, key: &str, v: &Option<String>) {
    if let Some(s) = v.as_deref().filter(|s| !s.is_empty()) {
        body.insert(key.into(), s.into());
    }
}

/// Plane's estimate scale is 0..=12; out-of-range values are clamped, as in Go.
fn put_estimate(body: &mut serde_json::Map<String, serde_json::Value>, v: Option<i64>) {
    if let Some(n) = v {
        body.insert("estimate_point".into(), n.clamp(0, 12).into());
    }
}

/// Dates and parent are sent whenever set, even empty, so a caller can clear them like in Go.
fn put_dates(
    body: &mut serde_json::Map<String, serde_json::Value>,
    start: &Option<String>,
    target: &Option<String>,
    parent: &Option<String>,
) {
    for (key, v) in [
        ("start_date", start),
        ("target_date", target),
        ("parent", parent),
    ] {
        if let Some(s) = v {
            body.insert(key.into(), s.clone().into());
        }
    }
}
