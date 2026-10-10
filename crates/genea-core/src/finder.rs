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

use crate::{
    action::Action,
    view::{FinderItem, FinderItemKind, FinderMode, FinderView},
};

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

    /// Takes a match's results. A new query selects result `first`
    /// (usually the best); new results for the same query (the files
    /// changed) keep the selection where it was.
    pub(crate) fn set_items(&mut self, items: Vec<FinderItem>, first: usize) {
        self.items = items;
        if std::mem::take(&mut self.new_query) {
            self.selected = first;
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
            matching: self.new_query,
        }
    }
}

/// What a match job matches the query against.
pub(crate) struct Candidates {
    /// Every file in the project (relative paths), for the file modes.
    pub(crate) files: Arc<[String]>,
    /// The files opened lately, most recent first.
    pub(crate) recent: Vec<String>,
}

/// Matches `query` against the candidates (a background thread).
pub(crate) fn run_match(mode: FinderMode, query: &str, candidates: &Candidates) -> Vec<FinderItem> {
    let mut scorer = Scorer::new(query);
    match mode {
        // An empty query lists the recent files.
        FinderMode::Files | FinderMode::Everywhere if scorer.is_empty() => recent_files(&mut scorer, candidates),
        FinderMode::Files => best(files(&mut scorer, candidates)),
        FinderMode::RecentFiles => recent_files(&mut scorer, candidates),
        FinderMode::Actions => best(actions(&mut scorer)),
        FinderMode::Everywhere => {
            let mut matched = files(&mut scorer, candidates);
            matched.extend(actions(&mut scorer));
            best(matched)
        }
    }
}

/// A match: its score and what matched.
type Scored<'a> = (u32, Hit<'a>);

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Hit<'a> {
    /// A file: (path length, path), so shorter paths win a tie.
    File(usize, &'a str),
    /// An action, by its place in the menus. On a tie, actions go after
    /// files.
    Action(usize),
}

/// The best matches as results, best first.
fn best<'a>(mut matched: Vec<Scored<'a>>) -> Vec<FinderItem> {
    let key = |(score, hit): &Scored<'a>| (Reverse(*score), *hit);
    // Sorting only the best is quicker on a long list.
    if matched.len() > MAX_RESULTS {
        matched.select_nth_unstable_by_key(MAX_RESULTS, key);
        matched.truncate(MAX_RESULTS);
    }
    matched.sort_unstable_by_key(key);
    matched
        .into_iter()
        .map(|(_, hit)| match hit {
            Hit::File(_, path) => file_item(Path::new(path)),
            Hit::Action(i) => action_item(Action::ALL[i]),
        })
        .collect()
}

/// The files that match.
fn files<'a>(scorer: &mut Scorer, candidates: &'a Candidates) -> Vec<Scored<'a>> {
    let matching = candidates.files.iter().filter_map(|path| Some((scorer.path(path)?, Hit::File(path.len(), path))));
    matching.collect()
}

/// The actions that match.
fn actions(scorer: &mut Scorer) -> Vec<Scored<'static>> {
    let matching = Action::ALL.iter().enumerate().filter_map(|(i, action)| Some((scorer.text(action.name())?, Hit::Action(i))));
    matching.collect()
}

/// An action as a result, with its shortcut beside it.
fn action_item(action: Action) -> FinderItem {
    FinderItem {
        label: action.name().to_owned(),
        detail: String::new(),
        shortcut: action.shortcut().map(str::to_owned),
        kind: FinderItemKind::Action(action),
    }
}

/// The recent files that match, in their order: most recently opened
/// first.
fn recent_files(scorer: &mut Scorer, candidates: &Candidates) -> Vec<FinderItem> {
    let matching = candidates.recent.iter().filter(|path| scorer.path(path).is_some());
    matching.take(MAX_RESULTS).map(|path| file_item(Path::new(path))).collect()
}

/// Scores text against the query.
struct Scorer {
    pattern: Pattern,
    /// Prefers matches that start after a `/`.
    paths: Matcher,
    /// For names.
    text: Matcher,
    buf: Vec<char>,
}

impl Scorer {
    fn new(query: &str) -> Self {
        let pattern = Pattern::new(query, CaseMatching::Smart, Normalization::Smart, AtomKind::Fuzzy);
        let (paths, text) = (Matcher::new(Config::DEFAULT.match_paths()), Matcher::new(Config::DEFAULT));
        Scorer { pattern, paths, text, buf: Vec::new() }
    }

    /// A name's score, or `None` if it doesn't match.
    fn text(&mut self, text: &str) -> Option<u32> {
        self.pattern.score(Utf32Str::new(text, &mut self.buf), &mut self.text)
    }

    /// The query is empty (or only spaces): everything matches.
    fn is_empty(&self) -> bool {
        self.pattern.atoms.is_empty()
    }

    /// A path's score, or `None` if it doesn't match.
    fn path(&mut self, path: &str) -> Option<u32> {
        self.pattern.score(Utf32Str::new(path, &mut self.buf), &mut self.paths)
    }
}

/// A file as a result: its name, with its folder beside it.
fn file_item(path: &Path) -> FinderItem {
    let label = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let detail = path.parent().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default();
    FinderItem { label, detail, shortcut: None, kind: FinderItemKind::File(PathBuf::from(path)) }
}
