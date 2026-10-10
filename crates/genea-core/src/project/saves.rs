//! Saves in flight, per file. Each save writes on a background thread, and
//! two saves of one file (⌘S twice) must neither interleave their writes
//! nor mark an older text saved after a newer one. So a file's writes take
//! turns, and a save that a newer one has overtaken writes nothing: the
//! newest save's text is what ends up on disk.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
};

/// The project's saves, by file. Cheap to clone, usable from any thread.
#[derive(Clone, Default)]
pub(super) struct Saves(Arc<Mutex<HashMap<PathBuf, Arc<FileSaves>>>>);

#[derive(Default)]
struct FileSaves {
    /// The newest save's number.
    newest: AtomicU64,
    /// Held while the file is written.
    turn: Mutex<()>,
}

/// One save of a file, numbered in the order the saves started.
pub(super) struct SaveTicket {
    file: Arc<FileSaves>,
    number: u64,
}

impl Saves {
    /// Call on the main thread when a save of `path` starts.
    pub(super) fn start(&self, path: &Path) -> SaveTicket {
        let file = self.0.lock().unwrap().entry(path.to_owned()).or_default().clone();
        let number = file.newest.fetch_add(1, Ordering::SeqCst) + 1;
        SaveTicket { file, number }
    }
}

impl SaveTicket {
    /// Whether no save of the file started after this one.
    pub(super) fn is_newest(&self) -> bool {
        self.file.newest.load(Ordering::SeqCst) == self.number
    }

    /// Waits for the file's earlier writes to finish; hold the guard while
    /// writing.
    pub(super) fn turn(&self) -> MutexGuard<'_, ()> {
        self.file.turn.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}
