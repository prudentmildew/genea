//! Read-only git (ticket #56; spec #19, Git): the current branch, and the
//! open files' text at HEAD for the gutter markers. Read in-process with
//! `gix`, so nothing needs to be installed (ADR 0005), and always on a
//! background job: each read opens the repository afresh.
//!
//! **Markers.** Each open file in a repository has its *base*, the file at
//! HEAD. A [`DiffJob`] compares the base's lines with a snapshot of the
//! buffer on a background thread; one runs per file at a time, and the next
//! starts when it lands if the text (or the base) moved on meanwhile, like
//! the syntax parse. Until then the last result is shown, so the markers can
//! be a keystroke behind, but typing never waits for them.

use std::{
    borrow::Cow,
    collections::HashMap,
    ops::Range,
    path::{Path, PathBuf},
};

use imara_diff::{Algorithm, Diff, InternedInput};
use ropey::Rope;

use crate::{
    jobs::Jobs,
    view::{GutterMark, HunkView, LineChange},
    watcher::FileChanges,
    workbench::ProjectId,
};

/// A project's git state. A project outside any repository has none, and
/// shows no branch and no markers.
pub(crate) struct Git {
    project: ProjectId,
    root: PathBuf,
    /// The branch HEAD is on, or the short commit id while detached.
    branch: Option<String>,
    /// The repository's git folder, once a read has found one.
    git_dir: Option<PathBuf>,
    /// Bumped by every read of HEAD, so a slow read can't replace a newer one.
    generation: u64,
    /// Open files (and files open when HEAD was last read), by path
    /// relative to the root.
    files: HashMap<PathBuf, FileDiff>,
    /// Hands out base ids.
    last_base: u64,
}

/// One file's base and the hunks last worked out against it.
#[derive(Default)]
struct FileDiff {
    /// The file at HEAD; `None` if it isn't in HEAD (or isn't text), or
    /// there is no repository.
    base: Option<Rope>,
    /// Identifies `base`: every base read gets a fresh one.
    base_id: u64,
    /// How the buffer differed from the base, top to bottom.
    hunks: Vec<Hunk>,
    /// The buffer version and base id `hunks` are for.
    diffed: Option<(u64, u64)>,
    /// A diff job is running.
    running: bool,
}

/// A run of lines that differ from HEAD.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Hunk {
    /// The lines at HEAD (0-based, end exclusive); empty for added lines.
    pub(crate) head: Range<usize>,
    /// The buffer's lines; empty for deleted lines, which were just above
    /// `lines.start`.
    pub(crate) lines: Range<usize>,
}

impl Hunk {
    fn change(&self) -> LineChange {
        if self.lines.is_empty() {
            LineChange::Deleted
        } else if self.head.is_empty() {
            LineChange::Added
        } else {
            LineChange::Modified
        }
    }
}

/// What a read of HEAD found.
struct Head {
    branch: Option<String>,
    git_dir: Option<PathBuf>,
    /// The requested files at HEAD.
    bases: Vec<(PathBuf, Option<Rope>)>,
}

/// A diff to run in the background: a file's base against a snapshot of
/// its buffer.
pub(crate) struct DiffJob {
    base: Rope,
    text: Rope,
    version: u64,
    base_id: u64,
}

/// A finished [`DiffJob`].
pub(crate) struct Diffed {
    hunks: Vec<Hunk>,
    version: u64,
    base_id: u64,
}

impl Git {
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        Git { project, root, branch: None, git_dir: None, generation: 0, files: HashMap::new(), last_base: 0 }
    }

    pub(crate) fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    /// Whether these changes on disk (from the watcher) may have moved
    /// HEAD: a checkout or a commit rewrites `HEAD`, a ref or
    /// `packed-refs`, and `git init` creates `.git`. Other writes to the
    /// git folder (the index, objects, logs) don't. A repository whose git
    /// folder is outside the project isn't watched.
    pub(crate) fn head_may_have_moved(&self, changes: &FileChanges) -> bool {
        let default = self.root.join(".git");
        let git_dirs = [Some(&default), self.git_dir.as_ref()];
        changes.rescan
            || changes.paths.iter().any(|path| {
                git_dirs.iter().flatten().any(|dir| match path.strip_prefix(dir) {
                    Ok(inside) => {
                        let first = inside.components().next().map(|c| c.as_os_str().to_string_lossy());
                        match first.as_deref() {
                            None => true,
                            Some(name) => name.starts_with("HEAD") || name == "refs" || name.starts_with("packed-refs"),
                        }
                    }
                    Err(_) => false,
                })
            })
    }

    /// Reads HEAD again in the background, with the open files at HEAD: at
    /// open, and when HEAD may have moved (a checkout, a commit, `git
    /// init`). Then diffs those files again.
    pub(crate) fn reload(&mut self, open: Vec<PathBuf>, jobs: &Jobs) {
        self.generation += 1;
        let (id, generation, root) = (self.project, self.generation, self.root.clone());
        jobs.spawn("read git HEAD", move || {
            let head = read_head(&root, &open);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                let git = &mut project.git;
                if git.generation != generation {
                    return;
                }
                git.branch = head.branch;
                git.git_dir = head.git_dir;
                git.files.retain(|path, _| open.contains(path));
                let paths: Vec<PathBuf> = head.bases.iter().map(|(path, _)| path.clone()).collect();
                for (path, base) in head.bases {
                    git.set_base(path, base);
                }
                for path in paths {
                    project.diff_file(&path, &jobs);
                }
            })
        });
    }

    /// Reads a newly opened file at HEAD in the background, then diffs it.
    pub(crate) fn load_base(&mut self, path: &Path, jobs: &Jobs) {
        let (id, generation, root, path) = (self.project, self.generation, self.root.clone(), path.to_owned());
        jobs.spawn("read file at git HEAD", move || {
            let base = gix::discover(&root).ok().and_then(|repo| read_base(&repo, &root, &path));
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                // A newer read of HEAD has (or will have) this file's base.
                if project.git.generation != generation {
                    return;
                }
                project.git.set_base(path.clone(), base);
                project.diff_file(&path, &jobs);
            })
        });
    }

    fn set_base(&mut self, path: PathBuf, base: Option<Rope>) {
        self.last_base += 1;
        let file = self.files.entry(path).or_default();
        if base.is_none() {
            file.hunks.clear();
        }
        file.base = base;
        file.base_id = self.last_base;
    }

    /// A diff to run for `path`, if it has a base, its markers are behind
    /// the buffer (`version`, `text`) or the base, and none is running.
    pub(crate) fn start_diff(&mut self, path: &Path, version: u64, text: Rope) -> Option<DiffJob> {
        let file = self.files.get_mut(path)?;
        let base = file.base.clone()?;
        if file.running || file.diffed == Some((version, file.base_id)) {
            return None;
        }
        file.running = true;
        Some(DiffJob { base, text, version, base_id: file.base_id })
    }

    /// Takes a finished diff of `path`. Hunks against a base that has been
    /// replaced meanwhile are dropped.
    pub(crate) fn diffed(&mut self, path: &Path, diffed: Diffed) {
        let Some(file) = self.files.get_mut(path) else { return };
        file.running = false;
        if diffed.base_id == file.base_id {
            file.hunks = diffed.hunks;
            file.diffed = Some((diffed.version, diffed.base_id));
        }
    }

    /// The change marked on `line` of `path`, with its lines at HEAD, if
    /// the markers are up to date with the buffer (`version`).
    fn current_hunk(&self, path: &Path, version: u64, line: usize) -> Option<(&Hunk, &Rope)> {
        let file = self.files.get(path)?;
        let base = file.base.as_ref()?;
        if file.diffed != Some((version, file.base_id)) {
            return None;
        }
        let hunk = file.hunks.iter().find(|h| h.lines.contains(&line) || h.lines == (line..line))?;
        Some((hunk, base))
    }

    /// The change marked on `line` of `path`, as its popover shows it.
    pub(crate) fn hunk_view(&self, path: &Path, version: u64, line: usize) -> Option<HunkView> {
        let (hunk, base) = self.current_hunk(path, version, line)?;
        let head = head_text(hunk, base).lines().collect::<Vec<_>>().join("\n");
        Some(HunkView { lines: hunk.lines.clone(), change: hunk.change(), head })
    }

    /// The gutter markers of `path` on `lines`, top to bottom.
    pub(crate) fn gutter(&self, path: &Path, lines: Range<usize>) -> Vec<GutterMark> {
        let Some(file) = self.files.get(path).filter(|f| f.base.is_some()) else { return Vec::new() };
        let mut marks = Vec::new();
        for hunk in &file.hunks {
            let change = hunk.change();
            let marked = if hunk.lines.is_empty() { hunk.lines.start..hunk.lines.start + 1 } else { hunk.lines.clone() };
            for line in marked.start.max(lines.start)..marked.end.min(lines.end) {
                marks.push(GutterMark { line, change });
            }
        }
        marks
    }
}

impl DiffJob {
    pub(crate) fn run(self) -> Diffed {
        let mut input = InternedInput::default();
        input.update_before(lines(&self.base));
        input.update_after(lines(&self.text));
        let mut diff = Diff::compute(Algorithm::Histogram, &input);
        diff.postprocess_lines(&input);
        let hunks = diff
            .hunks()
            .map(|h| Hunk {
                head: h.before.start as usize..h.before.end as usize,
                lines: h.after.start as usize..h.after.end as usize,
            })
            .collect();
        Diffed { hunks, version: self.version, base_id: self.base_id }
    }
}

/// A hunk's lines at HEAD, with their line breaks.
fn head_text(hunk: &Hunk, base: &Rope) -> String {
    base.slice(base.line_to_char(hunk.head.start)..base.line_to_char(hunk.head.end)).to_string()
}

/// A line of text with its line break, as a diff token.
#[derive(Default, PartialEq, Eq, Hash)]
struct Line<'a>(Cow<'a, str>);

impl AsRef<[u8]> for Line<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

/// The text's lines as the editor counts them, each with its line break.
/// The empty line after a final line break isn't one.
fn lines(text: &Rope) -> impl Iterator<Item = Line<'_>> {
    text.lines().filter(|line| line.len_bytes() > 0).map(|line| Line(line.into()))
}

/// Reads the repository that `root` is in, if any, and the files at HEAD.
/// Every failure (no repository, a broken one, no commit yet) reads as no
/// repository, or as files not in HEAD.
fn read_head(root: &Path, files: &[PathBuf]) -> Head {
    let Ok(repo) = gix::discover(root) else {
        return Head { branch: None, git_dir: None, bases: files.iter().map(|path| (path.clone(), None)).collect() };
    };
    let branch = match repo.head_name() {
        Ok(Some(name)) => Some(name.shorten().to_string()),
        Ok(None) => repo.head_id().ok().map(|id| id.to_hex_with_len(7).to_string()),
        Err(_) => None,
    };
    let bases = files.iter().map(|path| (path.clone(), read_base(&repo, root, path))).collect();
    Head { branch, git_dir: Some(repo.git_dir().to_owned()), bases }
}

/// A file (relative to `root`, or absolute) at HEAD, if it is there as text.
fn read_base(repo: &gix::Repository, root: &Path, path: &Path) -> Option<Rope> {
    let absolute = root.join(path);
    let relative = absolute.strip_prefix(repo.workdir()?).ok()?;
    let entry = repo.head_tree().ok()?.lookup_entry_by_path(relative).ok()??;
    if !entry.mode().is_blob() {
        return None;
    }
    let object = entry.object().ok()?;
    let text = std::str::from_utf8(&object.data).ok()?;
    Some(Rope::from_str(text))
}
