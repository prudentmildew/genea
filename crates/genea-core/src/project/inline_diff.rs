//! Inline diffs: an open file shown with its removed and added lines
//! against a base, its review baseline (ticket #55) or its text at HEAD
//! (ticket #57).
//!
//! `OpenChange` opens a file listed in Changes and reads its baseline from
//! the store in the background; `ShowDiffAgainstHead` reads the focused
//! file at HEAD with `gix`, also in the background. The file is then diffed
//! against its base like the git gutter diffs against HEAD (`crate::diff`,
//! one job at a time, again whenever the buffer moves on), and each result
//! is handed to the editor to lay out (`Editor::set_inline_diff`). A diff
//! against the review baseline closes when the file leaves Changes (Keep,
//! Revert, or its content going back to the baseline); a diff against
//! HEAD reads HEAD again when it moves. Either closes with
//! `CloseInlineDiff`, or when the file's last tab closes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use ropey::Rope;

use super::Project;
use crate::{
    command::Command,
    diff::BaseDiff,
    editor::{Editor, InlineDiff},
    git::{self, NoHeadText},
    jobs::Jobs,
    review::BaselineReader,
    view::{ChangeKind, DiffAgainst, EditorView},
};

/// The files shown with an inline diff, by path.
#[derive(Default)]
pub(super) struct InlineDiffs {
    files: HashMap<PathBuf, Shown>,
    /// Bumped by every read of a base, so a slow read can't replace a newer
    /// one.
    generation: u64,
}

/// One file's inline diff.
struct Shown {
    base: Base,
    /// The read of the base in flight, if any.
    reading: Option<u64>,
    diff: BaseDiff,
}

/// Where a shown file's base comes from.
enum Base {
    /// The review baseline, read from this reader.
    Review(BaselineReader),
    /// The file at HEAD.
    Head,
}

impl Base {
    fn against(&self) -> DiffAgainst {
        match self {
            Base::Review(_) => DiffAgainst::ReviewBaseline,
            Base::Head => DiffAgainst::Head,
        }
    }
}

impl InlineDiffs {
    /// The file's last tab closed.
    pub(super) fn forget(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

impl Project {
    pub(super) fn inline_diff_command(&mut self, command: Command, jobs: &Jobs) {
        let focused = self.editor.as_ref().map(|e| e.path().to_owned());
        match command {
            Command::OpenChange(path) => self.open_change(path, jobs),
            Command::ShowDiffAgainstHead => {
                if let Some(path) = focused {
                    self.show_inline_diff(path, Base::Head, jobs);
                }
            }
            Command::CloseInlineDiff => {
                if let Some(path) = focused {
                    self.end_inline_diff(&path);
                }
            }
            _ => {}
        }
    }

    /// Opens a file listed in Changes with its inline diff against its
    /// review baseline.
    fn open_change(&mut self, path: PathBuf, jobs: &Jobs) {
        let Some(baseline) = self.review.baseline_reader(&path) else {
            return self.open_file(path, None, jobs);
        };
        self.show_inline_diff(path.clone(), Base::Review(baseline), jobs);
        let deleted = self.review.listed_change(&path).is_some_and(|c| c.kind == ChangeKind::Deleted);
        if !deleted {
            self.open_file(path, None, jobs);
            return;
        }
        // Like an OpenFile, so a slower one asked for before doesn't take
        // the focus from it.
        self.open_generation += 1;
        if !self.focus_open_file(&path) {
            self.open_tab(Editor::missing(path));
        }
    }

    /// Shows a file with an inline diff against `base`, once it is read.
    fn show_inline_diff(&mut self, path: PathBuf, base: Base, jobs: &Jobs) {
        let against = base.against();
        let shown = Shown { base, reading: None, diff: BaseDiff::default() };
        let replaced = self.inline_diffs.files.insert(path.clone(), shown);
        // A diff against another base isn't shown while this one is read.
        if replaced.is_some_and(|old| old.base.against() != against) {
            let rows = self.viewport_rows;
            if let Some(editor) = self.open_editor_mut(&path) {
                editor.set_inline_diff(None, rows);
            }
        }
        self.read_base(path, jobs);
    }

    /// Reads a shown file's base in the background, then diffs it.
    fn read_base(&mut self, path: PathBuf, jobs: &Jobs) {
        let Some(shown) = self.inline_diffs.files.get_mut(&path) else { return };
        self.inline_diffs.generation += 1;
        let generation = self.inline_diffs.generation;
        shown.reading = Some(generation);
        let id = self.id;
        let (name, read): (_, Box<dyn FnOnce() -> Result<Rope, String> + Send>) = match &shown.base {
            Base::Review(baseline) => {
                let (baseline, path) = (baseline.clone(), path.clone());
                let read = move || {
                    baseline
                        .read()
                        .map_err(|error| format!("Couldn't read the review baseline of {}: {error}", path.display()))
                };
                ("read review baseline", Box::new(read))
            }
            Base::Head => {
                let (root, path) = (self.root.clone(), path.clone());
                let read = move || {
                    git::read_at_head(&root, &path).map_err(|error| match error {
                        NoHeadText::NoRepository => {
                            format!("Can't show {} against HEAD: the project isn't in a git repository.", path.display())
                        }
                        NoHeadText::NotText => {
                            format!("Can't show {} against HEAD: it isn't text there.", path.display())
                        }
                    })
                };
                ("read file at git HEAD for its diff", Box::new(read))
            }
        };
        jobs.spawn(name, move || {
            let read = read();
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                let Some(shown) = project.inline_diffs.files.get_mut(&path) else { return };
                if shown.reading != Some(generation) {
                    return;
                }
                shown.reading = None;
                match read {
                    Ok(base) => {
                        shown.diff.set_base(Some(base));
                        project.diff_inline(&path, &jobs);
                    }
                    Err(message) => {
                        project.end_inline_diff(&path);
                        project.notify(message);
                    }
                }
            })
        });
    }

    /// Starts a background diff of an open file shown with an inline diff,
    /// if it is behind the buffer and none is running.
    pub(super) fn diff_inline(&mut self, path: &Path, jobs: &Jobs) {
        let Some(editor) = self.open_editor(path) else { return };
        let (version, text, laid_out) = (editor.version(), editor.text().clone(), editor.has_inline_diff());
        let Some(shown) = self.inline_diffs.files.get_mut(path) else { return };
        let Some(job) = shown.diff.start(version, text) else {
            // An editor that came after the last result (a reopened file)
            // takes it.
            if !laid_out {
                self.lay_out_inline_diff(path);
            }
            return;
        };
        let (id, path) = (self.id, path.to_owned());
        jobs.spawn("inline diff", move || {
            let diffed = job.run();
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                let Some(shown) = project.inline_diffs.files.get_mut(&path) else { return };
                shown.diff.finish(diffed);
                project.lay_out_inline_diff(&path);
                project.diff_inline(&path, &jobs);
            })
        });
    }

    /// Hands a shown file's latest hunks to its editor.
    fn lay_out_inline_diff(&mut self, path: &Path) {
        let Some(shown) = self.inline_diffs.files.get(path) else { return };
        let Some(base) = shown.diff.base().cloned() else { return };
        let diff = InlineDiff { against: shown.base.against(), base, hunks: shown.diff.hunks().to_vec() };
        let rows = self.viewport_rows;
        if let Some(editor) = self.open_editor_mut(path) {
            editor.set_inline_diff(Some(diff), rows);
        }
    }

    /// Shows a file plainly again. A deleted file's tabs close.
    fn end_inline_diff(&mut self, path: &Path) {
        self.inline_diffs.files.remove(path);
        let rows = self.viewport_rows;
        if let Some(editor) = self.open_editor_mut(path) {
            if editor.is_missing() {
                return self.close_file(path);
            }
            editor.set_inline_diff(None, rows);
        }
    }

    /// Changes changed (a review op finished): diffs against the review
    /// baseline of files no longer listed close, and those whose baseline
    /// moved read it again.
    pub(super) fn review_changes_changed(&mut self, jobs: &Jobs) {
        let shown: Vec<(PathBuf, BaselineReader)> = self
            .inline_diffs
            .files
            .iter()
            .filter_map(|(path, shown)| match &shown.base {
                Base::Review(baseline) => Some((path.clone(), baseline.clone())),
                Base::Head => None,
            })
            .collect();
        for (path, baseline) in shown {
            match self.review.baseline_reader(&path) {
                None => self.end_inline_diff(&path),
                Some(now) if now != baseline => {
                    if let Some(shown) = self.inline_diffs.files.get_mut(&path) {
                        shown.base = Base::Review(now);
                    }
                    self.read_base(path, jobs);
                }
                Some(_) => {}
            }
        }
    }

    /// HEAD may have moved (a commit, a checkout): diffs against HEAD read
    /// it again.
    pub(super) fn head_moved(&mut self, jobs: &Jobs) {
        let shown: Vec<PathBuf> = self
            .inline_diffs
            .files
            .iter()
            .filter(|(_, shown)| matches!(shown.base, Base::Head))
            .map(|(path, _)| path.clone())
            .collect();
        for path in shown {
            self.read_base(path, jobs);
        }
    }

    /// Fills in what the editor doesn't know about its inline diff: the
    /// file's entry in Changes, for Keep and Revert.
    pub(super) fn complete_inline_diff(&self, view: &mut EditorView) {
        if let Some(diff) = &mut view.inline_diff
            && diff.against == DiffAgainst::ReviewBaseline
        {
            diff.change = self.review.listed_change(&view.path).cloned();
        }
    }
}
