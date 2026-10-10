//! A project's toolchain roles (ticket #35, ADR 0005): the runtime and the
//! package manager its root `package.json` pins, downloaded into the shared
//! store by `genea-toolchain`.
//!
//! On open, `package.json` is read in the background. Each role then
//! resolves its pin (or Genea's default) and installs it in its own job, so
//! the two download side by side and nothing waits on the main thread.
//! Download progress comes back as Applies, at most one per percent.
//!
//! What the user sees: [`ToolchainView`], the status-bar download item, and
//! notices: a failed role (with Retry), an unpinned project (with an action
//! that writes the defaults as exact pins), and a `package.json` that can't
//! be read or a pin that can't be understood.
//!
//! A folder without a root `package.json` has no toolchain and downloads
//! nothing. A foreign package manager (npm, Yarn, …; ticket #51), pinned in
//! `packageManager` or, without a pin, found by its root lockfile, turns
//! the role off: nothing is downloaded or run for it.

use std::{
    collections::HashSet,
    fs, io,
    path::{Path, PathBuf},
    sync::Arc,
};

use genea_host::SharedHost;
use genea_toolchain::{Installed, Pin, Pins, Progress, Request, Store, Tool, Version, pins};

use crate::{
    command::Command,
    jobs::Jobs,
    problems::{Problem, ProblemSource, Severity, TextPosition},
    review::{OwnWrites, store::hash_bytes},
    templates::{PackageManagerPin, RuntimePin},
    view::{
        Notice, NoticeAction, ToolState, ToolView, ToolchainOption, ToolchainPicker, ToolchainPickerKind,
        ToolchainView,
    },
    workbench::{Core, ProjectId},
};

/// What every project's toolchain shares: the host (for downloads) and the
/// one store.
#[derive(Clone)]
pub(crate) struct ToolchainContext {
    pub(crate) host: SharedHost,
    pub(crate) store: Arc<Store>,
}

impl ToolchainContext {
    pub(crate) fn new(host: SharedHost) -> Self {
        let store = Arc::new(Store::new(host.support_dir().join("toolchains")));
        ToolchainContext { host, store }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Role {
    Runtime,
    PackageManager,
}

impl Role {
    const ALL: [Role; 2] = [Role::Runtime, Role::PackageManager];

    fn index(self) -> usize {
        self as usize
    }

    fn noun(self) -> &'static str {
        match self {
            Role::Runtime => "runtime",
            Role::PackageManager => "package manager",
        }
    }
}

pub(crate) struct Toolchain {
    project: ProjectId,
    root: PathBuf,
    context: ToolchainContext,
    /// One per [`Role`]; `None` until `package.json` is read, and for a
    /// folder without one.
    slots: [Option<Slot>; 2],
    /// Problems with `package.json` itself, shown as notices.
    problems: Vec<String>,
    /// Bumped by every load, so a slow read can't replace a newer one.
    load_generation: u64,
    /// `package.json` is being read.
    loading: bool,
    /// The open toolchain picker, if any.
    picker: Option<Picker>,
    /// Bumped by every picker opened, so a slow listing can't fill a newer
    /// picker.
    picker_generation: u64,
    /// The lockfiles at the project root, checked against `packageManager`.
    lockfiles: Lockfiles,
    /// Where `packageManager` is in `package.json`: lockfile warnings point
    /// there.
    package_manager_at: TextPosition,
    /// Bumped by every lockfile check, so a slow one can't undo a newer one.
    lockfile_generation: u64,
    /// Writing pins to `package.json` is Genea's own write (ticket #53).
    own_writes: OwnWrites,
    /// `package.json` has a `packageManager` field. Without one, a foreign
    /// lockfile decides (ticket #51).
    package_manager_pinned: bool,
}

/// Writes pins to `package.json` as Genea's own write.
fn write_package_json(own_writes: &OwnWrites, path: &Path, text: &str) -> Result<(), String> {
    let _writing = own_writes.writing(Path::new("package.json"), Some(hash_bytes(text.as_bytes())));
    fs::write(path, text).map_err(|e| e.to_string())
}

/// The lockfiles Genea cross-checks, at the project root only: pnpm's, and
/// Bun's text and (older) binary ones.
pub(crate) const LOCKFILES: [&str; 3] = ["pnpm-lock.yaml", "bun.lock", "bun.lockb"];

/// The lockfiles of foreign package managers (ticket #51), at the project
/// root, with the tool each belongs to. One of them makes an unpinned
/// project an npm or Yarn project.
pub(crate) const FOREIGN_LOCKFILES: [(&str, &str); 3] =
    [("package-lock.json", "npm"), ("npm-shrinkwrap.json", "npm"), ("yarn.lock", "Yarn")];

/// Whether `name` is a lockfile Genea looks at, its own or a foreign one.
pub(crate) fn is_lockfile(name: &str) -> bool {
    LOCKFILES.contains(&name) || FOREIGN_LOCKFILES.iter().any(|(file, _)| *file == name)
}

/// Which lockfiles are at the project root.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Lockfiles {
    pnpm: bool,
    /// `bun.lock` or `bun.lockb`.
    bun: Option<&'static str>,
    /// The first foreign lockfile, and its tool (`npm`, `Yarn`).
    foreign: Option<(&'static str, &'static str)>,
}

impl Lockfiles {
    /// Stats the root for lockfiles (blocking).
    fn find(root: &Path) -> Self {
        let exists = |name: &str| root.join(name).is_file();
        Lockfiles {
            pnpm: exists(LOCKFILES[0]),
            bun: LOCKFILES[1..].iter().copied().find(|name| exists(name)),
            foreign: FOREIGN_LOCKFILES.iter().copied().find(|(name, _)| exists(name)),
        }
    }
}

/// A foreign package manager's name as the user reads it: `Yarn` for
/// `yarn`, anything else as written.
fn foreign_name(name: &str) -> String {
    if name == "yarn" { "Yarn".into() } else { name.to_owned() }
}

/// Where `packageManager` is in `package.json`'s text, else the start.
fn package_manager_position(text: &str) -> TextPosition {
    text.find("\"packageManager\"").map(|offset| TextPosition::of_byte_offset(text, offset)).unwrap_or_default()
}

/// An open toolchain picker.
struct Picker {
    kind: ToolchainPickerKind,
    query: String,
    generation: u64,
    /// `None` while the versions are being listed.
    listed: Option<Listed>,
}

/// The versions a picker offers, per tool.
struct Listed {
    /// Per tool: published and stored versions together, newest first.
    versions: Vec<(Tool, Vec<Version>)>,
    /// Per tool: the versions in the store.
    installed: Vec<(Tool, Vec<Version>)>,
    /// Lists that couldn't be fetched, for the picker's message.
    errors: Vec<String>,
}

struct Slot {
    /// The tool's display name: `Node`, or a foreign tool's own name.
    name: String,
    /// What to download: `None` for a foreign tool or an invalid pin.
    want: Option<(Tool, Request)>,
    /// The project doesn't pin this role; Genea's default is in use.
    defaulted: bool,
    /// The resolved version, else the pin as written.
    version: String,
    state: SlotState,
    /// Bumped by every start, so a stale job's results are dropped.
    generation: u64,
}

enum SlotState {
    Resolving,
    Downloading(Progress),
    Ready(Installed),
    Failed(String),
    /// An invalid pin: nothing to retry until `package.json` changes.
    Invalid(String),
    Off,
}

impl Toolchain {
    pub(crate) fn new(project: ProjectId, root: PathBuf, context: ToolchainContext, own_writes: OwnWrites) -> Self {
        Toolchain {
            own_writes,
            project,
            root,
            context,
            slots: [None, None],
            problems: Vec::new(),
            load_generation: 0,
            loading: false,
            picker: None,
            picker_generation: 0,
            lockfiles: Lockfiles::default(),
            package_manager_at: TextPosition::default(),
            lockfile_generation: 0,
            package_manager_pinned: false,
        }
    }

    /// Reads `package.json` in the background, then starts every role.
    pub(crate) fn load(&mut self, jobs: &Jobs) {
        self.load_generation += 1;
        self.loading = true;
        let generation = self.load_generation;
        self.lockfile_generation += 1;
        let lockfile_generation = self.lockfile_generation;
        let (id, root) = (self.project, self.root.clone());
        jobs.spawn("read toolchain pins", move || {
            let mut package_manager_at = TextPosition::default();
            let pins = match fs::read_to_string(root.join("package.json")) {
                Ok(text) => {
                    package_manager_at = package_manager_position(&text);
                    Some(pins::read(&text))
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => Some(Err(error.to_string())),
            };
            let lockfiles = Lockfiles::find(&root);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                if toolchain.load_generation == generation {
                    toolchain.loading = false;
                    toolchain.package_manager_at = package_manager_at;
                    if toolchain.lockfile_generation == lockfile_generation {
                        toolchain.lockfiles = lockfiles;
                    }
                    toolchain.loaded(pins, lockfiles, &jobs);
                }
                update_problems(core, id);
            })
        });
    }

    /// `package.json` is read: sets up every role. An unpinned package
    /// manager is a foreign one when the root has a foreign lockfile
    /// (`lockfiles`, as read with `package.json`).
    fn loaded(&mut self, pins: Option<Result<Pins, String>>, lockfiles: Lockfiles, jobs: &Jobs) {
        self.problems.clear();
        self.slots = [None, None];
        self.package_manager_pinned = false;
        let pins = match pins {
            None => return,
            Some(Ok(pins)) => pins,
            Some(Err(reason)) => {
                self.problems.push(format!("Couldn't read package.json: {reason}"));
                return;
            }
        };
        self.package_manager_pinned = pins.package_manager != Pin::Unpinned;
        let package_manager = match (pins.package_manager, lockfiles.foreign) {
            (Pin::Unpinned, Some((_, tool))) => Pin::Foreign(tool.to_owned()),
            (pin, _) => pin,
        };
        let defaults = [Tool::Node, Tool::Pnpm];
        for (role, pin) in Role::ALL.into_iter().zip([pins.runtime, package_manager]) {
            self.slots[role.index()] = Some(match pin {
                Pin::Unpinned => {
                    let tool = defaults[role.index()];
                    let version = tool.default_version();
                    Slot::new(tool.display_name(), Some((tool, Request::Exact(version))), true)
                }
                Pin::Pinned { tool, request } => Slot::new(tool.display_name(), Some((tool, request)), false),
                Pin::Foreign(name) => {
                    let mut slot = Slot::new(&foreign_name(&name), None, false);
                    slot.state = SlotState::Off;
                    slot
                }
                Pin::Invalid(reason) => {
                    let mut slot = Slot::new(role.noun(), None, false);
                    slot.state = SlotState::Invalid(reason);
                    slot
                }
            });
            self.start(role, jobs);
        }
    }

    /// Resolves and installs a role's tool in the background.
    fn start(&mut self, role: Role, jobs: &Jobs) {
        let Some(slot) = &mut self.slots[role.index()] else { return };
        let Some((tool, request)) = slot.want.clone() else { return };
        slot.generation += 1;
        slot.state = SlotState::Resolving;
        let generation = slot.generation;
        let id = self.project;
        let ToolchainContext { host, store } = self.context.clone();
        let progress_jobs = jobs.clone();
        jobs.spawn(&format!("install {}", tool.id()), move || {
            let downloads = host.downloads();
            let resolved = store.resolve(downloads, tool, &request).map_err(|e| (None, e));
            let result = resolved.and_then(|version| {
                let mut last = None;
                let mut report = |progress: Progress| {
                    // At most one Apply per percent, or per MiB without a length.
                    let step = progress.percent().map_or(progress.received >> 20, u64::from);
                    if last == Some(step) {
                        return;
                    }
                    last = Some(step);
                    let version = version.clone();
                    progress_jobs.busy().finish(Box::new(move |core| {
                        let Some(slot) = slot_mut(core, id, role, generation) else { return };
                        slot.version = version.to_string();
                        slot.state = SlotState::Downloading(progress);
                    }));
                };
                store.install(downloads, tool, &version, &mut report).map_err(|e| (Some(version.clone()), e))
            });
            let result = result.map_err(|(version, e)| (version, e.to_string()));
            Box::new(move |core| {
                let Some(slot) = slot_mut(core, id, role, generation) else { return };
                match result {
                    Ok(installed) => {
                        slot.version = installed.version.to_string();
                        slot.state = SlotState::Ready(installed);
                    }
                    Err((version, reason)) => {
                        if let Some(version) = version {
                            slot.version = version.to_string();
                        }
                        slot.state = SlotState::Failed(reason);
                    }
                }
            })
        });
    }

    /// Restarts every role whose download failed.
    pub(crate) fn retry(&mut self, jobs: &Jobs) {
        for role in Role::ALL {
            if matches!(self.slots[role.index()], Some(Slot { state: SlotState::Failed(_), .. })) {
                self.start(role, jobs);
            }
        }
    }

    /// Writes the defaults in use as exact pins, in the background.
    pub(crate) fn pin_defaults(&mut self, jobs: &Jobs) {
        let pin = |role: Role| -> Option<(Tool, Version)> {
            match &self.slots[role.index()] {
                Some(Slot { defaulted: true, want: Some((tool, Request::Exact(version))), .. }) => {
                    Some((*tool, version.clone()))
                }
                _ => None,
            }
        };
        let (runtime, package_manager) = (pin(Role::Runtime), pin(Role::PackageManager));
        if runtime.is_none() && package_manager.is_none() {
            return;
        }
        let (id, path) = (self.project, self.root.join("package.json"));
        let own_writes = self.own_writes.clone();
        jobs.spawn("pin toolchain defaults", move || {
            let written = fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
                let text = pins::write(
                    &text,
                    runtime.as_ref().map(|(t, v)| (*t, v)),
                    package_manager.as_ref().map(|(t, v)| (*t, v)),
                )?;
                write_package_json(&own_writes, &path, &text)
            });
            Box::new(move |core| {
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                match written {
                    Ok(()) => {
                        for slot in toolchain.slots.iter_mut().flatten() {
                            slot.defaulted = false;
                        }
                    }
                    Err(reason) => toolchain.problems.push(format!("Couldn't write the pins to package.json: {reason}")),
                }
            })
        });
    }

    /// Opens a toolchain picker and lists its versions in the background:
    /// what the publishers have plus what the store has, so a picker still
    /// offers the downloaded versions offline.
    pub(crate) fn open_picker(&mut self, kind: ToolchainPickerKind, jobs: &Jobs) {
        self.picker_generation += 1;
        let generation = self.picker_generation;
        self.picker = Some(Picker { kind, query: String::new(), generation, listed: None });
        let tools: Vec<Tool> = match kind {
            ToolchainPickerKind::Runtime => vec![Tool::Node, Tool::Bun],
            ToolchainPickerKind::PackageManager => vec![Tool::Pnpm, Tool::Bun],
            ToolchainPickerKind::Update => {
                let mut tools: Vec<Tool> =
                    self.slots.iter().flatten().filter_map(|slot| slot.want.as_ref().map(|(tool, _)| *tool)).collect();
                tools.dedup();
                tools
            }
        };
        let id = self.project;
        let ToolchainContext { host, store } = self.context.clone();
        jobs.spawn("list toolchain versions", move || {
            let mut listed = Listed { versions: Vec::new(), installed: Vec::new(), errors: Vec::new() };
            for tool in tools {
                let installed = store.installed(tool);
                let mut versions = match genea_toolchain::published(host.downloads(), tool) {
                    Ok(published) => published,
                    Err(error) => {
                        listed.errors.push(format!("Couldn't list the {tool} versions: {error}"));
                        Vec::new()
                    }
                };
                versions.extend(installed.iter().cloned());
                versions.sort_by(|a, b| b.cmp(a));
                versions.dedup();
                listed.versions.push((tool, versions));
                listed.installed.push((tool, installed));
            }
            Box::new(move |core| {
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                if let Some(picker) = toolchain.picker.as_mut().filter(|p| p.generation == generation) {
                    picker.listed = Some(listed);
                }
            })
        });
    }

    pub(crate) fn filter_picker(&mut self, query: String) {
        if let Some(picker) = &mut self.picker {
            picker.query = query;
        }
    }

    pub(crate) fn close_picker(&mut self) {
        self.picker = None;
    }

    /// Pins the runtime to an exact version (a picker's option).
    pub(crate) fn set_runtime(&mut self, pin: RuntimePin, jobs: &Jobs) {
        let (tool, version) = match pin {
            RuntimePin::Node(version) => (Tool::Node, version),
            RuntimePin::Bun(version) => (Tool::Bun, version),
        };
        self.set_pin(Role::Runtime, tool, &version, jobs);
    }

    /// Pins the package manager to an exact version (a picker's option).
    pub(crate) fn set_package_manager(&mut self, pin: PackageManagerPin, jobs: &Jobs) {
        let (tool, version) = match pin {
            PackageManagerPin::Pnpm(version) => (Tool::Pnpm, version),
            PackageManagerPin::Bun(version) => (Tool::Bun, version),
        };
        self.set_pin(Role::PackageManager, tool, &version, jobs);
    }

    /// Writes `tool` `version` as the role's exact pin in the background,
    /// then switches the role to it and starts its download.
    fn set_pin(&mut self, role: Role, tool: Tool, version: &str, jobs: &Jobs) {
        self.picker = None;
        let Ok(version) = Version::parse(version) else {
            self.problems.push(format!("Couldn't pin {tool} to \"{version}\": it isn't a version."));
            return;
        };
        let (id, path) = (self.project, self.root.join("package.json"));
        let written_version = version.clone();
        let own_writes = self.own_writes.clone();
        jobs.spawn("write toolchain pin", move || {
            let written = fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
                let pin = Some((tool, &written_version));
                let text = match role {
                    Role::Runtime => pins::write(&text, pin, None),
                    Role::PackageManager => pins::write(&text, None, pin),
                }?;
                write_package_json(&own_writes, &path, &text)?;
                Ok(package_manager_position(&text))
            });
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                match written {
                    Ok(package_manager_at) => {
                        toolchain.package_manager_at = package_manager_at;
                        toolchain.pinned(role, tool, version, &jobs);
                    }
                    Err(reason) => toolchain.problems.push(format!("Couldn't write the pin to package.json: {reason}")),
                }
                update_problems(core, id);
            })
        });
    }

    /// The role is now pinned to `tool` `version`: download it.
    fn pinned(&mut self, role: Role, tool: Tool, version: Version, jobs: &Jobs) {
        let generation = self.slots[role.index()].as_ref().map_or(0, |slot| slot.generation);
        let mut slot = Slot::new(tool.display_name(), Some((tool, Request::Exact(version))), false);
        slot.generation = generation;
        self.slots[role.index()] = Some(slot);
        self.start(role, jobs);
    }

    /// The open picker, as the window shows it.
    pub(crate) fn picker_view(&self) -> Option<ToolchainPicker> {
        let picker = self.picker.as_ref()?;
        let title = match picker.kind {
            ToolchainPickerKind::Runtime => "Set runtime",
            ToolchainPickerKind::PackageManager => "Set package manager",
            ToolchainPickerKind::Update => "Update toolchain",
        };
        let mut view = ToolchainPicker {
            kind: picker.kind,
            title: title.into(),
            query: picker.query.clone(),
            loading: picker.listed.is_none(),
            message: None,
            options: Vec::new(),
        };
        let Some(listed) = &picker.listed else { return Some(view) };
        let mut options = Vec::new();
        match picker.kind {
            ToolchainPickerKind::Runtime | ToolchainPickerKind::PackageManager => {
                let role = if picker.kind == ToolchainPickerKind::Runtime { Role::Runtime } else { Role::PackageManager };
                let in_use = self.in_use(role);
                for (tool, versions) in &listed.versions {
                    let installed = listed.installed.iter().find(|(t, _)| t == tool).map(|(_, v)| v.as_slice());
                    for version in versions {
                        let detail = if in_use.as_ref().is_some_and(|(t, v)| t == tool && v == version) {
                            "in use"
                        } else if installed.is_some_and(|installed| installed.contains(version)) {
                            "downloaded"
                        } else {
                            ""
                        };
                        options.push(option(role, *tool, version, detail.into()));
                    }
                }
            }
            ToolchainPickerKind::Update => {
                let mut current = Vec::new();
                for role in Role::ALL {
                    let Some((tool, now)) = self.in_use(role) else { continue };
                    current.push(format!("{tool} {now}"));
                    let versions = listed.versions.iter().find(|(t, _)| *t == tool).map(|(_, v)| v.as_slice());
                    for version in versions.unwrap_or_default().iter().filter(|v| **v > now) {
                        options.push(option(role, tool, version, format!("{}, now {now}", role.noun())));
                    }
                }
                if options.is_empty() && !current.is_empty() {
                    let verb = if current.len() == 1 { "is" } else { "are" };
                    view.message = Some(format!("{} {verb} up to date.", current.join(" and ")));
                }
            }
        }
        if !listed.errors.is_empty() {
            view.message = Some(listed.errors.join(" "));
        }
        let query = picker.query.to_lowercase();
        view.options = options
            .into_iter()
            .filter(|o| query.is_empty() || format!("{} {} {}", o.tool, o.version, o.detail).to_lowercase().contains(&query))
            .collect();
        Some(view)
    }

    /// The tool and version a role uses now: its resolved version, or its
    /// exact pin.
    fn in_use(&self, role: Role) -> Option<(Tool, Version)> {
        let slot = self.slots[role.index()].as_ref()?;
        let (tool, request) = slot.want.as_ref()?;
        let version = match request {
            Request::Exact(version) => version.clone(),
            Request::Range { .. } => Version::parse(&slot.version).ok()?,
        };
        Some((*tool, version))
    }

    /// Looks at the root's lockfiles again in the background (one changed
    /// on disk).
    pub(crate) fn check_lockfiles(&mut self, jobs: &Jobs) {
        self.lockfile_generation += 1;
        let generation = self.lockfile_generation;
        let (id, root) = (self.project, self.root.clone());
        jobs.spawn("check lockfiles", move || {
            let lockfiles = Lockfiles::find(&root);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                if toolchain.lockfile_generation == generation {
                    let was_foreign = toolchain.lockfiles.foreign.map(|(_, tool)| tool);
                    toolchain.lockfiles = lockfiles;
                    // A foreign lockfile came or went in an unpinned project:
                    // its package manager changes, so set the roles up again.
                    let decides = !toolchain.loading && !toolchain.package_manager_pinned;
                    if decides && toolchain.slots[Role::PackageManager.index()].is_some()
                        && was_foreign != lockfiles.foreign.map(|(_, tool)| tool)
                    {
                        toolchain.load(&jobs);
                    }
                }
                update_problems(core, id);
            })
        });
    }

    /// The package manager is a foreign one (npm, Yarn, …; ticket #51):
    /// its name and the warning that says what is off, on the lockfile that
    /// made it so, else on `packageManager`.
    pub(crate) fn foreign_package_manager(&self) -> Option<(String, Problem)> {
        let slot = self.slots[Role::PackageManager.index()].as_ref()?;
        if !matches!(slot.state, SlotState::Off) {
            return None;
        }
        let name = &slot.name;
        let off = format!("Genea doesn't run {name}, so installing dependencies and running scripts are off.");
        let (path, at, message) = match self.lockfiles.foreign {
            Some((lockfile, tool)) if !self.package_manager_pinned => {
                let article = if tool == "npm" { "an" } else { "a" };
                (lockfile, TextPosition::default(), format!("{lockfile} is {article} {tool} lockfile. {off}"))
            }
            _ => ("package.json", self.package_manager_at, format!("packageManager pins {name}. {off}")),
        };
        Some((name.clone(), Problem { severity: Severity::Warning, path: path.into(), start: at, end: at, message }))
    }

    /// The lockfile cross-check: `packageManager` (or Genea's default)
    /// decides, and a root lockfile of the other package manager, or of npm
    /// or Yarn, is a warning on `package.json`. A foreign or invalid pin
    /// isn't checked here.
    fn lockfile_problems(&self) -> Vec<Problem> {
        let Some(slot) = &self.slots[Role::PackageManager.index()] else { return Vec::new() };
        let Some((tool, _)) = &slot.want else { return Vec::new() };
        let Lockfiles { pnpm, bun, foreign } = self.lockfiles;
        let at = if slot.defaulted { TextPosition::default() } else { self.package_manager_at };
        let warning = |message| Problem { severity: Severity::Warning, path: "package.json".into(), start: at, end: at, message };
        // A foreign lockfile in a project that pins pnpm or Bun (unpinned,
        // it would have made the package manager foreign).
        let foreign = foreign.filter(|_| !slot.defaulted).map(|(stale, kind)| {
            let article = if kind == "npm" { "an" } else { "a" };
            warning(format!(
                "packageManager pins {tool}, but {stale} is {article} {kind} lockfile. Genea uses {tool}, so {stale} goes stale."
            ))
        });
        let (stale, stale_kind) = match tool {
            Tool::Bun => (pnpm.then_some(LOCKFILES[0]), Tool::Pnpm),
            _ => (bun, Tool::Bun),
        };
        let Some(stale) = stale else { return foreign.into_iter().collect() };
        let message = match (pnpm && bun.is_some(), slot.defaulted) {
            (true, defaulted) => {
                let decides = if defaulted {
                    "This project doesn't pin its package manager".to_owned()
                } else {
                    format!("packageManager pins {tool}")
                };
                format!(
                    "Both {} and {} are at the project root. {decides}, so Genea uses {tool} and {stale} goes stale.",
                    LOCKFILES[0],
                    bun.unwrap_or_default(),
                )
            }
            (false, true) => format!(
                "This project doesn't pin its package manager, so Genea uses {tool}, but {stale} is a {stale_kind} lockfile."
            ),
            (false, false) => format!(
                "packageManager pins {tool}, but {stale} is a {stale_kind} lockfile. Genea uses {tool}, so {stale} goes stale."
            ),
        };
        std::iter::once(warning(message)).chain(foreign).collect()
    }

    /// The roles' tools that are in the store, runtime first: the project
    /// environment puts each [`Installed::bin_dir`] first on PATH (#36).
    pub(crate) fn installed(&self) -> impl Iterator<Item = &Installed> {
        self.slots.iter().flatten().filter_map(|slot| match &slot.state {
            SlotState::Ready(installed) => Some(installed),
            _ => None,
        })
    }

    /// Whether `package.json` is being read.
    pub(crate) fn is_loading(&self) -> bool {
        self.loading
    }

    /// Whether the project installs with a package manager Genea runs:
    /// `package.json` is read and pins pnpm or Bun, or nothing (pnpm by
    /// default). Not for a foreign or invalid pin.
    pub(crate) fn can_install(&self) -> bool {
        matches!(&self.slots[Role::PackageManager.index()], Some(Slot { want: Some(_), .. }))
    }

    /// The package manager's command name (`pnpm`, `bun`), or why Genea
    /// doesn't run it.
    pub(crate) fn package_manager_name(&self) -> Result<&'static str, String> {
        match &self.slots[Role::PackageManager.index()] {
            None => Err("Couldn't read package.json, so Genea doesn't know the package manager.".into()),
            Some(Slot { want: Some((tool, _)), .. }) => Ok(tool.id()),
            Some(Slot { state: SlotState::Invalid(reason), .. }) => Err(format!("package.json: {reason}")),
            Some(slot) => Err(format!("Genea doesn't run {}, so it can't install this project's dependencies.", slot.name)),
        }
    }

    /// The package manager's executable in the store, or why there is none.
    pub(crate) fn package_manager_program(&self) -> Result<PathBuf, String> {
        let name = self.package_manager_name()?;
        let slot = self.slots[Role::PackageManager.index()].as_ref().expect("named above");
        match &slot.state {
            SlotState::Ready(installed) => Ok(installed.bin_dir.join(name)),
            SlotState::Failed(reason) => Err(format!("Couldn't download {} {}: {reason}", slot.name, slot.version)),
            _ => Err(format!("{} {} isn't downloaded yet.", slot.name, slot.version)),
        }
    }

    /// The runtime's executable in the store (`node` or `bun`), or why there
    /// is none (ticket #49: Oxlint and Oxfmt run on it).
    pub(crate) fn runtime_program(&self) -> Result<PathBuf, String> {
        let Some(slot) = &self.slots[Role::Runtime.index()] else {
            return Err("Couldn't read package.json, so Genea doesn't know the runtime.".into());
        };
        match (&slot.want, &slot.state) {
            (Some((tool, _)), SlotState::Ready(installed)) => Ok(installed.bin_dir.join(tool.id())),
            (_, SlotState::Invalid(reason)) => Err(format!("package.json: {reason}")),
            (None, _) => Err(format!("Genea doesn't run {}.", slot.name)),
            (_, SlotState::Failed(reason)) => Err(format!("Couldn't download {} {}: {reason}", slot.name, slot.version)),
            _ => Err(format!("{} {} isn't downloaded yet.", slot.name, slot.version)),
        }
    }

    /// Whether every role has settled: its tool is ready, or it failed or
    /// is off. Nothing is being read, resolved or downloaded.
    pub(crate) fn is_settled(&self) -> bool {
        !self.loading
            && self
                .slots
                .iter()
                .flatten()
                .all(|slot| !matches!(slot.state, SlotState::Resolving | SlotState::Downloading(_)))
    }

    pub(crate) fn view(&self) -> ToolchainView {
        let view = |slot: &Option<Slot>| {
            slot.as_ref().map(|slot| ToolView {
                tool: slot.name.clone(),
                version: slot.version.clone(),
                state: match &slot.state {
                    SlotState::Resolving => ToolState::Resolving,
                    SlotState::Downloading(progress) => ToolState::Downloading { percent: progress.percent() },
                    SlotState::Ready(_) => ToolState::Ready,
                    SlotState::Failed(_) | SlotState::Invalid(_) => ToolState::Failed,
                    SlotState::Off => ToolState::Off,
                },
            })
        };
        ToolchainView { runtime: view(&self.slots[0]), package_manager: view(&self.slots[1]) }
    }

    /// The status-bar item while downloads run.
    pub(crate) fn status(&self) -> Option<String> {
        let downloads: Vec<String> = self
            .slots
            .iter()
            .flatten()
            .filter_map(|slot| match &slot.state {
                SlotState::Downloading(progress) => Some(match progress.percent() {
                    Some(percent) => format!("{} {} {percent}%", slot.name, slot.version),
                    None => format!("{} {} {} MB", slot.name, slot.version, progress.received / 1_000_000),
                }),
                _ => None,
            })
            .collect();
        (!downloads.is_empty()).then(|| format!("Downloading {}", downloads.join(", ")))
    }

    pub(crate) fn notices(&self) -> Vec<Notice> {
        let mut notices: Vec<Notice> =
            self.problems.iter().map(|message| Notice { message: message.clone(), action: None }).collect();
        for slot in self.slots.iter().flatten() {
            match &slot.state {
                SlotState::Failed(reason) => notices.push(Notice {
                    message: format!("Couldn't download {} {}: {reason}", slot.name, slot.version),
                    action: Some(NoticeAction { label: "Retry".into(), command: Command::RetryToolchain }),
                }),
                SlotState::Invalid(reason) => {
                    notices.push(Notice { message: format!("package.json: {reason}"), action: None })
                }
                _ => {}
            }
        }
        let defaulted: Vec<(Role, &Slot)> = Role::ALL
            .into_iter()
            .filter_map(|role| self.slots[role.index()].as_ref().filter(|s| s.defaulted).map(|s| (role, s)))
            .collect();
        if !defaulted.is_empty() {
            let roles: Vec<&str> = defaulted.iter().map(|(role, _)| role.noun()).collect();
            let tools: Vec<String> = defaulted.iter().map(|(_, slot)| format!("{} {}", slot.name, slot.version)).collect();
            notices.push(Notice {
                message: format!(
                    "This project doesn't pin its {}, so Genea uses {}.",
                    roles.join(" or "),
                    tools.join(" and ")
                ),
                action: Some(NoticeAction { label: "Pin these versions".into(), command: Command::PinToolchainDefaults }),
            });
        }
        notices
    }
}

/// A picker option that pins `tool` `version` for `role`.
fn option(role: Role, tool: Tool, version: &Version, detail: String) -> ToolchainOption {
    let v = version.to_string();
    let command = match (role, tool) {
        (Role::Runtime, Tool::Bun) => Command::SetRuntime(RuntimePin::Bun(v.clone())),
        (Role::Runtime, _) => Command::SetRuntime(RuntimePin::Node(v.clone())),
        (Role::PackageManager, Tool::Bun) => Command::SetPackageManager(PackageManagerPin::Bun(v.clone())),
        (Role::PackageManager, _) => Command::SetPackageManager(PackageManagerPin::Pnpm(v.clone())),
    };
    ToolchainOption { tool: tool.display_name().into(), version: v, detail, command }
}

impl Slot {
    fn new(name: &str, want: Option<(Tool, Request)>, defaulted: bool) -> Self {
        let version = want.as_ref().map(|(_, request)| request.to_string()).unwrap_or_default();
        Slot { name: name.to_owned(), want, defaulted, version, state: SlotState::Resolving, generation: 0 }
    }
}

/// "Remove unused toolchains": deletes, in the background, every store
/// version that no recent or open project uses, and tells project `id` what
/// went. A project uses what its root `package.json` pins now: an exact
/// version, the newest stored match of a range, or Genea's default for a
/// role it doesn't pin. What an open project is running stays too.
pub(crate) fn remove_unused(core: &mut Core, id: ProjectId) {
    let mut roots: Vec<PathBuf> = core.recent_roots().to_vec();
    let mut keep: HashSet<(Tool, Version)> = HashSet::new();
    for project in core.open_projects() {
        roots.push(project.root().to_owned());
        let running = project.toolchain.iter().flat_map(Toolchain::installed);
        keep.extend(running.map(|installed| (installed.tool, installed.version.clone())));
    }
    let store = core.toolchain.store.clone();
    core.jobs.spawn("remove unused toolchains", move || {
        for root in roots {
            let Ok(text) = fs::read_to_string(root.join("package.json")) else { continue };
            let Ok(pins) = pins::read(&text) else { continue };
            for (pin, default) in [(pins.runtime, Tool::Node), (pins.package_manager, Tool::Pnpm)] {
                match pin {
                    Pin::Unpinned => {
                        keep.insert((default, default.default_version()));
                    }
                    Pin::Pinned { tool, request } => {
                        if let Some(version) = request.newest_match(&store.installed(tool)) {
                            keep.insert((tool, version.clone()));
                        }
                    }
                    Pin::Foreign(_) | Pin::Invalid(_) => {}
                }
            }
        }
        let (mut removed, mut failed) = (Vec::new(), Vec::new());
        for tool in Tool::ALL {
            let mut versions = store.installed(tool);
            versions.sort();
            for version in versions.into_iter().filter(|v| !keep.contains(&(tool, v.clone()))) {
                match store.remove(tool, &version) {
                    Ok(()) => removed.push(format!("{tool} {version}")),
                    Err(error) => failed.push(format!("{tool} {version} ({error})")),
                }
            }
        }
        let mut message = match (removed.len(), failed.is_empty()) {
            (0, true) => "There are no unused toolchain versions to remove.".to_owned(),
            (0, false) => String::new(),
            (1, _) => format!("Removed 1 unused toolchain version: {}. ", removed[0]),
            (n, _) => format!("Removed {n} unused toolchain versions: {}. ", removed.join(", ")),
        };
        if !failed.is_empty() {
            message.push_str(&format!("Couldn't remove {}.", failed.join(", ")));
        }
        let message = message.trim_end().to_owned();
        Box::new(move |core| {
            if let Some(project) = core.project_mut(id) {
                project.notify(message);
            }
        })
    });
}

/// Puts the toolchain's warnings into the project's Problems.
fn update_problems(core: &mut Core, id: ProjectId) {
    let Some(project) = core.project_mut(id) else { return };
    let problems = project.toolchain.as_ref().map(Toolchain::lockfile_problems).unwrap_or_default();
    project.replace_problems(ProblemSource::Toolchain, problems);
    project.update_foreign_tools();
}

fn toolchain_mut(core: &mut Core, id: ProjectId) -> Option<&mut Toolchain> {
    core.project_mut(id)?.toolchain.as_mut()
}

/// A role's slot, if the job that posts for `generation` is still current.
fn slot_mut(core: &mut Core, id: ProjectId, role: Role, generation: u64) -> Option<&mut Slot> {
    toolchain_mut(core, id)?.slots[role.index()].as_mut().filter(|slot| slot.generation == generation)
}
