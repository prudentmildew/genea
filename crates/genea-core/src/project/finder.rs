//! The project's side of the fuzzy finder (ticket #33): opening it,
//! starting match jobs, and choosing a result. Matching itself is in
//! `crate::finder`.

use genea_host::Host;

use super::Project;
use crate::{
    command::Command,
    finder::{self, Candidates, Finder},
    jobs::Jobs,
    view::FinderItemKind,
};

impl Project {
    pub(super) fn finder_command(&mut self, command: Command, jobs: &Jobs, _host: &dyn Host) {
        match command {
            Command::OpenFinder(mode) => {
                self.finder = Some(Finder::new(mode));
                self.match_finder(jobs);
            }
            Command::SetFinderQuery(query) => {
                if let Some(finder) = &mut self.finder {
                    finder.set_query(query);
                    self.match_finder(jobs);
                }
            }
            Command::MoveFinderSelection(by) => {
                if let Some(finder) = &mut self.finder {
                    finder.move_selection(by);
                }
            }
            Command::SelectFinderItem(index) => {
                if let Some(finder) = &mut self.finder {
                    finder.select(index);
                }
            }
            Command::AcceptFinder => {
                let Some(finder) = self.finder.take() else { return };
                match finder.selected_item().map(|item| item.kind.clone()) {
                    Some(FinderItemKind::File(path)) => self.open_file(path, None, jobs),
                    None => {}
                }
            }
            Command::CloseFinder => self.finder = None,
            _ => {}
        }
    }

    /// Matches the finder's query again if the files changed since its
    /// last match, so its results follow files being created and deleted.
    /// Called after every background result.
    pub(crate) fn refresh_finder(&mut self, jobs: &Jobs) {
        if self.finder.is_some() && self.files.version() != self.finder_files {
            self.match_finder(jobs);
        }
    }

    /// Starts a match job for the finder's query. A newer one drops its
    /// result.
    fn match_finder(&mut self, jobs: &Jobs) {
        let Some(finder) = &self.finder else { return };
        let query = finder.query.clone();
        self.finder_generation += 1;
        let generation = self.finder_generation;
        self.finder_files = self.files.version();
        let candidates = Candidates { files: self.files.file_list() };
        let id = self.id;
        jobs.spawn("match finder", move || {
            let items = finder::run_match(&query, &candidates);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.finder_generation != generation {
                    return;
                }
                if let Some(finder) = &mut project.finder {
                    finder.set_items(items);
                }
            })
        });
    }
}
