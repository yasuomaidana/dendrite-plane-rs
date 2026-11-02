# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0]

### Added
- Typed Plane REST API client `Client` and configuration loader `Config` with environment variable resolution (`PLANE_API_KEY`, `PLANE_BASE_URL`, `PLANE_WORKSPACE_SLUG`, `PLANE_PROJECT_ID`) and strict token enforcement ([DEN-342]).
- Cursor-paginated issue listing (`list_issues`) with minimal scheduling field projections (`ListFields::Min`) and module duplicate filtering ([DEN-342]).
- Bounded 16-worker thread pool (`RELATION_WORKERS`) for concurrent relation fetching (`relations_batch`), and whole-board snapshot generation (`snapshot`) skipping closed tickets ([DEN-342]).
- In-memory and atomic inter-process file-backed relation caching (`with_cache_file`) with configurable TTL and invalidation (`invalidate_relations`, `clear_relations_cache`) ([DEN-342]).
- Canonical status resolution (`status_for`) and reference matching (`resolve_issue`) supporting UUIDs, sequence IDs, bracketed IDs, and token boundaries ([DEN-342]).
- Single-issue retrieval (`get_issue`) with state resolution fallback against the states list ([DEN-354]).
- Idempotent issue creation (`create_issue`) keyed on `external_id`, estimate point clamping to 0..12, and empty context validation (`is_empty_context`) ([DEN-354]).
- Issue updates (`update_issue`) and comment creation wrapped in paragraph tags (`comment`) ([DEN-354]).
- Project lifecycle management endpoints for archiving (`archive_project`) and deletion (`delete_project`).
- Authenticated user retrieval (`me`) querying the public API with fallback to internal endpoints.
- Comment inspection (`list_comments`, sorted oldest first) and raw HTML modification (`update_comment`).
- Project label listing (`list_labels`), idempotent creation (`ensure_label`), and prefix-based patch updates (`patch_labels`).

### Changed
- Extended `UpdateIssue` to support assignee list assignment and label updating.
