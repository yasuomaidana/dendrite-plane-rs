use thiserror::Error;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Error)]
pub enum Error {
    /// `PLANE_API_KEY` is unset or empty. There is deliberately no default token.
    #[error(
        "Plane API token is not set: set PLANE_API_KEY in $DENDRITE_HOME/.env (create one in the Plane UI under Profile > API tokens) or export it"
    )]
    MissingToken,
    #[error("Plane rejected the API key (HTTP {0})")]
    Unauthorized(u16),
    #[error("Plane returned HTTP {status} for {what}")]
    Http { status: u16, what: String },
    #[error("no ticket matches {0:?} (use DEN-n, its Plane number, or the issue UUID)")]
    TicketNotFound(String),
    #[error("no workflow state matches {0}")]
    StateNotFound(String),
    #[error("request failed: {0}")]
    Transport(String),
    #[error("could not read Plane's response: {0}")]
    Decode(String),
}

impl From<ureq::Error> for Error {
    fn from(e: ureq::Error) -> Self {
        Error::Transport(e.to_string())
    }
}
