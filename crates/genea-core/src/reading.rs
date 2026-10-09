//! Reading a file for the editor, first screen first (spec #19, Large
//! files; ticket #27).
//!
//! Runs on a background thread. A file that may be large (over
//! [`LARGE_FILE_BYTES`], or of a size not known up front, like a pipe)
//! hands over its first screen as soon as those lines are read, so the
//! editor can show them while the rest comes in. Everything slow happens
//! here, off the main thread: reading, UTF-8 checking and building the
//! rope.

use std::{
    fs::File,
    io::{self, Read},
    path::Path,
};

use ropey::Rope;

use crate::{
    editor::LARGE_FILE_BYTES,
    text::{BINARY_SNIFF_BYTES, Decoded},
};

/// A whole file, as the editor gets it.
pub(crate) enum Contents {
    /// Valid UTF-8: editable.
    Text(Rope),
    /// Not valid UTF-8: read-only, each invalid sequence a U+FFFD.
    Invalid(Rope),
    /// Not text: the editor doesn't show it.
    Binary,
}

/// The beginning of a file still being read: whole lines, at least a
/// screenful. Always a prefix of the whole file's text, so positions in it
/// stay valid once the rest is in.
pub(crate) struct FirstScreen {
    pub(crate) text: Rope,
    /// The file's size on disk when the read started, if it is a regular
    /// file.
    pub(crate) size: Option<u64>,
}

/// How much one read takes while looking for the first screen.
const CHUNK: usize = 64 * 1024;

/// Reads the file at `path`. If it may be large, `first_screen` gets its
/// first `rows` lines (and some) as soon as they are in, unless the whole
/// file is in by then; it is called at most once, before this returns.
pub(crate) fn read(path: &Path, rows: usize, first_screen: impl FnOnce(FirstScreen)) -> io::Result<Contents> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    let size = metadata.is_file().then(|| metadata.len());
    let mut bytes = Vec::with_capacity(size.map_or(CHUNK, |s| s as usize + 1));
    if size.is_none_or(|s| s > LARGE_FILE_BYTES as u64) {
        let mut first_screen = Some(first_screen);
        let mut newlines = 0;
        loop {
            let start = bytes.len();
            bytes.resize(start + CHUNK, 0);
            let read = file.read(&mut bytes[start..])?;
            bytes.truncate(start + read);
            if read == 0 {
                break;
            }
            newlines += bytes[start..].iter().filter(|&&b| b == b'\n').count();
            if newlines > rows && bytes.len() >= BINARY_SNIFF_BYTES {
                if let Some(text) = first_lines(&bytes)
                    && let Some(first_screen) = first_screen.take()
                {
                    first_screen(FirstScreen { text, size });
                }
                break;
            }
        }
    }
    file.read_to_end(&mut bytes)?;
    Ok(match Decoded::from_bytes(bytes) {
        Decoded::Text(text) => Contents::Text(Rope::from_str(&text)),
        Decoded::Invalid(text) => Contents::Invalid(Rope::from_str(&text)),
        Decoded::Binary => Contents::Binary,
    })
}

/// The whole lines in `bytes`, without the last line break, or `None` if
/// they look binary.
fn first_lines(bytes: &[u8]) -> Option<Rope> {
    if bytes[..BINARY_SNIFF_BYTES].contains(&0) {
        return None;
    }
    let end = bytes.iter().rposition(|&b| b == b'\n')?;
    let end = if end > 0 && bytes[end - 1] == b'\r' { end - 1 } else { end };
    Some(Rope::from_str(&String::from_utf8_lossy(&bytes[..end])))
}
