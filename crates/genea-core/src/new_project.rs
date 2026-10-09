//! The new-project flow (ticket #61): the New Project dialog and what its
//! Create does.
//!
//! The dialog isn't tied to a project (the welcome window opens it too), so
//! its state lives in the workbench. Create generates the project from its
//! template in the background (`templates`), then opens it: the app gives
//! every open project without a window a new one. Opening starts the
//! toolchain downloads, and the install is dispatched at once; it waits for
//! the toolchain (ADR 0005's one exception to "only on a click").
//!
//! The parent folder of the last project created is remembered in
//! `new-project-folder.txt` in the application-support folder, read the
//! first time the dialog opens and written after each project created.

use std::{
    ffi::OsStr,
    fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use genea_toolchain::{Tool, Version};

use crate::{
    command::Command,
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
    /// Why the name can't be used, while it can't. An empty name has no
    /// problem until Create.
    pub name_problem: Option<String>,
    /// The parent folder, if one is chosen.
    pub parent: Option<PathBuf>,
    pub runtime: RuntimePin,
    /// What the runtime picker offers: Node, then Bun, each newest first.
    pub runtimes: Vec<NewProjectOption<RuntimePin>>,
    pub package_manager: PackageManagerPin,
    /// What the package-manager picker offers: pnpm, then Bun.
    pub package_managers: Vec<NewProjectOption<PackageManagerPin>>,
    /// The published versions are being listed; until then the pickers
    /// offer Genea's defaults.
    pub listing: bool,
    /// Why a version list is missing (offline, say), if one is.
    pub message: Option<String>,
    /// The project is being generated; the dialog closes once it opens.
    pub creating: bool,
    /// Why the last Create was refused, if it was.
    pub error: Option<String>,
}

/// A choice in one of the dialog's toolchain pickers: the newest release of
/// each major version, and Genea's default.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NewProjectOption<P> {
    /// `Node 24.21.0`.
    pub label: String,
    /// `default` for Genea's default version, else empty.
    pub detail: String,
    /// What choosing it sets (`SetRuntime` / `SetPackageManager`).
    pub pin: P,
}

/// The dialog's state in the workbench.
#[derive(Default)]
pub(crate) struct NewProjectFlow {
    dialog: Option<Dialog>,
    /// Bumped by every Create, so an older creation can't touch a newer
    /// dialog.
    generation: u64,
    /// Bumped by every dialog opened, so a slow listing can't fill a newer
    /// dialog.
    list_generation: u64,
    /// The parent folder of the last project created, once read from the
    /// support folder (`Some(None)`: there is none).
    last_parent: Option<Option<PathBuf>>,
    reading_last_parent: bool,
    /// The newest folder to save, taken by the save job under the lock so
    /// saves land in order.
    unsaved_parent: Arc<Mutex<Option<PathBuf>>>,
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
    /// Per tool, the published versions; `None` while they are listed.
    listed: Option<Listed>,
}

struct Listed {
    versions: Vec<(Tool, Vec<Version>)>,
    /// Lists that couldn't be fetched.
    errors: Vec<String>,
}

impl Listed {
    /// What a picker offers of `tool`: the newest release of each major
    /// version, plus the default, newest first. Prereleases aren't offered.
    fn offered(&self, tool: Tool) -> Vec<Version> {
        let default = tool.default_version();
        let published = self.versions.iter().find(|(t, _)| *t == tool).map(|(_, v)| v.as_slice()).unwrap_or_default();
        let mut versions: Vec<Version> = published.iter().filter(|v| !v.is_prerelease()).cloned().collect();
        versions.push(default.clone());
        versions.sort_by(|a, b| b.cmp(a));
        versions.dedup();
        let mut offered: Vec<Version> = Vec::new();
        for version in versions {
            let newest_of_major = offered.last().is_none_or(|newer| newer.major() != version.major());
            if newest_of_major || version == default {
                offered.push(version);
            }
        }
        offered
    }
}

impl NewProjectFlow {
    pub(crate) fn view(&self) -> Option<NewProjectDialog> {
        let dialog = self.dialog.as_ref()?;
        let none = Listed { versions: Vec::new(), errors: Vec::new() };
        let listed = dialog.listed.as_ref().unwrap_or(&none);
        let options = |tools: [Tool; 2]| {
            tools.into_iter().flat_map(|tool| listed.offered(tool).into_iter().map(move |version| (tool, version)))
        };
        let option = |tool: Tool, version: &Version| {
            let detail = if *version == tool.default_version() { "default" } else { "" };
            (format!("{tool} {version}"), detail.to_owned(), version.to_string())
        };
        let runtimes = options([Tool::Node, Tool::Bun])
            .map(|(tool, version)| {
                let (label, detail, version) = option(tool, &version);
                let pin = if tool == Tool::Bun { RuntimePin::Bun(version) } else { RuntimePin::Node(version) };
                NewProjectOption { label, detail, pin }
            })
            .collect();
        let package_managers = options([Tool::Pnpm, Tool::Bun])
            .map(|(tool, version)| {
                let (label, detail, version) = option(tool, &version);
                let pin =
                    if tool == Tool::Bun { PackageManagerPin::Bun(version) } else { PackageManagerPin::Pnpm(version) };
                NewProjectOption { label, detail, pin }
            })
            .collect();
        Some(NewProjectDialog {
            template: dialog.template,
            name: dialog.name.clone(),
            name_problem: (!dialog.name.is_empty()).then(|| check_name(&dialog.name).err()).flatten(),
            parent: dialog.parent.clone(),
            runtime: dialog.runtime.clone(),
            runtimes,
            package_manager: dialog.package_manager.clone(),
            package_managers,
            listing: dialog.listed.is_none(),
            message: (!listed.errors.is_empty()).then(|| listed.errors.join(" ")),
            creating: dialog.creating.is_some(),
            error: dialog.error.clone(),
        })
    }
}

/// Lists the published versions for the pickers in the background.
fn list_versions(core: &mut Core) {
    let flow = &mut core.new_project;
    flow.list_generation += 1;
    let generation = flow.list_generation;
    let host = core.host.clone();
    core.jobs.spawn("list versions for a new project", move || {
        let mut listed = Listed { versions: Vec::new(), errors: Vec::new() };
        for tool in Tool::ALL {
            match genea_toolchain::published(host.downloads(), tool) {
                Ok(versions) => listed.versions.push((tool, versions)),
                Err(error) => listed.errors.push(format!("Couldn't list the {tool} versions: {error}")),
            }
        }
        Box::new(move |core: &mut Core| {
            let flow = &mut core.new_project;
            if flow.list_generation == generation
                && let Some(dialog) = &mut flow.dialog
            {
                dialog.listed = Some(listed);
            }
        })
    });
}

/// Applies a dialog command.
pub(crate) fn dispatch(core: &mut Core, command: NewProjectCommand) {
    let flow = &mut core.new_project;
    if command == NewProjectCommand::Open {
        if flow.dialog.is_none() {
            let parent = default_parent(core);
            core.new_project.dialog = Some(Dialog {
                template: Template::Frontend,
                name: String::new(),
                parent,
                runtime: RuntimePin::Node(Tool::Node.default_version().to_string()),
                package_manager: PackageManagerPin::Pnpm(Tool::Pnpm.default_version().to_string()),
                creating: None,
                error: None,
                listed: None,
            });
            list_versions(core);
        }
        return;
    }
    if command == NewProjectCommand::Cancel {
        // A project being generated still opens when it is ready.
        flow.dialog = None;
        return;
    }
    let Some(dialog) = &mut flow.dialog else { return };
    // Nothing changes while the project is being generated.
    if dialog.creating.is_some() {
        return;
    }
    match command {
        NewProjectCommand::Open | NewProjectCommand::Cancel => {}
        NewProjectCommand::SetTemplate(template) => dialog.template = template,
        NewProjectCommand::SetName(name) => dialog.name = name,
        NewProjectCommand::SetParent(parent) => dialog.parent = Some(parent),
        NewProjectCommand::SetRuntime(pin) => dialog.runtime = pin,
        NewProjectCommand::SetPackageManager(pin) => dialog.package_manager = pin,
        NewProjectCommand::Create => create(core),
    }
}

/// The parent folder a new dialog starts with: the last one used, else the
/// home folder. The first time, it is read in the background and filled in
/// when it arrives (unless one was chosen meanwhile).
fn default_parent(core: &mut Core) -> Option<PathBuf> {
    let home = core.host.launch_environment().into_iter().find(|(k, _)| k == "HOME").map(|(_, v)| PathBuf::from(v));
    let flow = &mut core.new_project;
    if let Some(last) = &flow.last_parent {
        return last.clone().or(home);
    }
    if !flow.reading_last_parent {
        flow.reading_last_parent = true;
        let file = core.host.support_dir().join(LAST_PARENT_FILE);
        core.jobs.spawn("read the last new-project folder", move || {
            let last = fs::read(&file).ok().and_then(|bytes| parse_folder(&bytes));
            Box::new(move |core: &mut Core| {
                let flow = &mut core.new_project;
                flow.reading_last_parent = false;
                // A project created meanwhile has already set it.
                let last = flow.last_parent.get_or_insert(last).clone();
                if let Some(dialog) = flow.dialog.as_mut().filter(|d| d.parent.is_none()) {
                    dialog.parent = last.or(home);
                }
            })
        });
    }
    None
}

/// Remembers `parent` as the last folder used, and saves it in the
/// background.
fn remember_parent(core: &mut Core, parent: PathBuf) {
    let flow = &mut core.new_project;
    flow.last_parent = Some(Some(parent.clone()));
    *flow.unsaved_parent.lock().unwrap() = Some(parent);
    let (unsaved, file) = (flow.unsaved_parent.clone(), core.host.support_dir().join(LAST_PARENT_FILE));
    core.jobs.spawn("save the last new-project folder", move || {
        // Take the folder inside the job, under the lock, so that saves land
        // in order and the last one is the newest.
        let mut unsaved = unsaved.lock().unwrap();
        if let Some(parent) = unsaved.take()
            && let Err(error) = write_folder(&file, &parent)
        {
            eprintln!("genea: couldn't save the new-project folder to {}: {error}", file.display());
        }
        Box::new(|_| {})
    });
}

const LAST_PARENT_FILE: &str = "new-project-folder.txt";

fn parse_folder(bytes: &[u8]) -> Option<PathBuf> {
    let line = bytes.split(|&b| b == b'\n').next()?;
    Some(PathBuf::from(OsStr::from_bytes(line))).filter(|path| path.is_absolute())
}

/// Writes the folder atomically: a crash mid-write keeps the old file.
fn write_folder(file: &Path, folder: &Path) -> io::Result<()> {
    let mut bytes = folder.as_os_str().as_bytes().to_vec();
    if bytes.contains(&b'\n') {
        // A newline in a folder name can't be stored in this format.
        return Ok(());
    }
    bytes.push(b'\n');
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = file.with_extension("txt.tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, file)
}

fn create(core: &mut Core) {
    let flow = &mut core.new_project;
    let Some(dialog) = &mut flow.dialog else { return };
    if let Err(problem) = check_name(&dialog.name) {
        dialog.error = Some(problem);
        return;
    }
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
            ProjectCreation::Created { folder } => {
                remember_parent(core, parent);
                core.open_project(folder).map_err(|e| e.to_string()).inspect(|&id| {
                    // Creating the project was the click (ADR 0005). The
                    // install waits for package.json and the toolchain.
                    core.dispatch(id, Command::InstallDependencies);
                })
            }
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

/// The longest package name npm accepts.
const MAX_NAME_LENGTH: usize = 214;

/// Names npm refuses for a package.
const RESERVED_NAMES: [&str; 2] = ["node_modules", "favicon.ico"];

/// Node's built-in modules, which npm doesn't accept as new package names.
const NODE_BUILTINS: [&str; 42] = [
    "assert", "async_hooks", "buffer", "child_process", "cluster", "console", "constants", "crypto", "dgram",
    "diagnostics_channel", "dns", "domain", "events", "fs", "http", "http2", "https", "inspector", "module", "net",
    "os", "path", "perf_hooks", "process", "punycode", "querystring", "readline", "repl", "stream", "string_decoder",
    "sys", "timers", "tls", "trace_events", "tty", "url", "util", "v8", "vm", "wasi", "worker_threads", "zlib",
];

/// Checks `name` as a new npm package's name (the rules of
/// `validate-npm-package-name` for new packages, without scopes: the name is
/// also a folder name and the full-stack template's scope).
fn check_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("Enter a project name.".into());
    }
    if name.len() > MAX_NAME_LENGTH {
        return Err(format!("A project name can't be longer than {MAX_NAME_LENGTH} characters."));
    }
    if name.starts_with(['.', '_']) {
        return Err("A project name can't start with a dot or an underscore.".into());
    }
    if name.chars().any(|c| c.is_ascii_uppercase()) {
        return Err("A project name can't have capital letters.".into());
    }
    if !name.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '.' | '_')) {
        return Err("A project name can only have lowercase letters, digits, hyphens, dots and underscores.".into());
    }
    if RESERVED_NAMES.contains(&name) {
        return Err(format!("“{name}” can't be a package name."));
    }
    if NODE_BUILTINS.contains(&name) {
        return Err(format!("“{name}” is the name of a Node built-in module."));
    }
    Ok(())
}
