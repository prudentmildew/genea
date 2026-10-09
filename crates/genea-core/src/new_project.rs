//! The new-project flow (ticket #61): the New Project dialog and what its
//! Create does.
//!
//! The dialog isn't tied to a project (the welcome window opens it too), so
//! its state lives in the workbench. Create generates the project from its
//! template in the background (`templates`), then opens it: the app gives
//! every open project without a window a new one. Opening starts the
//! toolchain downloads, and the install is dispatched at once; it waits for
//! the toolchain (ADR 0005's one exception to "only on a click").

use std::path::PathBuf;

use genea_toolchain::Tool;

use crate::{
    templates::{self, NewProject, PackageManagerPin, ProjectCreation, RuntimePin, Template},
    workbench::Core,
};

/// What the New Project dialog's controls do.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NewProjectCommand {
    /// New Project…: opens the dialog, or keeps the open one as it is.
    Open,
    /// Closes the dialog without creating anything.
    Cancel,
    SetTemplate(Template),
    /// The project name: the folder's name, the root package's name and,
    /// in the full-stack template, the `@<name>/…` scope.
    SetName(String),
    /// The folder the project folder is created in.
    SetParent(PathBuf),
    SetRuntime(RuntimePin),
    SetPackageManager(PackageManagerPin),
    /// Creates the project into `<parent>/<name>`, then opens it and
    /// installs its dependencies.
    Create,
}

/// The New Project dialog, as it shows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProjectDialog {
    pub template: Template,
    pub name: String,
    /// The parent folder, if one is chosen.
    pub parent: Option<PathBuf>,
    pub runtime: RuntimePin,
    pub package_manager: PackageManagerPin,
    /// The project is being generated; the dialog closes once it opens.
    pub creating: bool,
    /// Why the last Create was refused, if it was.
    pub error: Option<String>,
}

/// The dialog's state in the workbench.
#[derive(Default)]
pub(crate) struct NewProjectFlow {
    dialog: Option<Dialog>,
    /// Bumped by every Create, so an older creation can't touch a newer
    /// dialog.
    generation: u64,
}

struct Dialog {
    template: Template,
    name: String,
    parent: Option<PathBuf>,
    runtime: RuntimePin,
    package_manager: PackageManagerPin,
    /// The generation of the Create under way, if any.
    creating: Option<u64>,
    error: Option<String>,
}

impl NewProjectFlow {
    pub(crate) fn view(&self) -> Option<NewProjectDialog> {
        let dialog = self.dialog.as_ref()?;
        Some(NewProjectDialog {
            template: dialog.template,
            name: dialog.name.clone(),
            parent: dialog.parent.clone(),
            runtime: dialog.runtime.clone(),
            package_manager: dialog.package_manager.clone(),
            creating: dialog.creating.is_some(),
            error: dialog.error.clone(),
        })
    }
}

/// Applies a dialog command.
pub(crate) fn dispatch(core: &mut Core, command: NewProjectCommand) {
    let flow = &mut core.new_project;
    if command == NewProjectCommand::Open {
        if flow.dialog.is_none() {
            flow.dialog = Some(Dialog {
                template: Template::Frontend,
                name: String::new(),
                parent: None,
                runtime: RuntimePin::Node(Tool::Node.default_version().to_string()),
                package_manager: PackageManagerPin::Pnpm(Tool::Pnpm.default_version().to_string()),
                creating: None,
                error: None,
            });
        }
        return;
    }
    let Some(dialog) = &mut flow.dialog else { return };
    // Nothing changes while the project is being generated.
    if dialog.creating.is_some() {
        return;
    }
    match command {
        NewProjectCommand::Open => {}
        NewProjectCommand::Cancel => flow.dialog = None,
        NewProjectCommand::SetTemplate(template) => dialog.template = template,
        NewProjectCommand::SetName(name) => dialog.name = name,
        NewProjectCommand::SetParent(parent) => dialog.parent = Some(parent),
        NewProjectCommand::SetRuntime(pin) => dialog.runtime = pin,
        NewProjectCommand::SetPackageManager(pin) => dialog.package_manager = pin,
        NewProjectCommand::Create => create(core),
    }
}

fn create(core: &mut Core) {
    let flow = &mut core.new_project;
    let Some(dialog) = &mut flow.dialog else { return };
    let Some(parent) = dialog.parent.clone() else {
        dialog.error = Some("Choose a folder to create the project in.".into());
        return;
    };
    flow.generation += 1;
    let generation = flow.generation;
    dialog.creating = Some(generation);
    dialog.error = None;
    let request = NewProject {
        template: dialog.template,
        name: dialog.name.clone(),
        folder: parent.join(&dialog.name),
        runtime: dialog.runtime.clone(),
        package_manager: dialog.package_manager.clone(),
    };
    templates::create_then(core, request, move |core, outcome| {
        let opened = match outcome {
            ProjectCreation::Created { folder } => core.open_project(folder).map_err(|e| e.to_string()),
            ProjectCreation::Failed { message, .. } => Err(message.clone()),
            ProjectCreation::Creating { .. } => return,
        };
        let flow = &mut core.new_project;
        let Some(dialog) = flow.dialog.as_mut().filter(|d| d.creating == Some(generation)) else { return };
        match opened {
            Ok(_) => flow.dialog = None,
            Err(message) => {
                dialog.creating = None;
                dialog.error = Some(message);
            }
        }
    });
}
