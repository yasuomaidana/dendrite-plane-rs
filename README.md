# dendrite-plane-rs

Fast, long-lived access to [Plane](https://plane.so) for Dendrite. Two crates:

- **`plane-client`**: a typed Plane API client. List calls use `per_page=100` and a field projection (about a third of the bytes), rows Plane repeats for each module an issue belongs to are dropped, relations are fetched by 16 worker threads and cached, and finished tickets (Done, Cancelled) are never asked for relations.
- **`dendrite-plane`**: a JSON-RPC 2.0 sidecar over stdio (one message per line, logs on stderr), the same shape as `dendrite-rag`. Dendrite keeps one process alive instead of starting one per call.

## Why this exists

Selecting the next ticket on a 369-issue board took 28 s. The issue list itself takes 0.3 s; the cost was one relations request per issue (35 ms each, serial). Plane serves about 70 of those per second, so threads alone reach about 4 s. Plane does **not** bump an issue's `updated_at` when a relation changes (verified), so staleness cannot be detected cheaply; the sidecar caches relations for 60 s (`DENDRITE_PLANE_RELATIONS_TTL_SECS`), clears them on `cache.invalidate`, and bypasses the cache with `fresh: true`.

| `board.snapshot` on 369 rows / 323 issues | time |
|---|---|
| original Go (serial relations) | 27.8 s |
| Go, 16 workers | 4.1 s |
| this sidecar, cold | 4.1 s |
| this sidecar, warm cache | 0.24 s |

## Configuration

The token is never defaulted and never sent over the protocol.

| Variable | Meaning |
|---|---|
| `PLANE_API_KEY` | **Required.** Create it in Plane (Profile > API tokens); Dendrite loads it from `$DENDRITE_HOME/.env`. Without it every Plane call fails with `data.kind = "config"`. |
| `PLANE_BASE_URL` | Default `http://localhost:$PLANE_API_PORT` (8090). |
| `PLANE_WORKSPACE_SLUG` | Default `dendrite-ws`. |
| `PLANE_PROJECT_ID` | Default project; every call may override it with `project_id`. |
| `DENDRITE_PLANE_RELATIONS_TTL_SECS` | Relations cache lifetime, default 60. |

## Protocol 0.1.0

Every method that touches Plane accepts the optional `base_url`, `workspace` and `project_id`. Errors use JSON-RPC codes `-32700`, `-32600`, `-32601`, `-32602`; engine failures are `-32000` with `data.kind` one of `config`, `unauthorized`, `http`, `not_found`, `transport`, `decode`.

| Method | Params | Result |
|---|---|---|
| `health` | none | `{name, version, protocol, features, configured}` |
| `board.snapshot` | `{fresh?}` | `{issues, statuses, relations, relation_errors, relations_cached, relations_fetched}`; `statuses` maps issue id to `BACKLOG`, `TODO`, `IN_PROGRESS`, `IN_REVIEW`, `BLOCKED`, `DONE` or `CANCELLED`; `relations` covers open issues only |
| `issues.list` | `{fields?: "min"\|"full"}` | `{issues}` |
| `states.list` | none | `{states}` |
| `issue.resolve` | `{ref}` | `{issue}`; `ref` is `DEN-n`, the Plane number or the UUID, never matched by substring |
| `issue.set_status` | `{ref, status}` | `{issue, state, status}`; resolves against a single issue list |
| `relations.batch` | `{issue_ids, fresh?}` | `{relations, errors, cached, fetched}` |
| `cache.invalidate` | `{issue_ids?}` | `{ok}`; empty clears the project's cache. Call it after changing relations |

Notes for clients: relations come from Plane's internal `/api/workspaces/.../issue-relation/` route because the public v1 API has none (it answers 404); status mapping is identical to Dendrite's Go `plane.StatusFor`; the Go side still owns the zero-mock quality gate before a move to Done or To Verify.

## Build and test

```bash
cargo test            # unit tests, a fake-Plane integration suite, and protocol tests
cargo build --release # target/release/dendrite-plane
```
