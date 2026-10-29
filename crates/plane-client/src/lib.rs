//! Typed client for the Plane API, built for speed on boards of hundreds of tickets.
//!
//! Three things make it fast compared with a naive client: list calls ask for `per_page=100` and
//! only the fields they need, relations are fetched by a bounded pool of worker threads, and
//! finished tickets are never asked for relations at all.
//!
//! The API token is never defaulted: [`Config::from_env`] fails with [`Error::MissingToken`].

mod client;
mod comments;
mod error;
mod labels;
mod relations;
mod resolve;
mod types;

pub use client::{Client, Config, ListFields};
pub use error::{Error, Result};
pub use relations::{DEFAULT_RELATIONS_TTL, RELATION_WORKERS, RelationsBatch, Snapshot};
pub use resolve::resolve_issue;
pub use types::{
    Comment, CreateIssue, Issue, Label, LabelDetail, RelationRef, Relations, State, StateDetail,
    Status, UpdateIssue, User, is_empty_context, status_for,
};
