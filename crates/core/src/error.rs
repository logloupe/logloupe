use std::path::PathBuf;

pub type Result<T, E = Error> = std::result::Result<T, E>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{}: {source}", path.display())]
    File {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("database error: {0}")]
    Db(#[from] duckdb::Error),

    #[error("site '{0}' is in use by another process")]
    SiteLocked(String),

    #[error("site '{0}' does not exist")]
    SiteNotFound(String),

    #[error("site '{0}' already exists")]
    SiteExists(String),

    #[error("{0}")]
    Invalid(String),

    #[error("could not detect the log format of {}. Tried: {tried}", path.display())]
    UnknownFormat { path: PathBuf, tried: String },

    #[error(
        "{}: {failed} of the first {checked} lines did not parse as '{format}'. Wrong format?",
        path.display()
    )]
    WrongFormat {
        path: PathBuf,
        format: String,
        failed: u64,
        checked: u64,
    },

    #[error(
        "{} overlaps the time range of files already ingested ({}). \
         Use --overlap skip to drop lines in a range already covered, or --overlap allow to ingest everything.",
        path.display(),
        with.join(", ")
    )]
    Overlap { path: PathBuf, with: Vec<String> },

    #[error("unknown report '{0}'")]
    UnknownReport(String),

    /// Fetching from a server failed (spec 9). The message is already
    /// stripped of control characters; some of it comes from the server.
    #[error("{host}: {message}")]
    Remote { host: String, message: String },

    #[error("cancelled")]
    Cancelled,
}

impl Error {
    /// Stable machine-readable code, part of the JSON output contract.
    pub fn code(&self) -> &'static str {
        match self {
            Error::Io(_) | Error::File { .. } => "io_error",
            Error::Db(_) => "database_error",
            Error::SiteLocked(_) => "site_locked",
            Error::SiteNotFound(_) => "site_not_found",
            Error::SiteExists(_) => "site_exists",
            Error::Invalid(_) => "invalid_argument",
            Error::UnknownFormat { .. } => "unknown_format",
            Error::WrongFormat { .. } => "wrong_format",
            Error::Overlap { .. } => "overlap",
            Error::UnknownReport(_) => "unknown_report",
            Error::Remote { .. } => "remote_error",
            Error::Cancelled => "cancelled",
        }
    }

    pub(crate) fn invalid(message: impl Into<String>) -> Self {
        Error::Invalid(message.into())
    }
}
