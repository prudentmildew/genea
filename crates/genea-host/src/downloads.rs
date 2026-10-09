use std::{fmt, io::Write};

/// HTTP downloads.
pub trait Downloads: Send + Sync {
    /// Fetches `url` (a GET that follows redirects) and streams the body into
    /// `sink`. Returns the number of bytes written.
    ///
    /// Blocking: call it from a background thread. Checksum verification is
    /// the caller's job, not the host's.
    fn fetch(&self, url: &str, sink: &mut dyn Write) -> Result<u64, DownloadError>;
}

/// Why a download failed.
#[derive(Debug)]
pub enum DownloadError {
    /// The server answered with a non-success status.
    Status(u16),
    /// The request never got an answer (DNS, TLS, connection, timeout).
    Transport(String),
    /// Writing the body into the sink failed.
    Io(std::io::Error),
}

impl fmt::Display for DownloadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DownloadError::Status(code) => write!(f, "the server answered HTTP {code}"),
            DownloadError::Transport(message) => write!(f, "{message}"),
            DownloadError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for DownloadError {}

impl From<std::io::Error> for DownloadError {
    fn from(error: std::io::Error) -> Self {
        DownloadError::Io(error)
    }
}
