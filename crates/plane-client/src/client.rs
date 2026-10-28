use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::resolve::resolve_issue;
use crate::types::{Issue, Relations, State, Status, status_for};

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

pub struct Client {
    cfg: Config,
    agent: ureq::Agent,
    relations_ttl: Duration,
    cache: Mutex<HashMap<String, (Instant, Relations)>>,
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
            cache: Mutex::new(HashMap::new()),
        }
    }

    /// Overrides how long cached relations are trusted (zero disables the cache).
    pub fn with_relations_ttl(mut self, ttl: Duration) -> Client {
        self.relations_ttl = ttl;
        self
    }

    /// Forgets cached relations for these issues; call it after changing their relations.
    pub fn invalidate_relations(&self, issue_ids: &[String]) {
        let mut cache = self.cache.lock().unwrap();
        for id in issue_ids {
            cache.remove(id);
        }
    }

    pub fn clear_relations_cache(&self) {
        self.cache.lock().unwrap().clear();
    }

    pub fn config(&self) -> &Config {
        &self.cfg
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
            let cache = self.cache.lock().unwrap();
            for id in issue_ids {
                match cache.get(id) {
                    Some((at, rel)) if !fresh && at.elapsed() < self.relations_ttl => {
                        out.relations.insert(id.clone(), rel.clone());
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
            let now = Instant::now();
            let mut cache = self.cache.lock().unwrap();
            for (id, rel) in &fetched {
                cache.insert(id.clone(), (now, rel.clone()));
            }
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
        let mut resp = self
            .agent
            .patch(&url)
            .header("x-api-key", &self.cfg.api_key)
            .header("Accept", "application/json")
            .send_json(serde_json::json!({ "state": state.id }))?;
        Self::check(resp.status().as_u16(), "set ticket state")?;
        let _ = resp.body_mut().read_to_string();
        Ok((issue, state))
    }
}
