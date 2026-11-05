# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- Protocol 1.4.1: `base_url` may repeat the configured `PLANE_BASE_URL` or name a loopback address (`localhost`, `127.0.0.0/8`, `::1`), which the request then uses; other hosts, userinfo, paths and non-http schemes are still refused. Restores Dendrite's per-call `base_url` for local Plane instances and stand-ins after 1.4.0 ([DPLANE-18]).

### Security
- Protocol 1.4.0: a request can no longer redirect the API key. `base_url` must equal the configured `PLANE_BASE_URL`, and `workspace` and `project_id` must be plain identifiers ([DPLANE-16]).
- `project.archive` and `project.delete` require an explicit `project_id` and `confirm: true`; an empty request no longer ends the default project ([DPLANE-16]).
- Issue references are used as ids only when they are real UUIDs, so a crafted 36-character string cannot reach another API route ([DPLANE-16]).

## [0.1.0]

### Added
- JSON-RPC 2.0 stdio sidecar server `Server` and binary executable `dendrite-plane` processing line-delimited requests ([DEN-342]).
- Initial Protocol 1.0.0 methods: `health`, `board.snapshot`, `issues.list`, `states.list`, `issue.resolve`, `issue.set_status`, `relations.batch`, and `cache.invalidate` ([DEN-342]).
- Inter-process relations cache sharing configured via `DENDRITE_PLANE_CACHE_DIR` ([DEN-342]).
- Protocol 1.1.0 methods: `issue.get`, `issue.create` (idempotent with `external_id`), `issue.update`, and `issue.comment` ([DEN-354]).
- JSON Schema definitions in `proto/` covering all exposed RPC methods ([DEN-354]).
- Protocol 1.2.0 methods: `project.archive` and `project.delete`.
- Protocol 1.3.0 methods: `user.me`, `issue.comments`, `issue.comment_update`, and `issue.labels_patch`.
- Automatic label name resolution to UUIDs within `issue.update` and assignee list updates.
- Structured engine error reporting using JSON-RPC code `-32000` with classified `data.kind` (`config`, `unauthorized`, `http`, `not_found`, `invalid`, `transport`, `decode`).
