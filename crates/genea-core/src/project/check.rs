//! A project's side of the project check (ticket #48): running it with the
//! project's TypeScript 7 and putting its results in Problems. The process
//! and its output are `crate::project_check`.

use genea_host::ProcessSpec;

use super::Project;
use crate::{problems::ProblemSource, project_check};

/// The project check's state in a project.
#[derive(Default)]
pub(crate) struct Check {
    /// Bumped by every run, so an older run's results are dropped.
    generation: u64,
}

impl Project {
    /// "Run project check".
    pub(super) fn run_project_check(&mut self) {
        let Some((binary, host, jobs)) = self.typescript_tsc() else { return };
        self.check.generation += 1;
        let generation = self.check.generation;
        let spec = self.process_env().apply(ProcessSpec::new(binary).args(project_check::ARGS));
        let (id, root) = (self.id, self.root.clone());
        jobs.spawn("project check", move || {
            let outcome = project_check::run(&host, &spec, &root);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.check.generation != generation {
                    return;
                }
                if let Ok(outcome) = outcome {
                    project.problems.replace(ProblemSource::ProjectCheck, outcome.problems);
                }
            })
        });
    }
}
