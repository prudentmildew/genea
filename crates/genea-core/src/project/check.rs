//! A project's side of the project check (ticket #48): running it with the
//! project's TypeScript 7 and putting its results in Problems. The process
//! and its output are `crate::project_check`.

use std::collections::BTreeSet;

use genea_host::ProcessSpec;

use super::Project;
use crate::{lsp::text, problems::ProblemSource, project_check};

/// The project check's state in a project.
#[derive(Default)]
pub(crate) struct Check {
    /// Bumped by every run, so an older run's results are dropped.
    generation: u64,
    /// A check is running.
    running: bool,
}

impl Project {
    /// "Run project check".
    pub(super) fn run_project_check(&mut self) {
        let Some((binary, host, jobs)) = self.typescript_tsc() else { return };
        self.check.generation += 1;
        let generation = self.check.generation;
        self.check.running = true;
        let spec = self.process_env().apply(ProcessSpec::new(binary).args(project_check::ARGS));
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("project check", move || {
            let outcome = project_check::run(&host, &spec, &root);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.check.generation != generation {
                    return;
                }
                project.check.running = false;
                if let Ok(outcome) = outcome {
                    project.problems.replace(ProblemSource::ProjectCheck, outcome.problems);
                }
            })
        });
    }

    /// The status-bar item while a check runs.
    pub(super) fn project_check_status(&self) -> Option<String> {
        self.check.running.then(|| "Checking project…".to_owned())
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
}
