use std::{fmt, io::Write};

/// HTTP downloads.
pub trait Downloads: Send + Sync {
    /// Fetches `url` (a GET that follows redirects) and streams the body into
    /// `sink`. Returns the number of bytes written.
    ///
    /// Blocking: call it from a background thread. Checksum verification is
    /// the caller's job, not the host's.
    fn fetch(&self, url: &str, sink: &mut dyn Write) -> Result<u64, DownloadError>;

    /// Like [`fetch`](Self::fetch), and first calls `length` with the body's
    /// length when the server sends one (`Content-Length`), so the caller can
    /// show progress as a fraction.
    fn fetch_with_length(
        &self,
        url: &str,
        sink: &mut dyn Write,
        length: &mut dyn FnMut(u64),
    ) -> Result<u64, DownloadError> {
        let _ = length;
        self.fetch(url, sink)
    }
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
