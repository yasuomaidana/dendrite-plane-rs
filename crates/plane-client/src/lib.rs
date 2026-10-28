//! Typed client for the Plane API, built for speed on boards of hundreds of tickets.
//!
//! Three things make it fast compared with a naive client: list calls ask for `per_page=100` and
//! only the fields they need, relations are fetched by a bounded pool of worker threads, and
//! finished tickets are never asked for relations at all.
//!
//! The API token is never defaulted: [`Config::from_env`] fails with [`Error::MissingToken`].

mod client;
mod error;
mod resolve;
mod types;

pub use client::{
    Client, Config, DEFAULT_RELATIONS_TTL, ListFields, RELATION_WORKERS, RelationsBatch, Snapshot,
};
pub use error::{Error, Result};
pub use resolve::resolve_issue;
pub use types::{Issue, RelationRef, Relations, State, Status, status_for};
