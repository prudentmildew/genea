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
//! nothing.

use std::{fs, io, path::PathBuf, sync::Arc};

use genea_host::SharedHost;
use genea_toolchain::{Installed, Pin, Pins, Progress, Request, Store, Tool, Version, pins};

use crate::{
    command::Command,
    jobs::Jobs,
    view::{Notice, NoticeAction, ToolState, ToolView, ToolchainView},
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
    pub(crate) fn new(project: ProjectId, root: PathBuf, context: ToolchainContext) -> Self {
        Toolchain { project, root, context, slots: [None, None], problems: Vec::new(), load_generation: 0 }
    }

    /// Reads `package.json` in the background, then starts every role.
    pub(crate) fn load(&mut self, jobs: &Jobs) {
        self.load_generation += 1;
        let generation = self.load_generation;
        let (id, path) = (self.project, self.root.join("package.json"));
        jobs.spawn("read toolchain pins", move || {
            let pins = match fs::read_to_string(&path) {
                Ok(text) => Some(pins::read(&text)),
                Err(error) if error.kind() == io::ErrorKind::NotFound => None,
                Err(error) => Some(Err(error.to_string())),
            };
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(toolchain) = toolchain_mut(core, id) else { return };
                if toolchain.load_generation == generation {
                    toolchain.loaded(pins, &jobs);
                }
            })
        });
    }

    fn loaded(&mut self, pins: Option<Result<Pins, String>>, jobs: &Jobs) {
        self.problems.clear();
        self.slots = [None, None];
        let pins = match pins {
            None => return,
            Some(Ok(pins)) => pins,
            Some(Err(reason)) => {
                self.problems.push(format!("Couldn't read package.json: {reason}"));
                return;
            }
        };
        let defaults = [Tool::Node, Tool::Pnpm];
        for (role, pin) in Role::ALL.into_iter().zip([pins.runtime, pins.package_manager]) {
            self.slots[role.index()] = Some(match pin {
                Pin::Unpinned => {
                    let tool = defaults[role.index()];
                    let version = tool.default_version();
                    Slot::new(tool.display_name(), Some((tool, Request::Exact(version))), true)
                }
                Pin::Pinned { tool, request } => Slot::new(tool.display_name(), Some((tool, request)), false),
                Pin::Foreign(name) => {
                    let mut slot = Slot::new(&name, None, false);
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
        jobs.spawn("pin toolchain defaults", move || {
            let written = fs::read_to_string(&path).map_err(|e| e.to_string()).and_then(|text| {
                let text = pins::write(
                    &text,
                    runtime.as_ref().map(|(t, v)| (*t, v)),
                    package_manager.as_ref().map(|(t, v)| (*t, v)),
                )?;
                fs::write(&path, text).map_err(|e| e.to_string())
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

    /// The roles' tools that are in the store, runtime first: what #36 puts
    /// first on PATH (each [`Installed::bin_dir`]).
    #[allow(dead_code)] // Used by the environment ticket (#36).
    pub(crate) fn installed(&self) -> impl Iterator<Item = &Installed> {
        self.slots.iter().flatten().filter_map(|slot| match &slot.state {
            SlotState::Ready(installed) => Some(installed),
            _ => None,
        })
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

impl Slot {
    fn new(name: &str, want: Option<(Tool, Request)>, defaulted: bool) -> Self {
        let version = want.as_ref().map(|(_, request)| request.to_string()).unwrap_or_default();
        Slot { name: name.to_owned(), want, defaulted, version, state: SlotState::Resolving, generation: 0 }
    }
}

fn toolchain_mut(core: &mut Core, id: ProjectId) -> Option<&mut Toolchain> {
    core.project_mut(id)?.toolchain.as_mut()
}

/// A role's slot, if the job that posts for `generation` is still current.
fn slot_mut(core: &mut Core, id: ProjectId, role: Role, generation: u64) -> Option<&mut Slot> {
    toolchain_mut(core, id)?.slots[role.index()].as_mut().filter(|slot| slot.generation == generation)
}
