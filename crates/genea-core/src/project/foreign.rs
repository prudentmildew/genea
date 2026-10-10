//! Foreign tools (ticket #51, ADR 0001): a project's side of them. A tool
//! the project uses for a role Genea doesn't bless turns that role's
//! features off, and Genea never runs it:
//!
//! - **npm or Yarn** (the toolchain's package-manager role, from
//!   `packageManager` or a root lockfile): no installs and no script
//!   runner.
//!
//! What the user sees: one warning per role in Problems
//! (`ProblemSource::ForeignTools`) and one status-bar item naming every
//! foreign tool (`StatusBar::foreign_tools`).

use super::Project;
use crate::{problems::ProblemSource, toolchain::Toolchain};

impl Project {
    /// Puts the foreign tools' warnings into Problems. Called whenever what
    /// they depend on (the toolchain) changes.
    pub(crate) fn update_foreign_tools(&mut self) {
        let problems = self.foreign_tools().into_iter().map(|(_, problem)| problem).collect();
        self.replace_problems(ProblemSource::ForeignTools, problems);
    }

    /// Every foreign tool by name, with its role's warning.
    fn foreign_tools(&self) -> Vec<(String, crate::problems::Problem)> {
        self.toolchain.iter().filter_map(Toolchain::foreign_package_manager).collect()
    }

    /// Why the script runner is off: the package manager is a foreign one.
    pub(super) fn scripts_off(&self) -> Option<String> {
        let (name, _) = self.toolchain.as_ref()?.foreign_package_manager()?;
        Some(format!("Genea doesn't run {name}, so it doesn't run this project's scripts."))
    }

    /// The status-bar item while the project uses foreign tools:
    /// `Reduced mode: npm, Prettier`.
    pub(super) fn foreign_tools_status(&self) -> Option<String> {
        let names: Vec<String> = self.foreign_tools().into_iter().map(|(name, _)| name).collect();
        (!names.is_empty()).then(|| format!("Reduced mode: {}", names.join(", ")))
    }
}
