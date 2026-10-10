//! Inline diffs (ticket #55): an open file shown with its removed and added
//! lines against a base, here its review baseline.
//!
//! `OpenChange` opens a file listed in Changes and reads its baseline from
//! the store in the background. The file is then diffed against it like the
//! git gutter diffs against HEAD (`crate::diff`, one job at a time, again
//! whenever the buffer moves on), and each result is handed to the editor
//! to lay out (`Editor::set_inline_diff`). The diff closes when the file
//! leaves Changes (Keep, Revert, or its content going back to the
//! baseline), with `CloseInlineDiff`, or when the file's last tab closes.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
};

use super::Project;
use crate::{
    command::Command,
    diff::BaseDiff,
    editor::{Editor, InlineDiff},
    jobs::Jobs,
    review::BaselineReader,
    view::{ChangeKind, DiffAgainst, EditorView},
};

/// The files shown with an inline diff, by path.
#[derive(Default)]
pub(super) struct InlineDiffs {
    files: HashMap<PathBuf, Shown>,
    /// Bumped by every baseline read, so a slow read can't replace a newer one.
    generation: u64,
}

/// One file's inline diff.
struct Shown {
    against: DiffAgainst,
    /// The baseline it was read from.
    baseline: BaselineReader,
    /// The read of the baseline in flight, if any.
    reading: Option<u64>,
    diff: BaseDiff,
}

impl InlineDiffs {
    /// The file's last tab closed.
    pub(super) fn forget(&mut self, path: &Path) {
        self.files.remove(path);
    }
}

impl Project {
    pub(super) fn inline_diff_command(&mut self, command: Command, jobs: &Jobs) {
        match command {
            Command::OpenChange(path) => self.open_change(path, jobs),
            Command::CloseInlineDiff => {
                if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
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
        let shown = Shown { against: DiffAgainst::ReviewBaseline, baseline: baseline.clone(), reading: None, diff: BaseDiff::default() };
        self.inline_diffs.files.insert(path.clone(), shown);
        self.read_baseline(path.clone(), baseline, jobs);
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

    /// Reads a shown file's baseline in the background, then diffs it.
    fn read_baseline(&mut self, path: PathBuf, baseline: BaselineReader, jobs: &Jobs) {
        let Some(shown) = self.inline_diffs.files.get_mut(&path) else { return };
        self.inline_diffs.generation += 1;
        let generation = self.inline_diffs.generation;
        shown.reading = Some(generation);
        let id = self.id;
        jobs.spawn("read review baseline", move || {
            let read = baseline.read();
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
                    Err(error) => {
                        project.end_inline_diff(&path);
                        project.notify(format!("Couldn't read the review baseline of {}: {error}", path.display()));
                    }
                }
            })
        });
    }

    /// Starts a background diff of an open file shown with an inline diff,
    /// if it is behind the buffer and none is running.
    pub(super) fn diff_inline(&mut self, path: &Path, jobs: &Jobs) {
        let Some(editor) = self.open_editor(path) else { return };
        let (version, text) = (editor.version(), editor.text().clone());
        let Some(job) = self.inline_diffs.files.get_mut(path).and_then(|shown| shown.diff.start(version, text)) else {
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
        let diff = InlineDiff { against: shown.against, base, hunks: shown.diff.hunks().to_vec() };
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
            .filter(|(_, shown)| shown.against == DiffAgainst::ReviewBaseline)
            .map(|(path, shown)| (path.clone(), shown.baseline.clone()))
            .collect();
        for (path, baseline) in shown {
            match self.review.baseline_reader(&path) {
                None => self.end_inline_diff(&path),
                Some(now) if now != baseline => {
                    if let Some(shown) = self.inline_diffs.files.get_mut(&path) {
                        shown.baseline = now.clone();
                    }
                    self.read_baseline(path, now, jobs);
                }
                Some(_) => {}
            }
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
