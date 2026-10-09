//! Review of external changes (spec #19, Review and external changes;
//! ticket #53).
//!
//! Every in-scope file ([`scope`]) has a *review baseline*: its content as
//! of the user's last review, kept in the project's baseline store
//! ([`store`]). The first open snapshots every in-scope file in the
//! background. When the watcher reports a change, the file is hashed and
//! compared with its baseline; one that differs is listed in Changes as
//! modified, created or deleted. Keep makes the disk content the baseline;
//! Revert writes the baseline back (deleting a created file, restoring a
//! deleted one).
//!
//! Genea's own writes ([`own`]) are not external changes: one to a file
//! without a pending change moves its baseline forward; one to a file with a
//! pending change leaves the baseline alone, so the file stays listed.
//!
//! **Order.** Everything that reads or writes the disk or the store (the
//! first snapshot, checks, Keep, Revert, writing the index) is an [`Op`],
//! and ops run one at a time in the background, in the order they were
//! asked for. So a check never races a Keep or a Revert of the same file.

mod own;
mod scope;
pub(crate) mod store;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, VecDeque},
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) use own::OwnWrites;
use scope::Scope;
use store::{Budget, FileState, Hash, LARGE_BLOBS_CAP, Store, read_file, store_file};

use crate::{
    jobs::Jobs,
    view::{ChangeItem, ChangeKind},
    watcher::FileChanges,
    workbench::ProjectId,
};

pub(crate) struct Review {
    id: ProjectId,
    root: PathBuf,
    own: OwnWrites,
    /// Set by `start`.
    store: Option<Arc<Store>>,
    scope: Arc<Scope>,
    /// Path (relative) → its baseline. A file without one isn't in it.
    baseline: BTreeMap<PathBuf, FileState>,
    /// Blobs the baseline uses: hash → (how many paths, size).
    blobs: HashMap<Hash, (usize, u64)>,
    /// Blobs no longer used, to remove when the index is next written.
    unreferenced: Vec<Hash>,
    /// The baseline changed since the index was last written.
    dirty: bool,
    /// Files whose disk content differs from their baseline.
    changes: BTreeMap<PathBuf, Change>,
    /// The Changes view's rows, rebuilt when `changes` changes.
    rows: Arc<[ChangeItem]>,
    ops: VecDeque<Op>,
    /// An op's job is running.
    running: bool,
}

/// A pending change: the file's disk state differs from its baseline.
#[derive(Clone, Copy)]
struct Change {
    /// `None`: the file is gone.
    disk: Option<FileState>,
}

/// Work on the disk and the store, run one at a time.
enum Op {
    /// Loads the index, or snapshots every file on a first open.
    Open,
    /// Compares these paths (relative; files or folders) with the baseline.
    Check(BTreeSet<PathBuf>),
    /// Compares every file with the baseline, finding the scope afresh.
    Rescan,
    Keep(BTreeSet<PathBuf>),
    Revert(BTreeSet<PathBuf>),
    /// Writes the index and removes unused blobs.
    Persist,
}

/// An op's result, applied on the main thread.
pub(crate) struct Done(Finished);

enum Finished {
    Opened { scope: Scope, baseline: BTreeMap<PathBuf, FileState>, snapshot: bool },
    Observed { observed: Vec<(PathBuf, Option<FileState>)> },
    Rescanned { scope: Scope, observed: Vec<(PathBuf, Option<FileState>)>, dropped: Vec<PathBuf> },
    Kept { kept: Vec<(PathBuf, Option<FileState>)>, failed: Vec<String> },
    Reverted { reverted: Vec<(PathBuf, Option<FileState>)>, failed: Vec<String> },
    Persisted,
}

/// What the project does after an op: reload open editors of reverted
/// files, and show errors.
#[derive(Default)]
pub(crate) struct Outcome {
    pub(crate) reverted: Vec<PathBuf>,
    pub(crate) errors: Vec<String>,
}

impl Review {
    pub(crate) fn new(id: ProjectId, root: PathBuf, jobs: &Jobs) -> Self {
        Review {
            id,
            own: OwnWrites::new(id, jobs),
            store: None,
            scope: Arc::new(Scope::new(root.clone())),
            root,
            baseline: BTreeMap::new(),
            blobs: HashMap::new(),
            unreferenced: Vec::new(),
            dirty: false,
            changes: BTreeMap::new(),
            rows: Arc::from([]),
            ops: VecDeque::new(),
            running: false,
        }
    }

    /// Opens the project's baseline store in the background: loads it, or
    /// on a first open snapshots every file in scope.
    pub(crate) fn start(&mut self, support_dir: &Path, jobs: &Jobs) {
        self.store = Some(Arc::new(Store::new(support_dir, &self.root)));
        self.push(Op::Open, jobs);
    }

    /// Where Genea announces its own writes to the project's files.
    pub(crate) fn own_writes(&self) -> OwnWrites {
        self.own.clone()
    }

    /// The watcher reported changes.
    pub(crate) fn files_changed(&mut self, changes: &FileChanges, jobs: &Jobs) {
        if changes.rescan {
            return self.push(Op::Rescan, jobs);
        }
        let paths: BTreeSet<PathBuf> = changes
            .paths
            .iter()
            .filter_map(|path| path.strip_prefix(&self.root).ok())
            .filter(|path| !path.as_os_str().is_empty() && scope::may_include(path))
            .map(Path::to_path_buf)
            .collect();
        if !paths.is_empty() {
            self.push(Op::Check(paths), jobs);
        }
    }

    /// Compares one path (relative) with its baseline, after what is queued.
    pub(crate) fn check(&mut self, path: PathBuf, jobs: &Jobs) {
        if scope::may_include(&path) && path.is_relative() {
            self.push(Op::Check(BTreeSet::from([path])), jobs);
        }
    }

    /// Keep: makes the disk content of these listed files (every listed
    /// file for `None`) their baseline.
    pub(crate) fn keep(&mut self, paths: Option<PathBuf>, jobs: &Jobs) {
        let paths = self.listed(paths);
        if !paths.is_empty() {
            self.push(Op::Keep(paths), jobs);
        }
    }

    /// Revert: writes the baseline of these listed files (every listed file
    /// for `None`) back to disk.
    pub(crate) fn revert(&mut self, paths: Option<PathBuf>, jobs: &Jobs) {
        let paths = self.listed(paths);
        if !paths.is_empty() {
            self.push(Op::Revert(paths), jobs);
        }
    }

    fn listed(&self, path: Option<PathBuf>) -> BTreeSet<PathBuf> {
        match path {
            Some(path) => self.changes.contains_key(&path).then_some(path).into_iter().collect(),
            None => self.changes.keys().cloned().collect(),
        }
    }

    /// The Changes view's rows.
    pub(crate) fn rows(&self) -> Arc<[ChangeItem]> {
        self.rows.clone()
    }

    /// The banner shown while unreviewed changes exist.
    pub(crate) fn banner(&self) -> Option<String> {
        match self.changes.len() {
            0 => None,
            1 => Some("1 file changed outside Genea".into()),
            n => Some(format!("{n} files changed outside Genea")),
        }
    }

    fn push(&mut self, op: Op, jobs: &Jobs) {
        let queued = |kind: fn(&Op) -> bool| self.ops.iter().any(kind);
        match op {
            Op::Persist if queued(|op| matches!(op, Op::Persist)) => {}
            Op::Rescan if queued(|op| matches!(op, Op::Rescan)) => {}
            Op::Check(paths) if let Some(Op::Check(queued)) = self.ops.back_mut() => queued.extend(paths),
            op => self.ops.push_back(op),
        }
        self.run_next(jobs);
    }

    /// Starts the next op, unless one is running.
    fn run_next(&mut self, jobs: &Jobs) {
        if self.running {
            return;
        }
        let Some(store) = self.store.clone() else { return };
        let Some(op) = self.ops.pop_front() else { return };
        self.running = true;
        let root = self.root.clone();
        let own = self.own.clone();
        let scope = self.scope.clone();
        let mut budget = Budget { large_bytes: LARGE_BLOBS_CAP.saturating_sub(self.stored_bytes()) };
        let work: Box<dyn FnOnce() -> Finished + Send> = match op {
            Op::Open => Box::new(move || open(&root, &store)),
            Op::Check(paths) => {
                let paths: Vec<(PathBuf, Vec<PathBuf>)> =
                    paths.into_iter().map(|path| (path.clone(), self.known_under(&path))).collect();
                Box::new(move || {
                    let mut check = Checker { root: &root, store: &store, own: &own, budget: &mut budget, observed: Vec::new() };
                    match check.paths(&scope, paths) {
                        Some(()) => Finished::Observed { observed: check.observed },
                        None => check.rescan(Vec::new()),
                    }
                })
            }
            Op::Rescan => {
                let known: Vec<PathBuf> = self.baseline.keys().chain(self.changes.keys()).cloned().collect();
                Box::new(move || {
                    let check = Checker { root: &root, store: &store, own: &own, budget: &mut budget, observed: Vec::new() };
                    check.rescan(known)
                })
            }
            Op::Keep(paths) => {
                let paths: Vec<PathBuf> = paths.into_iter().filter(|p| self.changes.contains_key(p)).collect();
                Box::new(move || keep(&root, &store, &mut budget, paths))
            }
            Op::Revert(paths) => {
                let items: Vec<(PathBuf, Option<FileState>)> = paths
                    .into_iter()
                    .filter(|path| self.changes.contains_key(path))
                    .map(|path| {
                        let baseline = self.baseline.get(&path).copied();
                        (path, baseline)
                    })
                    .filter(|(_, baseline)| baseline.is_none_or(|b| b.stored))
                    .collect();
                Box::new(move || revert(&root, &store, &own, items))
            }
            Op::Persist => {
                let baseline = self.baseline.clone();
                let blobs = &self.blobs;
                let mut unused = std::mem::take(&mut self.unreferenced);
                unused.retain(|hash| !blobs.contains_key(hash));
                Box::new(move || {
                    if store.write_index(&root, &baseline).is_ok() {
                        for hash in &unused {
                            store.remove_blob(hash);
                        }
                    }
                    Finished::Persisted
                })
            }
        };
        let id = self.id;
        jobs.spawn("review", move || {
            let done = Done(work());
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                if let Some(project) = core.project_mut(id) {
                    project.review_done(done, &jobs);
                }
            })
        });
    }

    /// Takes an op's result. The project then reloads reverted files' open
    /// editors and shows errors.
    pub(crate) fn done(&mut self, Done(finished): Done, jobs: &Jobs) -> Outcome {
        self.running = false;
        let mut outcome = Outcome::default();
        match finished {
            Finished::Opened { scope, baseline, snapshot } => {
                self.scope = Arc::new(scope);
                for (path, state) in baseline {
                    self.set_baseline(path, Some(state));
                }
                self.dirty = snapshot;
            }
            Finished::Observed { observed } => {
                for (path, disk) in observed {
                    self.observe(path, disk);
                }
            }
            Finished::Rescanned { scope, observed, dropped } => {
                self.scope = Arc::new(scope);
                for path in dropped {
                    self.changes.remove(&path);
                    self.set_baseline(path, None);
                }
                for (path, disk) in observed {
                    self.observe(path, disk);
                }
            }
            Finished::Kept { kept, failed } => {
                for (path, disk) in kept {
                    self.own.forget(&path);
                    self.changes.remove(&path);
                    self.set_baseline(path, disk);
                }
                outcome.errors = failed;
            }
            Finished::Reverted { reverted, failed } => {
                for (path, disk) in reverted {
                    self.observe(path.clone(), disk);
                    outcome.reverted.push(path);
                }
                outcome.errors = failed;
            }
            Finished::Persisted => {}
        }
        if std::mem::take(&mut self.dirty) {
            self.ops.push_back(Op::Persist);
        }
        self.rebuild_rows();
        self.run_next(jobs);
        outcome
    }

    /// Takes a file's disk state (`None`: gone) as read after a change.
    fn observe(&mut self, path: PathBuf, disk: Option<FileState>) {
        let baseline = self.baseline.get(&path).copied();
        let hash = disk.map(|d| d.hash);
        if baseline.map(|b| b.hash) == hash {
            self.own.seen(&path, hash);
            self.changes.remove(&path);
            return;
        }
        if self.own.is_own(&path, hash) {
            self.own.seen(&path, hash);
            if !self.changes.contains_key(&path) {
                // Genea's own write to a file without a pending change.
                self.set_baseline(path, disk);
                return;
            }
        }
        if let Some(disk) = disk.filter(|d| d.stored && !self.blobs.contains_key(&d.hash)) {
            self.unreferenced.push(disk.hash);
        }
        self.changes.insert(path, Change { disk });
    }

    /// Replaces a path's baseline (`None`: the file has none), keeping
    /// count of the blobs in use.
    fn set_baseline(&mut self, path: PathBuf, state: Option<FileState>) {
        let old = match state {
            Some(state) => self.baseline.insert(path, state),
            None => self.baseline.remove(&path),
        };
        if old == state {
            return;
        }
        if let Some(state) = state.filter(|s| s.stored) {
            self.blobs.entry(state.hash).or_insert((0, state.size)).0 += 1;
        }
        if let Some(old) = old.filter(|s| s.stored)
            && let Some((count, _)) = self.blobs.get_mut(&old.hash)
        {
            *count -= 1;
            if *count == 0 {
                self.blobs.remove(&old.hash);
                self.unreferenced.push(old.hash);
            }
        }
        self.dirty = true;
    }

    fn stored_bytes(&self) -> u64 {
        self.blobs.values().map(|(_, size)| size).sum()
    }

    /// Every path the baseline or Changes has at `path` or below it.
    fn known_under(&self, path: &Path) -> Vec<PathBuf> {
        let from = path.to_path_buf();
        let baseline = self.baseline.range(from.clone()..).map(|(key, _)| key);
        let changes = self.changes.range(from..).map(|(key, _)| key);
        let mut known: BTreeSet<&PathBuf> = baseline.take_while(|key| key.starts_with(path)).collect();
        known.extend(changes.take_while(|key| key.starts_with(path)));
        known.into_iter().cloned().collect()
    }

    fn rebuild_rows(&mut self) {
        let rows: Vec<ChangeItem> = self
            .changes
            .iter()
            .map(|(path, change)| {
                let baseline = self.baseline.get(path);
                let kind = match (baseline, change.disk) {
                    (None, _) => ChangeKind::Created,
                    (Some(_), None) => ChangeKind::Deleted,
                    (Some(_), Some(_)) => ChangeKind::Modified,
                };
                ChangeItem {
                    path: path.clone(),
                    kind,
                    diffable: baseline.is_none_or(FileState::diffable)
                        && change.disk.as_ref().is_none_or(FileState::diffable),
                    can_revert: baseline.is_none_or(|b| b.stored),
                }
            })
            .collect();
        if *rows != *self.rows {
            self.rows = rows.into();
        }
    }
}

/// Loads the store's index, or snapshots every file in scope into the
/// store if there is none (a first open). Background thread.
fn open(root: &Path, store: &Store) -> Finished {
    let _ = store.create(root);
    let mut scope = Scope::new(root.to_owned());
    let (files, _) = scope.walk(Path::new(""));
    if let Some(baseline) = store.read_index() {
        return Finished::Opened { scope, baseline, snapshot: false };
    }
    let mut budget = Budget { large_bytes: LARGE_BLOBS_CAP };
    let baseline = files
        .into_iter()
        .filter_map(|path| {
            let state = read_file(&root.join(&path), Some((store, &mut budget))).ok().flatten()?;
            Some((path, state))
        })
        .collect();
    Finished::Opened { scope, baseline, snapshot: true }
}

/// Reads changed files (background thread).
struct Checker<'a> {
    root: &'a Path,
    store: &'a Store,
    own: &'a OwnWrites,
    budget: &'a mut Budget,
    observed: Vec<(PathBuf, Option<FileState>)>,
}

impl Checker<'_> {
    /// Reads each changed path, with the baseline paths at or below it.
    /// Returns `None` if the scope may have changed (a `.gitignore` did), so
    /// a rescan is needed.
    fn paths(&mut self, scope: &Scope, paths: Vec<(PathBuf, Vec<PathBuf>)>) -> Option<()> {
        if paths.iter().any(|(path, _)| scope::is_gitignore(path)) {
            return None;
        }
        for (path, known) in paths {
            match fs::symlink_metadata(self.root.join(&path)) {
                Ok(metadata) if metadata.is_dir() => {
                    if known.contains(&path) {
                        self.gone(path.clone());
                    }
                    // A folder with known files under it reports its files'
                    // changes itself; a new one (created, or moved in) is
                    // read with everything in it.
                    if known.iter().any(|k| *k != path) || scope.excludes(&path, true) {
                        continue;
                    }
                    let mut scope = scope.clone();
                    let (files, found_ignores) = scope.walk(&path);
                    if found_ignores {
                        return None;
                    }
                    for file in files {
                        self.file(file);
                    }
                }
                Ok(metadata) if metadata.is_file() => {
                    if !scope.excludes(&path, false) {
                        self.file(path);
                    }
                }
                Err(error) if error.kind() != io::ErrorKind::NotFound => {}
                // Gone, or no longer a regular file: so is everything
                // review knew there.
                _ => {
                    for path in known {
                        self.gone(path);
                    }
                }
            }
        }
        Some(())
    }

    /// Reads every file in scope, finding the scope afresh, and compares
    /// them and every `known` path with the baseline.
    fn rescan(mut self, known: Vec<PathBuf>) -> Finished {
        let mut scope = Scope::new(self.root.to_owned());
        let (files, _) = scope.walk(Path::new(""));
        let files: BTreeSet<PathBuf> = files.into_iter().collect();
        let mut dropped = Vec::new();
        for path in known {
            if files.contains(&path) {
                continue;
            }
            if scope.excludes(&path, false) {
                dropped.push(path);
            } else {
                self.gone(path);
            }
        }
        for file in files {
            self.file(file);
        }
        Finished::Rescanned { scope, observed: self.observed, dropped }
    }

    /// Reads a file that changed. Skipped if Genea wrote it meanwhile: that
    /// write's end checks it again.
    fn file(&mut self, path: PathBuf) {
        let Some(generation) = self.own.generation(&path) else { return };
        let Ok(mut state) = read_file(&self.root.join(&path), None) else { return };
        if let Some(read) = &mut state
            && self.own.is_own(&path, Some(read.hash))
        {
            read.stored = store_file(&self.root.join(&path), &read.hash, self.store, self.budget);
        }
        if self.own.generation(&path) == Some(generation) {
            self.observed.push((path, state));
        }
    }

    fn gone(&mut self, path: PathBuf) {
        if self.own.generation(&path).is_some() {
            self.observed.push((path, None));
        }
    }
}

/// Reads the listed files into the store for Keep (background thread).
fn keep(root: &Path, store: &Store, budget: &mut Budget, paths: Vec<PathBuf>) -> Finished {
    let (mut kept, mut failed) = (Vec::new(), Vec::new());
    for path in paths {
        match read_file(&root.join(&path), Some((store, budget))) {
            Ok(state) => kept.push((path, state)),
            Err(error) => failed.push(format!("Couldn't keep {}: {error}", path.display())),
        }
    }
    Finished::Kept { kept, failed }
}

/// Writes each file's baseline back, or deletes it if it has none
/// (background thread). Each write is Genea's own.
fn revert(root: &Path, store: &Store, own: &OwnWrites, items: Vec<(PathBuf, Option<FileState>)>) -> Finished {
    let (mut reverted, mut failed) = (Vec::new(), Vec::new());
    for (path, baseline) in items {
        let absolute = root.join(&path);
        let written = match baseline {
            Some(state) => store.read_blob(&state.hash).and_then(|bytes| {
                let _writing = own.writing(&path, Some(state.hash));
                if let Some(folder) = absolute.parent() {
                    fs::create_dir_all(folder)?;
                }
                fs::write(&absolute, bytes)
            }),
            None => {
                let _writing = own.writing(&path, None);
                match fs::remove_file(&absolute) {
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    removed => removed,
                }
            }
        };
        match written {
            Ok(()) => reverted.push((path, baseline)),
            Err(error) => failed.push(format!("Couldn't revert {}: {error}", path.display())),
        }
    }
    Finished::Reverted { reverted, failed }
}
