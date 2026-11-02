# plane-client

`plane-client` is a typed Rust client for the Plane project-management REST API, optimized for speed when interacting with boards containing hundreds of issues. A naive client querying issue dependencies serially over Plane's REST endpoints takes upwards of 28 seconds on medium-sized boards because relation requests require ~35 ms each. `plane-client` reduces this turnaround time to fractions of a second by fetching relations across a bounded pool of 16 worker threads, caching relation sets in memory and optionally in a shared atomic file across processes, skipping relation requests entirely for closed tickets (Done and Cancelled), and querying cursor-paginated issue lists with a minimal field projection (`per_page=100`) while deduplicating rows that Plane repeats for multiple modules.

## When to use it

Use `plane-client` when:
- Building Rust applications, CLI tools, or services that need direct, typed, and performant access to Plane boards, issues, comments, labels, and workflow states.
- Generating board snapshots or calculating dependency schedules across hundreds of tickets without blocking on serial HTTP requests.
- Performing lifecycle operations including idempotent ticket creation keyed by `external_id`, partial issue updates, comment additions and updates, and status transitions.
- Sharing a disk-backed relations cache across multiple short-lived process invocations.

Do not use `plane-client` when:
- Integrating with Plane from non-Rust environments (such as Go, Python, or shell scripts); use the `dendrite-plane` JSON-RPC stdio sidecar instead.
- Subscribing to real-time webhook events or WebSocket feeds, which are handled directly by the Dendrite control plane.
- Managing rich workspace configurations, roles, or billing outside of ticket and project management.

## Main types and functions

The primary types and entry points in `plane-client` are:

- `Config`: Connection settings (`base_url`, `workspace`, `project_id`, `api_key`). `Config::from_env` reads `PLANE_API_KEY` (required; fails with `Error::MissingToken` if missing or empty), `PLANE_BASE_URL` (default `http://localhost:$PLANE_API_PORT` or `http://localhost:8090`), `PLANE_WORKSPACE_SLUG` (default `dendrite-ws`), and `PLANE_PROJECT_ID`.
- `Client`: HTTP agent managing connections, relation worker pools, TTL policies, and in-memory/file caching.
- `Issue`: Work item representation including identifier, sequence number, state, priority, estimate points, dates, labels, and descriptions.
- `CreateIssue`: Input parameters for creating a new issue, with idempotency handling via `external_id`.
- `UpdateIssue`: Patch parameters where unset or empty string fields are omitted from the request.
- `Status`: Canonical issue lifecycle state (`Backlog`, `Todo`, `InProgress`, `InReview`, `Blocked`, `Done`, `Cancelled`).
- `State`: Plane workflow state with unique identifier, name, and group.
- `Snapshot`: Full board snapshot bundling issues, mapped statuses, and relations for active tickets.
- `Relations`: Struct representing `blocking`, `blocked_by`, `duplicate`, and `relates_to` dependencies.
- `ListFields`: Enum selecting whether list queries fetch minimal scheduling fields (`ListFields::Min`) or full payloads (`ListFields::Full`).
- `resolve_issue`: Standalone function matching ticket references against a list of issues by UUID, sequence number, bracketed `[ID]` prefix, or whole-token boundary without false substring matches.
- `status_for`: Standalone mapper converting Plane workflow state group and name into a canonical `Status`.
- `is_empty_context`: Helper verifying whether description text has substantive content after stripping HTML tags and non-breaking spaces.

## Example

```rust
use plane_client::{Client, Config, CreateIssue, ListFields, Status};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Configure client from environment variables (PLANE_API_KEY is required)
    let config = Config::from_env()?;
    let client = Client::new(config);

    // List issues with minimal field projection to save bandwidth
    let issues = client.list_issues(ListFields::Min)?;
    println!("Retrieved {} issues", issues.len());

    // Take a snapshot: fetches open ticket relations concurrently and caches results
    let snapshot = client.snapshot(false)?;
    println!(
        "Snapshot loaded: {} issues, {} relations cached, {} relations fetched",
        snapshot.issues.len(),
        snapshot.relations_cached,
        snapshot.relations_fetched
    );

    // Create an issue idempotently; existing external_id returns (issue, false)
    let new_ticket = CreateIssue {
        name: "Process telemetry stream".to_string(),
        description_html: "<p>Implement worker to process queued telemetry.</p>".to_string(),
        estimate_point: Some(3),
        priority: Some("high".to_string()),
        external_id: Some("TASK-901".to_string()),
        ..CreateIssue::default()
    };
    let (issue, created) = client.create_issue(&new_ticket)?;
    println!("Issue ID: {}, created: {}", issue.id, created);

    // Move the issue to the In Progress workflow state
    let (updated_issue, state) = client.set_status(&issue.id, Status::InProgress)?;
    println!("Updated {} to state: {}", updated_issue.name, state.name);

    Ok(())
}
```

## Feature flags

This crate defines no optional Cargo feature flags.

## Relation to other crates and repositories

- **Workspace Crates**:
  - `dendrite-plane`: The stdio JSON-RPC 2.0 sidecar binary and server that wraps `plane-client` to provide long-lived Plane access for non-Rust components.
- **Sibling Repositories**:
  - `dendrite-core-rs`: Defines core agent execution primitives, tool abstractions, and tracker traits that integrate with `plane-client`.
  - `dendrite-git-rs`: Manages git operations and isolated worktrees associated with tickets managed in Plane.
  - `dendrite-rag-rs`: Stdio JSON-RPC sidecar providing semantic retrieval and code index capabilities, using the same sidecar lifecycle patterns as `dendrite-plane`.
  - `dendrite-plane-rs`: This repository and workspace, housing both `plane-client` and `dendrite-plane`.
  - `embedag`: Embeddings service used for indexing and vector search across Dendrite agents.
  - `Dendrite control plane`: The primary Go orchestrator that coordinates agent task dispatching, ticket state tracking, and sidecar process lifecycles.

## Build and test

Run the test suite, which includes unit tests and an in-process mock HTTP server exercising edge cases:

```bash
cargo test -p plane-client
```

## Specification

The Plane API behavior conforms to:
- Plane REST API v1 endpoints (`/api/v1/workspaces/...`) for issues, states, labels, and comments.
- Plane internal workspace endpoints (`/api/workspaces/.../issue-relation/`) for ticket relation graphs.
- Compatibility contracts matching Dendrite's Go client implementations: `plane.StatusFor`, `plane.ResolveIssue`, and `plane.IsEmptyContext`.
