use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::client::{Client, ListFields};
use crate::error::{Error, Result};
use crate::types::{Issue, Relations, Status, status_for};

/// Relation requests in flight at once. Plane answers each in about 35ms, so a serial sweep of a
/// few hundred issues takes many seconds while sixteen workers finish in about one.
pub const RELATION_WORKERS: usize = 16;

/// How long a fetched relation set is trusted. Plane does not bump an issue's `updated_at` when a
/// relation is added or removed, so freshness cannot be detected cheaply; a short TTL plus
/// invalidation on our own writes keeps dependencies current without re-fetching hundreds of
/// issues on every call.
pub const DEFAULT_RELATIONS_TTL: Duration = Duration::from_secs(60);

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
pub(crate) struct CacheEntry {
    pub(crate) fetched_ms: u64,
    pub(crate) relations: Relations,
}

#[derive(Default)]
pub(crate) struct RelationCache {
    pub(crate) entries: HashMap<String, CacheEntry>,
    /// Modification time of the cache file when it was last read or written by this process.
    pub(crate) seen_mtime: Option<SystemTime>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

impl Client {
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

    pub(crate) fn file_mtime(&self) -> Option<SystemTime> {
        std::fs::metadata(self.cache_file.as_ref()?)
            .and_then(|m| m.modified())
            .ok()
    }

    /// Replaces the in-memory entries with the file's when another process rewrote it.
    pub(crate) fn reload_if_changed(&self, cache: &mut RelationCache) {
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
    pub(crate) fn save(&self, cache: &mut RelationCache) {
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
}
