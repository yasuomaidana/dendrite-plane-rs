# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- CI: every workflow runs on every pull request, whatever its target branch (`ci.yml` no longer lists `develop`, `main` and `feature/**`); pushes still run on `develop`, `main` and `feature/**` (DPLANE-22).
- CI: `.github/workflows/complexity.yml` cross-checks function complexity with clippy `cognitive_complexity` and `lizard`; report only while DPLANE-11 is open (DPLANE-10).

## [0.1.0]

### Added
- Initial workspace implementation of `plane-client` and `dendrite-plane` sidecar featuring board snapshots, issue resolution, status transitions, and concurrent relation fetching ([DEN-342]).
- Atomic file-backed relation caching shared between processes via `DENDRITE_PLANE_CACHE_DIR` under Protocol 1.0.0 ([DEN-342]).
- Repository linking and tracking integration with the Plane project ([DPLANE]).
- Single-issue lookups (`issue.get`), idempotent issue creation (`issue.create`), updates (`issue.update`), comments (`issue.comment`), and JSON schema definitions in `proto/` under Protocol 1.1.0 ([DEN-354]).
- Project lifecycle endpoints for archiving (`project.archive`) and deletion (`project.delete`) under Protocol 1.2.0.
- Authenticated user endpoint (`user.me`), comment listing (`issue.comments`) and HTML editing (`issue.comment_update`), and label patching with prefix removal (`issue.labels_patch`) under Protocol 1.3.0.
- Label resolution by name within issue updates and assignee list assignments.
- Structured engine error reporting using JSON-RPC error `-32000` with classified kinds (`config`, `unauthorized`, `http`, `not_found`, `invalid`, `transport`, `decode`).

### Changed
- Extended issue update payloads to support assignees and label updates.
