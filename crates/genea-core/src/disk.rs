//! External changes in open editors (spec #19, Review and external changes;
//! ticket #32).
//!
//! Each open file remembers what Genea believes is on disk: the text it
//! read, or the text it last started writing. When the watcher reports a
//! change to the file, a background [`DiskCheck`] reads it and compares. The
//! same text is no change, which is how Genea's own saves are recognised.
//! Different text is an external change: a clean buffer reloads it as one
//! undoable edit, and a buffer with unsaved edits shows the conflict bar.

use std::{ops::Range, path::Path};

use ropey::Rope;

use crate::text::Decoded;

/// What a background check needs from an open file, taken on the main
/// thread (cheap: rope clones share their nodes).
pub(crate) struct DiskCheck {
    /// What Genea believes is on disk.
    pub(crate) disk: Rope,
    /// The buffer, to work out the reload's edit from.
    pub(crate) text: Rope,
    /// The buffer's version and the disk text's generation when checked:
    /// the result is stale once either has moved on.
    pub(crate) version: u64,
    pub(crate) generation: u64,
}

/// The result of a check.
pub(crate) struct Checked {
    pub(crate) version: u64,
    pub(crate) generation: u64,
    /// `None` when the file holds what Genea believes, or can't be read as
    /// text (a deleted, binary or non-UTF-8 file is left alone).
    pub(crate) changed: Option<DiskText>,
}

/// A file's new text on disk.
pub(crate) struct DiskText {
    pub(crate) text: Rope,
    /// The edit that turns the buffer (as checked) into it; `None` when
    /// they are already the same.
    pub(crate) edit: Option<Splice>,
}

/// Replaces a char range of a text.
pub(crate) struct Splice {
    pub(crate) remove: Range<usize>,
    pub(crate) insert: String,
}

impl DiskCheck {
    /// Reads the file and compares it. Runs on a background thread. Only a
    /// regular file is read: opening a pipe would block until it has a
    /// writer.
    pub(crate) fn run(self, absolute: &Path) -> Checked {
        let read = match std::fs::metadata(absolute) {
            Ok(metadata) if metadata.is_file() => std::fs::read(absolute),
            _ => Err(std::io::ErrorKind::Unsupported.into()),
        };
        let changed = match read.map(Decoded::from_bytes) {
            Ok(Decoded::Text(read)) if self.disk != read => {
                let text = Rope::from_str(&read);
                let edit = splice(&self.text, &text);
                Some(DiskText { text, edit })
            }
            _ => None,
        };
        Checked { version: self.version, generation: self.generation, changed }
    }
}

/// The smallest single splice that turns `from` into `to` (the changed
/// stretch between their common start and end), or `None` if they are equal.
pub(crate) fn splice(from: &Rope, to: &Rope) -> Option<Splice> {
    let prefix = from.chars().zip(to.chars()).take_while(|(a, b)| a == b).count();
    if prefix == from.len_chars() && prefix == to.len_chars() {
        return None;
    }
    let most = from.len_chars().min(to.len_chars()) - prefix;
    let (mut a, mut b) = (from.chars_at(from.len_chars()), to.chars_at(to.len_chars()));
    let mut suffix = 0;
    while suffix < most && a.prev() == b.prev() {
        suffix += 1;
    }
    let to_end = to.len_chars() - suffix;
    Some(Splice { remove: prefix..from.len_chars() - suffix, insert: to.slice(prefix..to_end).to_string() })
}
