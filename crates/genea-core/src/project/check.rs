//! A project's side of the project check (ticket #48): running it with the
//! project's TypeScript 7, putting its results in Problems, hiding them in
//! files open on the language server (live diagnostics take over there),
//! and marking them stale in files that change after it. The process and
//! its output are `crate::project_check`.

use std::{collections::BTreeSet, path::PathBuf};

use genea_host::ProcessSpec;

use super::Project;
use crate::{
    lsp::text,
    problems::ProblemSource,
    project_check::{self, Cancel, Finished},
    view::Notice,
    watcher::FileChanges,
};

/// The project check's state in a project.
#[derive(Default)]
pub(crate) struct Check {
    /// Bumped by every run, so an older run's results are dropped.
    generation: u64,
    /// The running check's process; `None` while none runs.
    running: Option<Cancel>,
    /// Files changed on disk since the running check started (relative to
    /// the root); stale once its results land.
    changed: BTreeSet<PathBuf>,
    /// The watcher lost track of changes while the check ran.
    rescanned: bool,
    /// What the last check (or attempt) has to say.
    notices: Vec<Notice>,
}

impl Drop for Check {
    /// A closing project stops its check.
    fn drop(&mut self) {
        if let Some(cancel) = self.running.take() {
            cancel.cancel();
        }
    }
}

impl Project {
    /// "Run project check": starts a check, stopping one that runs.
    pub(super) fn run_project_check(&mut self) {
        let Some((binary, host, jobs)) = self.typescript_tsc() else {
            self.check.notices = vec![notice("The project check needs TypeScript 7 installed in this project.".into())];
            return;
        };
        if let Some(cancel) = self.check.running.take() {
            cancel.cancel();
        }
        self.check.generation += 1;
        let generation = self.check.generation;
        let cancel = Cancel::default();
        self.check.running = Some(cancel.clone());
        self.check.changed.clear();
        self.check.rescanned = false;
        let spec = self.process_env().apply(ProcessSpec::new(binary).args(project_check::ARGS));
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("project check", move || {
            let finished = project_check::run(&host, &spec, &root, &cancel);
            Box::new(move |core| {
                if let Some(project) = core.project_mut(id)
                    && project.check.generation == generation
                {
                    project.project_check_finished(finished);
                }
            })
        });
    }

    fn project_check_finished(&mut self, finished: Finished) {
        self.check.running = None;
        self.check.notices = match finished {
            Finished::Failed(reason) => vec![notice(format!("The project check failed: {reason}"))],
            Finished::Checked { problems, skipped, general } => {
                self.problems.replace(ProblemSource::ProjectCheck, problems);
                let mut notices: Vec<Notice> =
                    general.into_iter().map(|message| notice(format!("The project check: {message}"))).collect();
                notices.extend(skipped_notice(&skipped));
                notices
            }
        };
        if std::mem::take(&mut self.check.rescanned) {
            self.problems.mark_all_stale();
        }
        for path in std::mem::take(&mut self.check.changed) {
            self.problems.mark_stale(&path);
        }
    }

    /// The status-bar item while a check runs.
    pub(super) fn project_check_status(&self) -> Option<String> {
        self.check.running.as_ref().map(|_| "Checking project…".to_owned())
    }

    /// What the last check (or attempt) has to say.
    pub(super) fn project_check_notices(&self) -> Vec<Notice> {
        self.check.notices.clone()
    }

    /// Keeps the project check in step with the open editors: open files
    /// show live diagnostics instead of its results. Runs after every
    /// command and every background result.
    pub(crate) fn sync_project_check(&mut self) {
        let live: BTreeSet<_> = self
            .open_editors()
            .filter(|e| text::language_id(e.path()).is_some() && e.is_loaded() && !e.is_large())
            .map(|e| e.path().to_owned())
            .collect();
        self.problems.set_live(live);
    }

    /// Files changed on disk: their project-check results are stale, and
    /// so will be those of a check running now.
    pub(super) fn project_check_files_changed(&mut self, changes: &FileChanges) {
        let running = self.check.running.is_some();
        if changes.rescan {
            self.problems.mark_all_stale();
            self.check.rescanned |= running;
        }
        for path in &changes.paths {
            let Ok(path) = path.strip_prefix(&self.root) else { continue };
            self.problems.mark_stale(path);
            if running {
                self.check.changed.insert(path.to_owned());
            }
        }
    }
}

fn notice(message: String) -> Notice {
    Notice { message, action: None }
}

/// Names the projects TypeScript 7 skipped for their references (TS6310;
/// see `crate::project_check`).
fn skipped_notice(skipped: &[PathBuf]) -> Option<Notice> {
    let names: Vec<String> = skipped.iter().map(|path| path.display().to_string()).collect();
    let (list, their) = match names.as_slice() {
        [] => return None,
        [one] => (one.clone(), "Its"),
        [rest @ .., last] => (format!("{} and {last}", rest.join(", ")), "Their"),
    };
    Some(notice(format!(
        "The project check skipped {list}: TypeScript 7 won't check a project that references other projects \
         without emitting their output (TS6310). {their} open files still get live diagnostics."
    )))
}
