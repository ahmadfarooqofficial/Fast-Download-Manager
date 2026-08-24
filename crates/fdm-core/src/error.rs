use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("invalid url: {0}")]
    Url(#[from] url::ParseError),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("server returned status {0}")]
    Status(u16),

    /// The server answered a ranged request with something other than 206.
    ///
    /// Either it ignores `Range` entirely, or `If-Range` failed and it sent the
    /// whole resource. Writing that body at a segment offset would silently
    /// corrupt the file, so the caller must discard all partial data and
    /// restart as a single sequential stream.
    #[error("range request not honoured (server replied {status}); must restart sequentially")]
    RangeLost { status: u16 },

    #[error("range not satisfiable (416) at offset {offset}; remote file shrank")]
    RangeNotSatisfiable { offset: u64 },

    #[error("resource changed on server since the download started")]
    ResourceChanged,

    #[error("gave up after exhausting retries: {0}")]
    RetriesExhausted(String),

    #[error("download cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn other(msg: impl Into<String>) -> Self {
        Error::Other(msg.into())
    }

    /// Transient failures worth retrying with backoff, as opposed to
    /// structural problems where retrying the same request cannot help.
    pub fn is_retryable(&self) -> bool {
        match self {
            Error::Http(e) => e.is_timeout() || e.is_connect() || e.is_request() || e.is_body(),
            Error::Io(e) => matches!(
                e.kind(),
                std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
                    | std::io::ErrorKind::ConnectionReset
                    | std::io::ErrorKind::ConnectionAborted
                    | std::io::ErrorKind::BrokenPipe
                    | std::io::ErrorKind::UnexpectedEof
            ),
            // 408 Request Timeout, 429 Too Many Requests, and 5xx are worth another try.
            Error::Status(s) => *s == 408 || *s == 429 || (500..600).contains(s),
            _ => false,
        }
    }
}

pub type Result<T> = std::result::Result<T, Error>;
