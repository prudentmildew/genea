//! Whether a project's dependencies are installed (ticket #41): its root
//! `node_modules` folder exists. Looked at in the background when the
//! project opens and again whenever the watcher sees something change at or
//! under it, so the "Install dependencies" notice follows installs and
//! deletions made anywhere.
//!
//! The install itself runs in a terminal tab (`Terminal::run_install`);
//! nothing here starts a process.

use std::path::PathBuf;

use crate::{jobs::Jobs, watcher::FileChanges, workbench::ProjectId};

pub(crate) struct Dependencies {
    project: ProjectId,
    /// `<root>/node_modules`.
    node_modules: PathBuf,
    /// Whether it exists; `None` until first looked at.
    installed: Option<bool>,
    /// Bumped by every look, so a slow one can't undo a newer one.
    generation: u64,
}

impl Dependencies {
    pub(crate) fn new(project: ProjectId, root: &std::path::Path) -> Self {
        Dependencies { project, node_modules: root.join("node_modules"), installed: None, generation: 0 }
    }

    /// Looks for `node_modules` in the background.
    pub(crate) fn check(&mut self, jobs: &Jobs) {
        self.generation += 1;
        let (id, generation, node_modules) = (self.project, self.generation, self.node_modules.clone());
        jobs.spawn("look for node_modules", move || {
            let installed = node_modules.is_dir();
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                let dependencies = &mut project.dependencies;
                if dependencies.generation == generation {
                    dependencies.installed = Some(installed);
                }
            })
        })
    }

    /// Files changed on disk: looks again if `node_modules` may have
    /// appeared or gone.
    pub(crate) fn files_changed(&mut self, changes: &FileChanges, jobs: &Jobs) {
        if changes.rescan || changes.paths.iter().any(|path| path.starts_with(&self.node_modules)) {
            self.check(jobs);
        }
    }

    /// `node_modules` is known to be missing.
    pub(crate) fn are_missing(&self) -> bool {
        self.installed == Some(false)
    }
}
