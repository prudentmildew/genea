//! Recent projects: the global list the welcome shows (ticket #58).
//!
//! The list lives in `recent-projects.txt` in Genea's application-support
//! folder, one project root per line, most recent first. It is read once
//! when the workbench is created: a small file read at start, like the
//! folder check in `open_project`, and the welcome needs it for its first
//! frame. Changes are saved in the background a short pause after the last
//! one (on the host clock), so opening several projects writes the file once.

use std::{
    ffi::OsStr,
    fs, io,
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use genea_host::Clock;

use crate::{jobs::Jobs, view::RecentProject};

const FILE_NAME: &str = "recent-projects.txt";

/// How many projects the list keeps.
const MAX_RECENT: usize = 10;

/// The pause between the last change and the save.
const SAVE_DELAY: Duration = Duration::from_secs(1);

pub(crate) struct RecentProjects {
    /// Project roots, most recent first.
    roots: Vec<PathBuf>,
    file: PathBuf,
    save: Arc<Mutex<PendingSave>>,
}

/// The newest unsaved list, shared with the save timer and job.
#[derive(Default)]
struct PendingSave {
    roots: Option<Vec<PathBuf>>,
    timer_set: bool,
}

impl RecentProjects {
    /// Reads the list from `support_dir`. A missing or unreadable file is an
    /// empty list.
    pub(crate) fn load(support_dir: &Path) -> Self {
        let file = support_dir.join(FILE_NAME);
        let mut roots = fs::read(&file).map(|bytes| parse(&bytes)).unwrap_or_default();
        roots.truncate(MAX_RECENT);
        RecentProjects { roots, file, save: Arc::default() }
    }

    /// Moves `root` to the front of the list.
    pub(crate) fn opened(&mut self, root: &Path, jobs: &Jobs, clock: &dyn Clock) {
        self.roots.retain(|r| r != root);
        self.roots.insert(0, root.to_owned());
        self.roots.truncate(MAX_RECENT);
        self.changed(jobs, clock);
    }

    /// Removes `root`, e.g. when its folder is gone.
    pub(crate) fn forget(&mut self, root: &Path, jobs: &Jobs, clock: &dyn Clock) {
        let before = self.roots.len();
        self.roots.retain(|r| r != root);
        if self.roots.len() != before {
            self.changed(jobs, clock);
        }
    }

    pub(crate) fn view(&self) -> Vec<RecentProject> {
        self.roots
            .iter()
            .map(|root| RecentProject {
                root: root.clone(),
                name: root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            })
            .collect()
    }

    /// Schedules a save of the current list, unless one is already waiting.
    fn changed(&mut self, jobs: &Jobs, clock: &dyn Clock) {
        let mut pending = self.save.lock().unwrap();
        pending.roots = Some(self.roots.clone());
        if pending.timer_set {
            return;
        }
        pending.timer_set = true;
        let (save, file, jobs) = (self.save.clone(), self.file.clone(), jobs.clone());
        clock.after(
            SAVE_DELAY,
            Box::new(move || {
                save.lock().unwrap().timer_set = false;
                jobs.spawn("save recent projects", move || {
                    // Take the list inside the job, under the lock, so that
                    // saves land in order and the last one is the newest.
                    let mut pending = save.lock().unwrap();
                    if let Some(roots) = pending.roots.take()
                        && let Err(error) = write(&file, &roots)
                    {
                        eprintln!("genea: couldn't save the recent projects to {}: {error}", file.display());
                    }
                    Box::new(|_| {})
                });
            }),
        );
    }
}

fn parse(bytes: &[u8]) -> Vec<PathBuf> {
    bytes
        .split(|&b| b == b'\n')
        .map(|line| PathBuf::from(OsStr::from_bytes(line)))
        .filter(|root| root.is_absolute())
        .collect()
}

/// Writes the list atomically: a crash mid-write keeps the old file.
fn write(file: &Path, roots: &[PathBuf]) -> io::Result<()> {
    let mut bytes = Vec::new();
    for root in roots {
        let line = root.as_os_str().as_bytes();
        // A newline in a folder name can't be stored in this format.
        if !line.contains(&b'\n') {
            bytes.extend_from_slice(line);
            bytes.push(b'\n');
        }
    }
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp = file.with_extension("txt.tmp");
    fs::write(&temp, bytes)?;
    fs::rename(&temp, file)
}
