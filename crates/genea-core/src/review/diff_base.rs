//! A listed file's review baseline as text, for its inline diff (ticket
//! #55).

use std::{io, path::Path, sync::Arc};

use ropey::Rope;

use super::{
    Review,
    store::{Hash, Store},
};
use crate::view::ChangeItem;

/// What reading a listed file's review baseline takes. Two readers of the
/// same baseline compare equal.
#[derive(Clone)]
pub(crate) struct BaselineReader {
    store: Arc<Store>,
    /// `None`: the file has no baseline (it was created), so it is empty.
    hash: Option<Hash>,
}

impl PartialEq for BaselineReader {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash
    }
}

impl BaselineReader {
    /// Reads the baseline's text. Blocking: runs on a background thread.
    pub(crate) fn read(&self) -> io::Result<Rope> {
        let Some(hash) = &self.hash else { return Ok(Rope::new()) };
        let bytes = self.store.read_blob(hash)?;
        let text = String::from_utf8(bytes).map_err(|_| io::Error::from(io::ErrorKind::InvalidData))?;
        Ok(Rope::from(text))
    }
}

impl Review {
    /// The file's entry in Changes, if it is listed.
    pub(crate) fn listed_change(&self, path: &Path) -> Option<&ChangeItem> {
        self.rows.iter().find(|item| item.path == path)
    }

    /// A reader of the file's baseline, if it is listed and can be diffed.
    pub(crate) fn baseline_reader(&self, path: &Path) -> Option<BaselineReader> {
        if !self.listed_change(path)?.diffable {
            return None;
        }
        let hash = match self.baseline.get(path) {
            Some(state) if !state.stored => return None,
            Some(state) => Some(state.hash),
            None => None,
        };
        Some(BaselineReader { store: self.store.clone()?, hash })
    }
}
