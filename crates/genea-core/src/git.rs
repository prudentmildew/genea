//! Read-only git (ticket #56; spec #19, Git): the current branch, and the
//! open files' text at HEAD for the gutter markers. Read in-process with
//! `gix`, so nothing needs to be installed (ADR 0005), and always on a
//! background job: each read opens the repository afresh.
//!
//! **Markers.** Each open file in a repository has its *base*, the file at
//! HEAD, and is diffed against it in the background as it changes
//! ([`crate::diff`]). The markers show the last result, so they can be a
//! keystroke behind, but typing never waits for them.

use std::{
    collections::{HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
};

use ropey::Rope;

use crate::{
    diff::{BaseDiff, DiffJob, Diffed, Hunk},
    jobs::Jobs,
    view::{GutterMark, HunkView},
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
    /// relative to the root, each with its text at HEAD as the base: `None`
    /// if it isn't in HEAD (or isn't text), or there is no repository.
    files: HashMap<PathBuf, BaseDiff>,
}

/// What a read of HEAD found.
struct Head {
    branch: Option<String>,
    git_dir: Option<PathBuf>,
    /// The requested files at HEAD.
    bases: Vec<(PathBuf, Option<Rope>)>,
}

impl Git {
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        Git { project, root, branch: None, git_dir: None, generation: 0, files: HashMap::new() }
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
    /// init`). Then diffs those files again. Files opened meanwhile read
    /// their own base (`load_base`) and keep it.
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
                // Files closed since go; files opened since keep the base
                // `load_base` read for them.
                let open_now: HashSet<PathBuf> = project.open_editors().map(|e| e.path().to_owned()).collect();
                let git = &mut project.git;
                git.files.retain(|path, _| open_now.contains(path));
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
        self.files.entry(path).or_default().set_base(base);
    }

    /// A diff to run for `path`, if it has a base, its markers are behind
    /// the buffer (`version`, `text`) or the base, and none is running.
    pub(crate) fn start_diff(&mut self, path: &Path, version: u64, text: Rope) -> Option<DiffJob> {
        self.files.get_mut(path)?.start(version, text)
    }

    /// Takes a finished diff of `path`. Hunks against a base that has been
    /// replaced meanwhile are dropped.
    pub(crate) fn diffed(&mut self, path: &Path, diffed: Diffed) {
        if let Some(file) = self.files.get_mut(path) {
            file.finish(diffed);
        }
    }

    /// The change marked on `line` of `path`, with its lines at HEAD, if
    /// the markers are up to date with the buffer (`version`).
    fn current_hunk(&self, path: &Path, version: u64, line: usize) -> Option<(&Hunk, &Rope)> {
        let (hunks, base) = self.files.get(path)?.current(version)?;
        let hunk = hunks.iter().find(|h| h.lines.contains(&line) || h.lines == (line..line))?;
        Some((hunk, base))
    }

    /// The change marked on `line` of `path`, as its popover shows it.
    pub(crate) fn hunk_view(&self, path: &Path, version: u64, line: usize) -> Option<HunkView> {
        let (hunk, base) = self.current_hunk(path, version, line)?;
        let head = hunk.base_text(base).lines().collect::<Vec<_>>().join("\n");
        Some(HunkView { lines: hunk.lines.clone(), change: hunk.change(), head })
    }

    /// What rolling back the change marked on `line` of `path` takes: the
    /// buffer's lines to replace, and their text at HEAD (line breaks
    /// included) to replace them with.
    pub(crate) fn rollback(&self, path: &Path, version: u64, line: usize) -> Option<(Range<usize>, String)> {
        let (hunk, base) = self.current_hunk(path, version, line)?;
        Some((hunk.lines.clone(), hunk.base_text(base)))
    }

    /// The gutter markers of `path` on `lines`, top to bottom.
    pub(crate) fn gutter(&self, path: &Path, lines: Range<usize>) -> Vec<GutterMark> {
        let Some(file) = self.files.get(path) else { return Vec::new() };
        let mut marks = Vec::new();
        for hunk in file.hunks() {
            let change = hunk.change();
            let marked = if hunk.lines.is_empty() { hunk.lines.start..hunk.lines.start + 1 } else { hunk.lines.clone() };
            for line in marked.start.max(lines.start)..marked.end.min(lines.end) {
                marks.push(GutterMark { line, change });
            }
        }
        marks
    }
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

/// Why a file can't be shown against HEAD.
#[derive(Debug)]
pub(crate) enum NoHeadText {
    /// The project isn't in a git repository.
    NoRepository,
    /// The file is at HEAD, but not as text.
    NotText,
}

/// A file (relative to `root`, or absolute) at HEAD, for "Show Diff Against
/// HEAD" (ticket #57). A file that isn't at HEAD (untracked, or no commit
/// yet) reads as empty, so it shows as all added.
pub(crate) fn read_at_head(root: &Path, path: &Path) -> Result<Rope, NoHeadText> {
    let repo = gix::discover(root).map_err(|_| NoHeadText::NoRepository)?;
    let workdir = repo.workdir().ok_or(NoHeadText::NoRepository)?;
    let absolute = root.join(path);
    let Ok(relative) = absolute.strip_prefix(workdir) else { return Ok(Rope::new()) };
    let Ok(tree) = repo.head_tree() else { return Ok(Rope::new()) };
    let Ok(Some(entry)) = tree.lookup_entry_by_path(relative) else { return Ok(Rope::new()) };
    if !entry.mode().is_blob() {
        return Ok(Rope::new());
    }
    let object = entry.object().map_err(|_| NoHeadText::NotText)?;
    let text = std::str::from_utf8(&object.data).map_err(|_| NoHeadText::NotText)?;
    Ok(Rope::from_str(text))
}
