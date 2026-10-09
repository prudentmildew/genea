//! The fuzzy finder (ticket #33): an overlay that finds files and actions
//! as the user types.
//!
//! Matching runs off the main thread with `nucleo-matcher`, over the file
//! index's list of files (`FileIndex::file_list`), which leaves out
//! `node_modules`, `.git` and the config's `exclude`. Each query starts a
//! match job; a newer query (or a closed finder) drops an older job's
//! result. When the file list changes while the finder is open, the query
//! is matched again, so results follow files being created and deleted.

use std::{
    cmp::Reverse,
    path::{Path, PathBuf},
    sync::Arc,
};

use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

use crate::view::{FinderItem, FinderItemKind, FinderMode, FinderView};

/// The most results the finder lists.
pub(crate) const MAX_RESULTS: usize = 100;

/// The open finder.
pub(crate) struct Finder {
    pub(crate) mode: FinderMode,
    pub(crate) query: String,
    items: Vec<FinderItem>,
    selected: usize,
    /// The query changed since the last results came in: the next ones
    /// select their best result.
    new_query: bool,
}

impl Finder {
    pub(crate) fn new(mode: FinderMode) -> Self {
        Finder { mode, query: String::new(), items: Vec::new(), selected: 0, new_query: true }
    }

    pub(crate) fn set_query(&mut self, query: String) {
        self.query = query;
        self.new_query = true;
    }

    /// Takes a match's results. A new query selects the best result; new
    /// results for the same query (the files changed) keep the selection
    /// where it was.
    pub(crate) fn set_items(&mut self, items: Vec<FinderItem>) {
        self.items = items;
        if std::mem::take(&mut self.new_query) {
            self.selected = 0;
        }
        self.selected = self.selected.min(self.items.len().saturating_sub(1));
    }

    /// Moves the selection by `by` results, wrapping around.
    pub(crate) fn move_selection(&mut self, by: isize) {
        if let Ok(len) = isize::try_from(self.items.len())
            && len > 0
        {
            self.selected = (self.selected as isize + by).rem_euclid(len) as usize;
        }
    }

    pub(crate) fn select(&mut self, index: usize) {
        if index < self.items.len() {
            self.selected = index;
        }
    }

    /// The result Return would choose.
    pub(crate) fn selected_item(&self) -> Option<&FinderItem> {
        self.items.get(self.selected)
    }

    pub(crate) fn view(&self) -> FinderView {
        FinderView {
            mode: self.mode,
            query: self.query.clone(),
            items: self.items.clone(),
            selected: (!self.items.is_empty()).then_some(self.selected),
        }
    }
}

/// What a match job matches the query against.
pub(crate) struct Candidates {
    /// Every file in the project (relative paths), for the file modes.
    pub(crate) files: Arc<[String]>,
}

/// Matches `query` against the candidates (a background thread).
pub(crate) fn run_match(query: &str, candidates: &Candidates) -> Vec<FinderItem> {
    let pattern = Pattern::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy);
    let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
    let mut buf = Vec::new();
    let mut matched: Vec<(u32, &str)> = candidates
        .files
        .iter()
        .filter_map(|path| pattern.score(Utf32Str::new(path, &mut buf), &mut matcher).map(|score| (score, path.as_str())))
        .collect();
    // Best first; on a tie, the shorter path, then by path.
    matched.sort_unstable_by_key(|(score, path)| (Reverse(*score), path.len(), *path));
    matched.into_iter().take(MAX_RESULTS).map(|(_, path)| file_item(Path::new(path))).collect()
}

/// A file as a result: its name, with its folder beside it.
fn file_item(path: &Path) -> FinderItem {
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let detail = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    FinderItem { label, detail, shortcut: None, kind: FinderItemKind::File(PathBuf::from(path)) }
}
