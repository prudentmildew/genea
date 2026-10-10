//! The project's side of review (ticket #53; the review itself is in
//! `crate::review`): applying its background results, and reloading open
//! editors after a Revert.

use super::Project;
use crate::{jobs::Jobs, review::Done, watcher::FileChanges};

impl Project {
    /// A review op finished.
    pub(crate) fn review_done(&mut self, done: Done, jobs: &Jobs) {
        let outcome = self.review.done(done, jobs);
        for error in outcome.errors {
            self.notify(error);
        }
        if !outcome.reverted.is_empty() {
            // Open editors follow without waiting for the watcher.
            let paths = outcome.reverted.iter().map(|path| self.root.join(path)).collect();
            self.check_open_files(&FileChanges { paths, ..FileChanges::default() }, jobs);
        }
    }
}
