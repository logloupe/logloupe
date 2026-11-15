//! Log line formats. One line in, one borrowed [`RawRecord`] out.

use std::net::IpAddr;

pub mod combined;
pub mod detect;
pub mod timestamp;

pub use combined::Combined;

/// The request part of a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    Parsed {
        method: &'a str,
        /// Request target without the query string. Not URL-decoded.
        path: &'a str,
        /// `None` when there is no query string or it is empty.
        query: Option<&'a str>,
        /// `None` for HTTP/0.9-style requests with no protocol token.
        protocol: Option<&'a str>,
    },
    /// Scanner garbage, TLS handshakes sent to a plain port, empty requests.
    /// Kept as-is because these rows matter for security reports.
    Malformed(&'a str),
}

/// One parsed log line, borrowing from the input.
#[derive(Debug, Clone, PartialEq)]
pub struct RawRecord<'a> {
    /// The address the server logged as the client (the peer, or whatever the
    /// server was configured to log in its place).
    pub ip: IpAddr,
    /// Authenticated user, `None` for `-`.
    pub user: Option<&'a str>,
    /// Unix seconds, UTC.
    pub ts: i64,
    pub request: Request<'a>,
    pub status: u16,
    pub bytes: u64,
    /// `None` for `-` or empty.
    pub referer: Option<&'a str>,
    /// `None` for `-` or empty.
    pub user_agent: Option<&'a str>,
    /// Request duration in milliseconds, when the format has a timing field.
    pub request_ms: Option<u32>,
    pub vhost: Option<&'a str>,
    /// Raw forwarded-IP field (`X-Forwarded-For` and similar), when present.
    pub forwarded_for: Option<&'a str>,
}

/// A parsed line plus whatever text the format did not consume.
#[derive(Debug, Clone, PartialEq)]
pub struct Parsed<'a> {
    pub record: RawRecord<'a>,
    /// Text after the last field the format knows about, trimmed. Non-empty
    /// `rest` counts as a mismatch during auto-detection.
    pub rest: &'a str,
}

/// Which field a line failed on. Shown next to sample bad lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseError {
    Ip,
    Timestamp,
    Request,
    Status,
    Bytes,
    Referer,
    UserAgent,
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let field = match self {
            ParseError::Ip => "client address",
            ParseError::Timestamp => "timestamp",
            ParseError::Request => "request",
            ParseError::Status => "status",
            ParseError::Bytes => "bytes",
            ParseError::Referer => "referer",
            ParseError::UserAgent => "user agent",
        };
        write!(f, "could not parse the {field} field")
    }
}

impl std::error::Error for ParseError {}

/// A pluggable access log format.
pub trait LogFormat: Send + Sync {
    /// Stable name, stored in `ingested_files.format` and accepted by `--format`.
    fn name(&self) -> &str;

    /// Parses one line (without requiring a trailing newline to be stripped).
    fn parse<'a>(&self, line: &'a str) -> Result<Parsed<'a>, ParseError>;
}
