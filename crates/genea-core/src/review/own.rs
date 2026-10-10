//! Genea's own writes to a project's files (spec #19, Review: "a change is
//! Genea's own when the content equals what Genea last wrote to that path").
//!
//! Everything in Genea that writes or deletes a file in a project (a save,
//! "Open config" creating `genea.jsonc`, a toolchain pin written to
//! `package.json`, a Revert) announces it here first with
//! [`OwnWrites::writing`] and holds the returned guard until the write is
//! done. Review then recognises the content as Genea's own when the watcher
//! reports it.
//!
//! A write isn't atomic: a check that reads the file while Genea writes it
//! may see it empty or half-written. So a check drops what it read if a
//! write to that path was in flight or happened meanwhile
//! ([`OwnWrites::generation`]), and the guard asks review to check the path
//! again once the write is done.

use std::{
    collections::{HashMap, VecDeque},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use super::store::Hash;
use crate::{jobs::Jobs, workbench::ProjectId};

/// How many recent own writes are remembered per path.
const REMEMBERED: usize = 8;

/// The project's register of Genea's own writes. Cheap to clone, usable
/// from any thread.
#[derive(Clone)]
pub(crate) struct OwnWrites {
    paths: Arc<Mutex<HashMap<PathBuf, Writes>>>,
    jobs: Jobs,
    project: ProjectId,
}

#[derive(Default)]
struct Writes {
    /// Contents Genea wrote, oldest first, and not yet seen by review;
    /// `None` is a delete.
    contents: VecDeque<Option<Hash>>,
    /// Writes that have started and not finished.
    in_flight: usize,
    /// Bumped when a write starts and when it ends. Entries are never
    /// removed, so it never repeats.
    generation: u64,
}

/// A write in progress. Dropping it marks the write done and has review
/// check the path again.
pub(crate) struct Writing {
    own: OwnWrites,
    path: PathBuf,
}

impl OwnWrites {
    pub(crate) fn new(project: ProjectId, jobs: &Jobs) -> Self {
        OwnWrites { paths: Arc::default(), jobs: jobs.clone(), project }
    }

    /// Call just before Genea writes `contents` (`None`: deletes the file)
    /// to `path` (relative to the project root), and hold the guard until
    /// the write is done.
    pub(crate) fn writing(&self, path: &Path, contents: Option<Hash>) -> Writing {
        let mut paths = self.paths.lock().unwrap();
        let writes = paths.entry(path.to_owned()).or_default();
        writes.contents.push_back(contents);
        if writes.contents.len() > REMEMBERED {
            writes.contents.pop_front();
        }
        writes.in_flight += 1;
        writes.generation += 1;
        Writing { own: self.clone(), path: path.to_owned() }
    }

    /// Identifies the state of Genea's writes to `path`: `None` while one is
    /// in flight. A read of the file is only trusted if this is `Some` and
    /// the same before and after it.
    pub(crate) fn generation(&self, path: &Path) -> Option<u64> {
        match self.paths.lock().unwrap().get(path) {
            Some(writes) if writes.in_flight > 0 => None,
            Some(writes) => Some(writes.generation),
            None => Some(0),
        }
    }

    /// Whether Genea wrote this content (`None`: deleted the file) to
    /// `path` and review hasn't seen it yet.
    pub(crate) fn is_own(&self, path: &Path, contents: Option<Hash>) -> bool {
        self.paths.lock().unwrap().get(path).is_some_and(|w| w.contents.contains(&contents))
    }

    /// Review has seen this content of `path`: forgets it and every write to
    /// the path before it.
    pub(crate) fn seen(&self, path: &Path, contents: Option<Hash>) {
        let mut paths = self.paths.lock().unwrap();
        let Some(writes) = paths.get_mut(path) else { return };
        if let Some(at) = writes.contents.iter().position(|c| *c == contents) {
            writes.contents.drain(..=at);
        }
    }

    /// Forgets every finished write to `path` (Keep makes whatever is on
    /// disk the baseline, so what Genea wrote before no longer matters).
    pub(crate) fn forget(&self, path: &Path) {
        if let Some(writes) = self.paths.lock().unwrap().get_mut(path) {
            writes.contents.clear();
        }
    }
}

impl Drop for Writing {
    fn drop(&mut self) {
        {
            let mut paths = self.own.paths.lock().unwrap();
            if let Some(writes) = paths.get_mut(&self.path) {
                writes.in_flight -= 1;
                writes.generation += 1;
            }
        }
        let (project, path) = (self.own.project, std::mem::take(&mut self.path));
        self.own.jobs.busy().finish(Box::new(move |core| {
            let jobs = core.jobs.clone();
            if let Some(project) = core.project_mut(project) {
                project.review.check(path, &jobs);
            }
        }));
    }
}
