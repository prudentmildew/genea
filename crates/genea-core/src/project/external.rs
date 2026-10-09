//! Open files following external changes on disk (ticket #32; the
//! comparison is in `crate::disk`).

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use super::Project;
use crate::{command::ConflictChoice, editor::Editor, jobs::Jobs, watcher::FileChanges};

impl Project {
    /// The watcher reported changes: every open file among them (or every
    /// open file, on a rescan) is checked against what Genea believes is on
    /// disk.
    pub(super) fn check_open_files(&mut self, changes: &FileChanges, jobs: &Jobs) {
        let changed: Vec<PathBuf> = self
            .open_editors()
            .map(Editor::path)
            .filter(|path| changes.rescan || changes.paths.contains(&self.root.join(path)))
            .map(Path::to_path_buf)
            .collect();
        for path in changed {
            self.check_disk(path, jobs);
        }
    }

    /// Answers an open file's conflict bar.
    pub(super) fn resolve_conflict(&mut self, path: &Path, choice: ConflictChoice, now: Instant, jobs: &Jobs) {
        let rows = self.viewport_rows;
        if let Some(editor) = self.open_editor_mut(path) {
            editor.resolve_conflict(choice, now, rows);
            self.reparse_file(path, jobs);
        }
    }

    /// Reads an open file in the background and compares it with what
    /// Genea believes is on disk. A stale result checks again.
    fn check_disk(&mut self, path: PathBuf, jobs: &Jobs) {
        let Some(check) = self.open_editor(&path).map(Editor::disk_check) else { return };
        let absolute = self.root.join(&path);
        let id = self.id;
        jobs.spawn("check file on disk", move || {
            let checked = check.run(&absolute);
            Box::new(move |core| {
                let now = core.host.clock().now();
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                let rows = project.viewport_rows;
                let Some(editor) = project.open_editor_mut(&path) else { return };
                if editor.disk_checked(checked, now, rows) {
                    project.reparse_file(&path, &jobs);
                } else {
                    project.check_disk(path, &jobs);
                }
            })
        });
    }
}
