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
    view::ProjectView,
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
}

/// Core state. Background Applies get `&mut Core`.
pub(crate) struct Core {
    /// The outside world: the clipboard now; the toolchain, LSP, undo and
    /// terminal tickets reach it only through this too.
    pub(crate) host: SharedHost,
    pub(crate) jobs: Jobs,
    projects: BTreeMap<ProjectId, Project>,
    next_id: u64,
}

impl Core {
    pub(crate) fn project_mut(&mut self, id: ProjectId) -> Option<&mut Project> {
        self.projects.get_mut(&id)
    }
}

impl Workbench {
    pub fn new(host: SharedHost) -> Self {
        let (jobs, inbox) = jobs::channel();
        Workbench { core: Core { host, jobs, projects: BTreeMap::new(), next_id: 0 }, inbox }
    }

    /// Registers the change notification. `notify` is called from any
    /// thread whenever background work has finished; it must be cheap and
    /// must not call back into the workbench. It should arrange for
    /// [`pump`](Self::pump) to run on the main thread.
    pub fn set_notifier(&mut self, notify: impl Fn() + Send + Sync + 'static) {
        self.core.jobs.set_notifier(Some(Arc::new(notify)));
    }

    /// Opens the folder at `root` as a project. Opening a folder that is
    /// already open returns its existing id.
    pub fn open_project(&mut self, root: impl AsRef<Path>) -> Result<ProjectId, OpenProjectError> {
        let root = root.as_ref();
        let error = |reason| OpenProjectError { path: root.to_owned(), reason };
        let root = root.canonicalize().map_err(|e| error(e.to_string()))?;
        if !root.is_dir() {
            return Err(error("it isn't a folder".into()));
        }
        if let Some((id, _)) = self.core.projects.iter().find(|(_, p)| p.root() == root) {
            return Ok(*id);
        }
        let id = ProjectId(self.core.next_id);
        self.core.next_id += 1;
        self.core.projects.insert(id, Project::new(id, root));
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
    /// and its result is applied. Tests call this instead of sleeping.
    ///
    /// Timers on the host clock are not background work: with the test
    /// host, advance the clock first, then settle.
    pub fn settle(&mut self) -> Result<(), SettleError> {
        let deadline = Instant::now() + SETTLE_TIMEOUT;
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
