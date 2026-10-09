//! Templates: creating a new project from one of the three built-in
//! starting points (ticket #60, contents decided in #12).
//!
//! Everything a template writes is embedded in the binary, and dependency
//! versions are frozen per Genea release as caret ranges, so generation needs
//! no network. The toolchain pins come from the caller and are written exact.
//! `git init` goes through `gix`, so no `git` binary is needed either.
//!
//! The workbench runs generation in the background
//! ([`Workbench::create_project`](crate::Workbench::create_project)) and
//! reports it as a [`ProjectCreation`].

mod generate;

use std::path::PathBuf;

use crate::workbench::Core;

/// One of the built-in templates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Template {
    /// A React 19 + Vite 8 single-page app.
    Frontend,
    /// A Hono 4 server run from source by Node type stripping.
    Backend,
    /// A workspace of `apps/web`, `apps/api` and `packages/shared`.
    FullStack,
}

/// The runtime a new project pins in `devEngines.runtime`, with its exact
/// version (`"24.21.0"`, no `v`).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum RuntimePin {
    Node(String),
    Bun(String),
}

/// The package manager a new project pins in `packageManager`, with its
/// exact version.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum PackageManagerPin {
    Pnpm(String),
    Bun(String),
}

/// What to create. The caller (the New Project dialog) has already checked
/// the name; generation itself only refuses a non-empty folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProject {
    pub template: Template,
    /// The project name: a valid npm package name. It names the root
    /// package and, in the full-stack template, the `@<name>/…` scope.
    pub name: String,
    /// The folder to create the project in, usually `<parent>/<name>`. It
    /// may exist if it is empty.
    pub folder: PathBuf,
    pub runtime: RuntimePin,
    pub package_manager: PackageManagerPin,
}

/// The state of the last [`create_project`](crate::Workbench::create_project).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProjectCreation {
    /// Files are being written in the background.
    Creating { folder: PathBuf },
    /// The project is ready to open.
    Created { folder: PathBuf },
    /// Nothing usable was created. `message` is for the user.
    Failed { folder: PathBuf, message: String },
}

/// The workbench's creation state: the last request and its outcome.
#[derive(Default)]
pub(crate) struct Creations {
    current: Option<ProjectCreation>,
    /// Bumped by every request, so a slow creation can't report over a newer one.
    generation: u64,
}

impl Creations {
    pub(crate) fn view(&self) -> Option<ProjectCreation> {
        self.current.clone()
    }
}

/// Starts generating `request` in the background.
pub(crate) fn create(core: &mut Core, request: NewProject) {
    create_then(core, request, |_, _| {});
}

/// Like [`create`], then calls `done` on the main thread with the outcome,
/// even if a newer request has replaced this one's state (the new-project
/// flow opens what it created).
pub(crate) fn create_then(
    core: &mut Core,
    request: NewProject,
    done: impl FnOnce(&mut Core, &ProjectCreation) + Send + 'static,
) {
    let creations = &mut core.creations;
    creations.generation += 1;
    let generation = creations.generation;
    let folder = request.folder.clone();
    creations.current = Some(ProjectCreation::Creating { folder: folder.clone() });

    core.jobs.spawn("create project", move || {
        let outcome = match generate::generate(&request) {
            Ok(()) => ProjectCreation::Created { folder },
            Err(message) => ProjectCreation::Failed { folder, message },
        };
        Box::new(move |core: &mut Core| {
            done(core, &outcome);
            if core.creations.generation == generation {
                core.creations.current = Some(outcome);
            }
        })
    });
}
