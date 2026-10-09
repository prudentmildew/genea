//! The workbench: the core API's single entry point.

use std::{
    collections::BTreeMap,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use genea_host::SharedHost;

use crate::{
    command::Command,
    jobs::{self, Inbox, Jobs},
    project::Project,
    recent::RecentProjects,
    templates::{self, Creations, NewProject, ProjectCreation},
    toolchain::ToolchainContext,
    update::{self, UpdateNotice},
    view::{ProjectView, WelcomeView},
};

/// How long `settle` waits for background work before giving up. Real time,
/// not host time: it guards tests against hangs.
const SETTLE_TIMEOUT: Duration = Duration::from_secs(10);

/// Identifies an open project. Not reused after the project closes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProjectId(u64);

/// Genea's core: open projects, commands in, view state out.
///
/// The workbench lives on the main thread. Commands apply synchronously;
/// slow work (file reads, and later parsing, LSP traffic, git, search) runs
/// in the background and comes back through [`pump`](Self::pump).
///
/// - The **app** sets a [notifier](Self::set_notifier), and on each
///   notification calls `pump` on the main thread and re-reads view state.
/// - **Tests** call [`settle`](Self::settle) after commands instead.
pub struct Workbench {
    core: Core,
    inbox: Inbox,
    /// Applies run so far; `settle` counts what a watcher sync brought in.
    applied: usize,
}

/// Core state. Background Applies get `&mut Core`.
pub(crate) struct Core {
    /// The outside world: the clipboard now; the toolchain, LSP, undo and
    /// terminal tickets reach it only through this too.
    pub(crate) host: SharedHost,
    /// The host and the shared toolchain store, for every project (#35).
    pub(crate) toolchain: ToolchainContext,
    pub(crate) jobs: Jobs,
    projects: BTreeMap<ProjectId, Project>,
    next_id: u64,
    recent: RecentProjects,
    pub(crate) creations: Creations,
    /// A newer Genea release, once the update check has found one.
    pub(crate) update_notice: Option<UpdateNotice>,
    /// Lives as long as the core. Host timers hold a weak reference to it,
    /// so they do nothing once the workbench is gone.
    alive: Arc<()>,
}

impl Core {
    pub(crate) fn project_mut(&mut self, id: ProjectId) -> Option<&mut Project> {
        self.projects.get_mut(&id)
    }
}

impl Workbench {
    pub fn new(host: SharedHost) -> Self {
        let (jobs, inbox) = jobs::channel();
        let recent = RecentProjects::load(host.support_dir());
        let toolchain = ToolchainContext::new(host.clone());
        let creations = Creations::default();
        let (update_notice, alive) = (None, Arc::new(()));
        let core =
            Core { host, toolchain, jobs, projects: BTreeMap::new(), next_id: 0, recent, creations, update_notice, alive };
        Workbench { core, inbox, applied: 0 }
    }

    /// Registers the change notification. `notify` is called from any
    /// thread whenever background work has finished; it must be cheap and
    /// must not call back into the workbench. It should arrange for
    /// [`pump`](Self::pump) to run on the main thread.
    pub fn set_notifier(&mut self, notify: impl Fn() + Send + Sync + 'static) {
        self.core.jobs.set_notifier(Some(Arc::new(notify)));
    }

    /// Opens the folder at `root` as a project and puts it first in the
    /// recent projects. Opening a folder that is already open returns its
    /// existing id; the app then focuses that project's window. A folder
    /// that can't be opened leaves the recent projects.
    pub fn open_project(&mut self, root: impl AsRef<Path>) -> Result<ProjectId, OpenProjectError> {
        let root = root.as_ref();
        let error = |reason| OpenProjectError { path: root.to_owned(), reason };
        let checked = root.canonicalize().map_err(|e| e.to_string()).and_then(|canonical| {
            if canonical.is_dir() { Ok(canonical) } else { Err("it isn't a folder".into()) }
        });
        let root = match checked {
            Ok(root) => root,
            Err(reason) => {
                // A recent project that can't be opened any more leaves the list.
                let Core { recent, jobs, host, .. } = &mut self.core;
                recent.forget(root, jobs, host.clock());
                return Err(error(reason));
            }
        };
        let Core { recent, jobs, host, .. } = &mut self.core;
        recent.opened(&root, jobs, host.clock());
        if let Some((id, _)) = self.core.projects.iter().find(|(_, p)| p.root() == root) {
            return Ok(*id);
        }
        let id = ProjectId(self.core.next_id);
        self.core.next_id += 1;
        let project = self.core.projects.entry(id).or_insert(Project::new(id, root));
        project.start(&self.core.jobs, self.core.host.as_ref());
        project.start_toolchain(self.core.toolchain.clone(), &self.core.jobs);
        Ok(id)
    }

    /// Closes a project. Background results for it are dropped.
    pub fn close_project(&mut self, project: ProjectId) {
        self.core.projects.remove(&project);
    }

    /// The open projects, oldest first.
    pub fn projects(&self) -> Vec<ProjectId> {
        self.core.projects.keys().copied().collect()
    }

    /// What the welcome shows, or `None` while a project is open: the app
    /// shows the welcome exactly when this is `Some`.
    pub fn welcome(&self) -> Option<WelcomeView> {
        self.core.projects.is_empty().then(|| WelcomeView { recent_projects: self.core.recent.view() })
    }

    /// Applies a command to a project. Commands for a closed project are
    /// ignored.
    pub fn dispatch(&mut self, project: ProjectId, command: Command) {
        let Core { projects, jobs, host, .. } = &mut self.core;
        if let Some(project) = projects.get_mut(&project) {
            project.dispatch(command, jobs, host.as_ref());
        }
    }

    /// A snapshot of what the project's window shows, or `None` if the
    /// project isn't open.
    pub fn project(&self, project: ProjectId) -> Option<ProjectView> {
        self.core.projects.get(&project).map(Project::view)
    }

    /// Creates a project from a template in the background: writes its
    /// files into `request.folder` and initialises a git repository there.
    /// It refuses a folder that isn't empty. The outcome shows in
    /// [`project_creation`](Self::project_creation); a new request replaces
    /// the last one's state.
    pub fn create_project(&mut self, request: NewProject) {
        templates::create(&mut self.core, request);
    }

    /// The state of the last [`create_project`](Self::create_project), if
    /// any.
    pub fn project_creation(&self) -> Option<ProjectCreation> {
        self.core.creations.view()
    }

    /// Starts the release-update check: at most once a day, the first one a
    /// little after start, always off the main thread. A newer release shows
    /// up as [`update_notice`](Self::update_notice), with the change
    /// notification. Call it once, after the first window is up, with the
    /// running Genea's version (`env!("CARGO_PKG_VERSION")`). What it learns
    /// is kept in the host's support folder, so restarts don't check more.
    pub fn start_update_checks(&mut self, current_version: &str) {
        let core = &self.core;
        update::start(core.host.clone(), core.jobs.clone(), Arc::downgrade(&core.alive), current_version);
    }

    /// The newer release the update check found, if any. It isn't tied to a
    /// project: every window shows it.
    pub fn update_notice(&self) -> Option<UpdateNotice> {
        self.core.update_notice.clone()
    }

    /// Applies finished background work without waiting. Returns whether
    /// anything was applied, i.e. whether view state may have changed.
    pub fn pump(&mut self) -> bool {
        let mut applied = false;
        while let Some(apply) = self.inbox.try_next() {
            self.run(apply);
            applied = true;
        }
        applied
    }

    /// Waits until the core is quiescent: every background job has finished
    /// and its result is applied, and every project's watcher has delivered
    /// the changes made on disk before the call (and those made by the
    /// work it waited for). Tests call this instead of sleeping.
    ///
    /// Timers on the host clock are not background work: with the test
    /// host, advance the clock first, then settle.
    pub fn settle(&mut self) -> Result<(), SettleError> {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
        loop {
            self.drain(deadline)?;
            // A watcher cookie per project (see `Watcher::sync`). If nothing
            // but the cookies came back, the watchers are drained too.
            let before = self.applied;
            let cookies = self.core.projects.values().filter(|p| p.sync_watcher()).count();
            if cookies == 0 {
                return Ok(());
            }
            self.drain(deadline)?;
            if self.applied - before == cookies {
                return Ok(());
            }
        }
    }

    /// Runs Applies until no job is pending.
    fn drain(&mut self, deadline: Instant) -> Result<(), SettleError> {
        loop {
            self.pump();
            if self.inbox.pending() == 0 {
                return Ok(());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.inbox.next_timeout(left) {
                Some(apply) => self.run(apply),
                None if Instant::now() >= deadline => {
                    return Err(SettleError { pending: self.inbox.pending() });
                }
                None => {}
            }
        }
    }

    fn run(&mut self, apply: jobs::Apply) {
        // Count the job done even if its Apply re-raises a background panic.
        struct Done<'a>(&'a Inbox);
        impl Drop for Done<'_> {
            fn drop(&mut self) {
                self.0.done();
            }
        }
        let _done = Done(&self.inbox);
        self.applied += 1;
        apply(&mut self.core);
    }
}

/// A folder couldn't be opened as a project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenProjectError {
    pub path: PathBuf,
    pub reason: String,
}

impl fmt::Display for OpenProjectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "Couldn't open {} as a project: {}", self.path.display(), self.reason)
    }
}

impl std::error::Error for OpenProjectError {}

/// Background work didn't finish within the settle timeout.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettleError {
    /// Jobs still pending when settle gave up.
    pub pending: usize,
}

impl fmt::Display for SettleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "the core didn't settle within {SETTLE_TIMEOUT:?}: {} jobs pending", self.pending)
    }
}

impl std::error::Error for SettleError {}
