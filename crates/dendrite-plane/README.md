# dendrite-plane

`dendrite-plane` is a JSON-RPC 2.0 sidecar binary and server library that provides long-lived, high-throughput access to Plane over stdio for Dendrite agents and the Dendrite control plane. Instead of launching a new client process per operation, Dendrite maintains a single persistent `dendrite-plane` daemon that processes line-delimited JSON-RPC requests on stdin, emits responses on stdout, and directs logging to stderr. The sidecar encapsulates environment-based authentication, connection persistence, error classification, and inter-process relation caching across requests.

## When to use it

Use `dendrite-plane` when:
- Integrating non-Rust components (such as Go control plane daemons, CLI tools, or external agent scripts) with Plane through a standardized stdio JSON-RPC interface.
- Keeping a persistent process running to avoid the overhead of repeated process startups and TLS connection renegotiations.
- Performing batch ticket operations, dependency resolution, status updates, or label modifications through line-delimited JSON-RPC 2.0 protocols.
- Coordinating relation cache invalidation across separate agent processes via a shared cache directory (`DENDRITE_PLANE_CACHE_DIR`).

Do not use `dendrite-plane` when:
- Developing purely within a native Rust application that can link directly against the `plane-client` crate.
- Requiring non-stdio transports like HTTP webhooks or streaming WebSocket feeds, which are handled directly by the Dendrite control plane.

## Binary commands and library usage

### Binary command

The crate produces a binary named `dendrite-plane`:

```bash
# Start the stdio sidecar daemon
dendrite-plane
```

The daemon reads newline-delimited JSON-RPC 2.0 messages from stdin, processes each request synchronously, flushes the resulting JSON response to stdout on a single line, and writes diagnostics to stderr. It terminates gracefully when stdin reaches EOF.

Example interaction via stdio:

```bash
echo '{"jsonrpc":"2.0","id":1,"method":"health","params":{}}' | cargo run -q -p dendrite-plane
```

Expected stdout output (`configured` is `true` when `PLANE_API_KEY` is set in the environment):

```json
{"id":1,"jsonrpc":"2.0","result":{"configured":false,"features":[],"name":"dendrite-plane","protocol":"1.3.0","version":"0.1.0"}}
```

### Main types

When used as a library, `dendrite-plane` exposes:

- `PROTOCOL`: The supported protocol version string (`"1.3.0"`).
- `Server`: The JSON-RPC request dispatcher.
  - `Server::from_env()`: Initializes the server reading `PLANE_API_KEY`, `PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG`, `PLANE_PROJECT_ID`, `DENDRITE_PLANE_RELATIONS_TTL_SECS`, and `DENDRITE_PLANE_CACHE_DIR`. If `PLANE_API_KEY` is missing, startup still succeeds, `health` reports `configured: false`, and subsequent Plane calls fail with error kind `config`.
  - `Server::with_config(cfg: Config)`: Initializes the server with an explicit `plane_client::Config`.
  - `Server::handle_line(&mut self, line: &str) -> Option<String>`: Parses and evaluates a JSON-RPC line. Returns `Some(response_json)` for requests, and `None` for notifications (requests without an `id`).

### Library example

```rust
use dendrite_plane::{PROTOCOL, Server};

fn main() {
    let mut server = Server::from_env();
    let request = r#"{"jsonrpc":"2.0","id":1,"method":"health","params":{}}"#;

    if let Some(response) = server.handle_line(request) {
        println!("Server response (Protocol {PROTOCOL}): {response}");
    }
}
```

## Protocol methods

`dendrite-plane` implements Protocol 1.3.0. Standard JSON-RPC errors use codes `-32700` (Parse error), `-32600` (Invalid request), `-32601` (Method not found), and `-32602` (Invalid params). Underlying engine errors use code `-32000` with `error.data.kind` set to one of `config`, `unauthorized`, `http`, `not_found`, `invalid`, `transport`, or `decode`.

| Method | Parameters | Result |
|---|---|---|
| `health` | none | `{name, version, protocol, features, configured}` |
| `board.snapshot` | `{fresh?}` | `{issues, statuses, relations, relation_errors, relations_cached, relations_fetched}` |
| `issues.list` | `{fields?: "min"\|"full"}` | `{issues}` |
| `states.list` | none | `{states}` |
| `issue.resolve` | `{ref}` | `{issue}` |
| `issue.get` | `{ref}` | `{issue}` |
| `issue.create` | `{name, description_html, state?, estimate_point?, priority?, start_date?, target_date?, parent?, external_id?, external_source?, labels?}` | `{issue, created}` |
| `issue.update` | `{ref, name?, description_html?, state?, estimate_point?, priority?, start_date?, target_date?, parent?, assignees?, labels?}` | `{issue}` |
| `issue.labels_patch` | `{ref, add: [names], remove_prefixes: [strings]}` | `{issue, labels}` |
| `user.me` | none | `{user: {id, email, display_name}}` |
| `issue.comments` | `{ref}` | `{comments: [{id, comment_html, created_at, updated_at}]}` |
| `issue.comment_update` | `{ref, comment_id, comment_html}` | `{ok: true}` |
| `issue.comment` | `{ref, comment}` | `{ok: true}` |
| `issue.set_status` | `{ref, status}` | `{issue, state, status}` |
| `relations.batch` | `{issue_ids, fresh?}` | `{relations, errors, cached, fetched}` |
| `cache.invalidate` | `{issue_ids?}` | `{ok: true}` |
| `project.archive` | none | `{ok: true}` |
| `project.delete` | none | `{ok: true}` |

Every method accepting target parameters supports optional overrides for `base_url`, `workspace`, and `project_id`.

## Feature flags

This crate defines no optional Cargo feature flags.

## Relation to other crates and repositories

- **Workspace Crates**:
  - `plane-client`: The core typed Plane client library that `dendrite-plane` wraps.
- **Sibling Repositories**:
  - `dendrite-core-rs`: Agent runtime that interfaces with tracker sidecars like `dendrite-plane`.
  - `dendrite-git-rs`: Manages git branch and worktree lifecycles for issues tracked via `dendrite-plane`.
  - `dendrite-rag-rs`: Stdio JSON-RPC sidecar providing semantic codebase retrieval, matching the architectural patterns of `dendrite-plane`.
  - `dendrite-plane-rs`: The parent workspace containing `dendrite-plane` and `plane-client`.
  - `embedag`: Embeddings service utilized by Dendrite agents.
  - `Dendrite control plane`: Spawns and supervises `dendrite-plane` processes to drive agent issue tracking.

## Build and test

Build the release binary:

```bash
cargo build --release -p dendrite-plane
```

Run test suite verifying JSON-RPC compliance, routing, error codes, and schema synchronization:

```bash
cargo test -p dendrite-plane
```

## Specification

The sidecar specification is formalized in JSON Schema documents located in the workspace `proto/` directory:
- `proto/*.json`: Individual schemas for each of the 18 protocol methods detailing parameter requirements and response shapes.
- Tests in `tests/protocol.rs` verify that every method handled by the server has a corresponding schema in `proto/`, with no extraneous or missing schema files.
