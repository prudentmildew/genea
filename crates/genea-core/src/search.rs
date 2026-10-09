//! Project search (⌘⇧F, ticket #34).
//!
//! A search walks the project on a background thread with `ignore` (so
//! `.gitignore` applies, with or without a git repo), skipping
//! `node_modules`, `.git` and the config's `exclude`, and matches each file
//! with ripgrep's searcher, several files at a time (opening files one by
//! one is what costs on a large project). Results stream back to the main
//! thread: whenever the main thread has taken the last batch, the next one
//! goes, so the first match shows as soon as its file is searched and the
//! rest follow at the pace the main thread takes them. Each file goes into
//! the list at its place in the project's tree.
//!
//! A new query cancels the search in flight (a flag the walk checks before
//! every file and after every match) and drops anything it still sends.

use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use grep_matcher::{LineTerminator, Matcher};
use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::{
    WalkState,
    gitignore::{Gitignore, GitignoreBuilder},
};

use crate::{
    command::SearchQuery,
    files::is_hidden_name,
    jobs::Jobs,
    problems::TextPosition,
    view::{SearchFile, SearchMatch, SearchView},
    workbench::ProjectId,
};

/// A search stops after this many matches; the view says the list is cut
/// short.
pub const MAX_SEARCH_MATCHES: usize = 10_000;

/// How much of a match's line a result shows, in chars, before and after
/// the match (and of the match itself).
const BEFORE_CHARS: usize = 24;
const MATCH_CHARS: usize = 200;
const AFTER_CHARS: usize = 120;

pub(crate) struct Search {
    id: ProjectId,
    root: PathBuf,
    query: SearchQuery,
    /// Bumped by every query, so a cancelled search's results are dropped.
    generation: u64,
    /// Set to cancel the search in flight.
    cancel: Arc<AtomicBool>,
    files: Vec<SearchFile>,
    /// `files` as last shown, shared with view snapshots.
    shown: Arc<[SearchFile]>,
    match_count: usize,
    searching: bool,
    limited: bool,
    error: Option<String>,
}

/// Results the walk has found that the main thread hasn't taken yet.
#[derive(Default)]
struct Pending {
    files: Vec<SearchFile>,
    /// A batch is on its way to the main thread.
    posted: bool,
}

impl Search {
    pub(crate) fn new(id: ProjectId, root: PathBuf) -> Self {
        Search {
            id,
            root,
            query: SearchQuery::default(),
            generation: 0,
            cancel: Arc::default(),
            files: Vec::new(),
            shown: Arc::from([]),
            match_count: 0,
            searching: false,
            limited: false,
            error: None,
        }
    }

    pub(crate) fn view(&self) -> SearchView {
        SearchView {
            query: self.query.clone(),
            files: self.shown.clone(),
            match_count: self.match_count,
            searching: self.searching,
            limited: self.limited,
            error: self.error.clone(),
        }
    }

    /// Starts searching for `query`, cancelling the search in flight. An
    /// empty query just clears the results.
    pub(crate) fn start(&mut self, query: SearchQuery, exclude: &[String], jobs: &Jobs) {
        self.cancel.store(true, Ordering::Relaxed);
        self.cancel = Arc::default();
        self.generation += 1;
        self.files.clear();
        self.shown = Arc::from([]);
        self.match_count = 0;
        self.limited = false;
        self.error = None;
        self.searching = false;
        self.query = query;
        if self.query.text.is_empty() {
            return;
        }
        let matcher = match matcher(&self.query) {
            Ok(matcher) => matcher,
            Err(error) => {
                self.error = Some(error);
                return;
            }
        };
        self.searching = true;
        let walk = Walk { root: self.root.clone(), exclude: exclude_matcher(&self.root, exclude), matcher };
        let (id, generation, cancel) = (self.id, self.generation, self.cancel.clone());
        let pending = Arc::new(Mutex::new(Pending::default()));
        let sender = jobs.clone();
        jobs.spawn("search", move || {
            let pending_for_walk = pending.clone();
            let limited = walk.run(&cancel, &|file| {
                let mut queue = pending_for_walk.lock().unwrap();
                queue.files.push(file);
                if !std::mem::replace(&mut queue.posted, true) {
                    let pending = pending_for_walk.clone();
                    sender.busy().finish(Box::new(move |core| {
                        let Some(project) = core.project_mut(id) else { return };
                        project.search.take(generation, &pending, None);
                    }));
                }
            });
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                project.search.take(generation, &pending, Some(limited));
            })
        })
    }

    /// Takes the walk's latest results (main thread). `done` is set by the
    /// walk's last batch: whether it stopped at the match limit.
    fn take(&mut self, generation: u64, pending: &Mutex<Pending>, done: Option<bool>) {
        let files = {
            let mut queue = pending.lock().unwrap();
            queue.posted = false;
            std::mem::take(&mut queue.files)
        };
        if generation != self.generation {
            return;
        }
        if !files.is_empty() {
            for file in files {
                self.match_count += file.matches.len();
                let at = self.files.partition_point(|f| tree_order(&f.path, &file.path).is_lt());
                self.files.insert(at, file);
            }
            self.shown = self.files.as_slice().into();
        }
        if let Some(limited) = done {
            self.searching = false;
            self.limited = limited;
        }
    }
}

impl Drop for Search {
    /// A closed project's search stops.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The order of the project's tree (the Files view): folder by folder,
/// folders before files, names case-insensitively.
fn tree_order(a: &Path, b: &Path) -> std::cmp::Ordering {
    let (mut a, mut b) = (a.components().peekable(), b.components().peekable());
    loop {
        let (Some(x), Some(y)) = (a.next(), b.next()) else { return a.peek().cmp(&b.peek()) };
        // A name with more after it is a folder.
        let (x_file, y_file) = (a.peek().is_none(), b.peek().is_none());
        let (x, y) = (x.as_os_str().to_string_lossy(), y.as_os_str().to_string_lossy());
        let order = x_file.cmp(&y_file).then_with(|| x.to_lowercase().cmp(&y.to_lowercase())).then_with(|| x.cmp(&y));
        if order.is_ne() {
            return order;
        }
    }
}

/// The matcher for a query, or why the query can't be searched for.
fn matcher(query: &SearchQuery) -> Result<RegexMatcher, String> {
    RegexMatcherBuilder::new()
        .fixed_strings(!query.regex)
        .case_insensitive(!query.case_sensitive)
        .word(query.whole_word)
        // A match never spans lines, and `$` matches before `\r\n` too.
        .crlf(true)
        .build(&query.text)
        .map_err(|error| error.to_string())
}

fn exclude_matcher(root: &Path, patterns: &[String]) -> Gitignore {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        // The config only keeps patterns that build.
        let _ = builder.add_line(None, pattern);
    }
    builder.build().unwrap_or_else(|_| Gitignore::empty())
}

/// One search's walk of the project (background threads).
struct Walk {
    root: PathBuf,
    exclude: Gitignore,
    matcher: RegexMatcher,
}

impl Walk {
    /// Searches every file, several at a time, passing each file with
    /// matches to `found`. Returns whether it stopped at
    /// [`MAX_SEARCH_MATCHES`].
    fn run(self, cancel: &AtomicBool, found: &(dyn Fn(SearchFile) + Sync)) -> bool {
        let root = self.root.clone();
        let exclude = self.exclude;
        let walk = ignore::WalkBuilder::new(&self.root)
            // `.gitignore` files in the project (and `.git/info/exclude`),
            // whether or not it is a git repo; nothing above the root, and
            // nothing user-wide, so a project searches the same everywhere.
            .hidden(false)
            .parents(false)
            .ignore(false)
            .git_global(false)
            .require_git(false)
            .filter_entry(move |entry| {
                if is_hidden_name(entry.file_name()) {
                    return false;
                }
                let Ok(relative) = entry.path().strip_prefix(&root) else { return true };
                let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
                relative.as_os_str().is_empty() || !exclude.matched(relative, is_dir).is_ignore()
            })
            .build_parallel();
        let remaining = AtomicUsize::new(MAX_SEARCH_MATCHES);
        let (root, matcher, remaining) = (&self.root, &self.matcher, &remaining);
        walk.run(|| {
            let mut searcher = SearcherBuilder::new()
                .line_terminator(LineTerminator::crlf())
                .line_number(true)
                .binary_detection(BinaryDetection::quit(0))
                .build();
            let matcher = matcher.clone();
            Box::new(move |entry| {
                if cancel.load(Ordering::Relaxed) || remaining.load(Ordering::Relaxed) == 0 {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                let Ok(path) = entry.path().strip_prefix(root) else { return WalkState::Continue };
                let mut sink = FileSink { matcher: &matcher, cancel, remaining, matches: Vec::new() };
                // An unreadable file has no results.
                let _ = searcher.search_path(&matcher, entry.path(), &mut sink);
                if !sink.matches.is_empty() {
                    found(SearchFile { path: path.to_owned(), matches: sink.matches });
                }
                WalkState::Continue
            })
        });
        remaining.load(Ordering::Relaxed) == 0
    }
}

/// Collects one file's matches.
struct FileSink<'a> {
    matcher: &'a RegexMatcher,
    cancel: &'a AtomicBool,
    /// Matches left before the limit, shared by every file.
    remaining: &'a AtomicUsize,
    matches: Vec<SearchMatch>,
}

impl Sink for FileSink<'_> {
    type Error = io::Error;

    fn matched(&mut self, _: &Searcher, found: &SinkMatch<'_>) -> Result<bool, io::Error> {
        let line_index = found.line_number().map_or(0, |n| n as usize - 1);
        let mut line = found.bytes();
        while let [rest @ .., b'\n' | b'\r'] = line {
            line = rest;
        }
        let mut more = true;
        self.matcher
            .find_iter(line, |range| {
                // Takes one of the matches left, if there is one.
                more = self.remaining.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1)).is_ok();
                if more {
                    self.matches.push(search_match(line, line_index, range.start(), range.end()));
                }
                more
            })
            .map_err(|error| io::Error::other(error.to_string()))?;
        Ok(more && !self.cancel.load(Ordering::Relaxed))
    }
}

/// A result for the match at `start..end` (bytes) in a line.
fn search_match(line: &[u8], line_index: usize, start: usize, end: usize) -> SearchMatch {
    let before = String::from_utf8_lossy(&line[..start]);
    let matched = String::from_utf8_lossy(&line[start..end]);
    let after = String::from_utf8_lossy(&line[end..]);
    let column = before.chars().count();
    SearchMatch {
        position: TextPosition { line: line_index, column },
        location: format!("{}:{}", line_index + 1, column + 1),
        before: tail(before.trim_start(), BEFORE_CHARS),
        matched: head(&matched, MATCH_CHARS),
        after: head(after.trim_end(), AFTER_CHARS),
    }
}

/// The last `max` chars of `text`, after an ellipsis if it is longer.
fn tail(text: &str, max: usize) -> String {
    let count = text.chars().count();
    let text = untab(text);
    if count <= max { text } else { format!("…{}", text.chars().skip(count - max).collect::<String>()) }
}

/// The first `max` chars of `text`, before an ellipsis if it is longer.
fn head(text: &str, max: usize) -> String {
    let text = untab(text);
    if text.chars().count() <= max { text } else { format!("{}…", text.chars().take(max).collect::<String>()) }
}

/// Tabs inside a result's text show as single spaces.
fn untab(text: &str) -> String {
    text.replace('\t', " ")
}
