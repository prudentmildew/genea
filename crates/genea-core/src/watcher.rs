//! The project watcher: one `notify` watcher (FSEvents) per project, over
//! the whole project folder, `node_modules` and `.git` included (spec #19,
//! Building blocks → Watching).
//!
//! Changes come back to the main thread in batches, as
//! [`Project::files_changed`](crate::project::Project::files_changed): the
//! first event after a batch was taken schedules the next one, and every
//! event until it runs joins it. Each area that cares about files on disk
//! (the config, the file index and open editors now; review later) reacts
//! there.
//!
//! **Settle.** The watcher is a long-lived background source, so `settle`
//! must wait for it, but FSEvents delivers asynchronously: a test that
//! writes a file and calls `settle` must not return before the event has
//! arrived. [`Watcher::sync`] writes a *cookie* file into a folder in the
//! application-support folder that the same FSEvents stream watches, and
//! holds a busy token until the cookie's event comes back. One stream
//! delivers events in order, so by then every earlier change has been
//! delivered and batched.

use std::{
    collections::{BTreeSet, HashMap},
    fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

use notify::{RecursiveMode, Watcher as _};

use crate::{
    jobs::{Busy, Jobs},
    workbench::ProjectId,
};

/// What changed on disk since the last batch.
#[derive(Debug, Default)]
pub(crate) struct FileChanges {
    /// Absolute paths of files and folders created, changed or removed.
    pub(crate) paths: BTreeSet<PathBuf>,
    /// Those among `paths` that the watcher saw appear: created, or renamed
    /// into place (language servers tell creations from changes).
    pub(crate) created: BTreeSet<PathBuf>,
    /// Events were lost or coalesced (e.g. FSEvents' "must scan subdirs"):
    /// anything may have changed, so rescan.
    pub(crate) rescan: bool,
}

impl FileChanges {
    fn is_empty(&self) -> bool {
        self.paths.is_empty() && !self.rescan
    }
}

pub(crate) struct Watcher {
    // Held for its Drop, which stops the stream.
    _watcher: notify::RecommendedWatcher,
    state: Arc<Mutex<State>>,
    jobs: Jobs,
    sync_dir: PathBuf,
}

#[derive(Default)]
struct State {
    pending: FileChanges,
    /// A batch is posted and hasn't run yet: new events join it.
    scheduled: bool,
    /// Cookies written by `sync` and not seen yet.
    cookies: HashMap<PathBuf, Busy>,
}

/// Makes cookie names unique across watchers and workbenches.
static NEXT_COOKIE: AtomicU64 = AtomicU64::new(0);

impl Watcher {
    /// Starts watching `root` (canonical). `support_dir` is the
    /// application-support folder, where the cookies for [`sync`](Self::sync)
    /// go.
    pub(crate) fn start(root: &Path, support_dir: &Path, project: ProjectId, jobs: &Jobs) -> io::Result<Watcher> {
        let sync_dir = support_dir.join("watch-sync");
        fs::create_dir_all(&sync_dir)?;
        // FSEvents reports real paths (/private/var, not /var).
        let sync_dir = sync_dir.canonicalize()?;

        let state = Arc::new(Mutex::new(State::default()));
        let handler = {
            let (state, jobs, root, sync_dir) = (state.clone(), jobs.clone(), root.to_owned(), sync_dir.clone());
            move |event: notify::Result<notify::Event>| {
                let shared = state.clone();
                let mut state = state.lock().unwrap();
                match event {
                    Ok(event) => {
                        state.pending.rescan |= event.need_rescan();
                        let appeared = matches!(
                            event.kind,
                            notify::EventKind::Create(_) | notify::EventKind::Modify(notify::event::ModifyKind::Name(_))
                        );
                        for path in event.paths {
                            if path.starts_with(&sync_dir) {
                                if let Some(busy) = state.cookies.remove(&path) {
                                    let _ = fs::remove_file(&path);
                                    busy.finish(Box::new(|_| {}));
                                }
                            } else if path.starts_with(&root) {
                                if appeared {
                                    state.pending.created.insert(path.clone());
                                }
                                state.pending.paths.insert(path);
                            }
                        }
                    }
                    Err(_) => state.pending.rescan = true,
                }
                if !state.scheduled && !state.pending.is_empty() {
                    state.scheduled = true;
                    // The batch is taken when it runs, so that every event
                    // until then joins it.
                    jobs.busy().finish(Box::new(move |core| {
                        let changes = {
                            let mut state = shared.lock().unwrap();
                            state.scheduled = false;
                            std::mem::take(&mut state.pending)
                        };
                        let jobs = core.jobs.clone();
                        if let Some(project) = core.project_mut(project) {
                            project.files_changed(changes, &jobs);
                        }
                    }));
                }
            }
        };
        let mut watcher = notify::recommended_watcher(handler).map_err(io::Error::other)?;
        {
            let mut paths = watcher.paths_mut();
            paths.add(root, RecursiveMode::Recursive).map_err(io::Error::other)?;
            paths.add(&sync_dir, RecursiveMode::NonRecursive).map_err(io::Error::other)?;
            paths.commit().map_err(io::Error::other)?;
        }
        Ok(Watcher { _watcher: watcher, state, jobs: jobs.clone(), sync_dir })
    }

    /// Writes a cookie and keeps the core busy until the watcher has seen
    /// it, so `settle` waits for every change made before this call.
    /// Returns whether a cookie was written.
    pub(crate) fn sync(&self) -> bool {
        let name = format!("{}-{}", std::process::id(), NEXT_COOKIE.fetch_add(1, Ordering::Relaxed));
        let cookie = self.sync_dir.join(name);
        let busy = self.jobs.busy();
        self.state.lock().unwrap().cookies.insert(cookie.clone(), busy);
        if fs::write(&cookie, b"").is_err() {
            // Dropping the token ends the wait.
            self.state.lock().unwrap().cookies.remove(&cookie);
            return false;
        }
        true
    }
}
