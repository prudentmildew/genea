//! The project's side of the fuzzy finder (ticket #33): opening it,
//! starting match jobs, and choosing a result. Matching itself is in
//! `crate::finder`.

use std::path::Path;

use genea_host::Host;

use super::Project;
use crate::{
    action::Action,
    command::Command,
    finder::{self, Candidates, Finder},
    jobs::Jobs,
    view::{FinderItemKind, FinderMode},
};

/// How many files Recent Files remembers.
const MAX_RECENT_FILES: usize = 50;

impl Project {
    pub(super) fn finder_command(&mut self, command: Command, jobs: &Jobs, host: &dyn Host) {
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
                    Some(FinderItemKind::Action(action)) => {
                        if let Some(command) = self.action_command(action) {
                            self.dispatch(command, jobs, host);
                        }
                    }
                    None => {}
                }
            }
            Command::CloseFinder => self.finder = None,
            _ => {}
        }
    }

    /// The command an action runs now. Tab actions act on the focused
    /// pane's active tab, and do nothing without one.
    fn action_command(&self, action: Action) -> Option<Command> {
        if let Some(command) = action.command() {
            return Some(command);
        }
        let (pane, tab, count) = self.active_tab()?;
        Some(match action {
            Action::CloseTab => Command::CloseTab { pane, tab },
            Action::MoveTabToOtherSide => Command::MoveTabToOtherSide { pane, tab },
            Action::SelectNextTab => Command::SelectTab { pane, tab: (tab + 1) % count },
            Action::SelectPreviousTab => Command::SelectTab { pane, tab: (tab + count - 1) % count },
            _ => return None,
        })
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
        let (mode, query) = (finder.mode, finder.query.clone());
        self.finder_generation += 1;
        let generation = self.finder_generation;
        self.finder_files = self.files.version();
        // Files that are gone leave the recent files.
        let recent = self.recent_files.iter().filter(|path| self.files.contains(path));
        let recent = recent.map(|path| path.to_string_lossy().into_owned()).collect();
        let candidates = Candidates { files: self.files.file_list(), recent };
        let id = self.id;
        jobs.spawn("match finder", move || {
            let items = finder::run_match(mode, &query, &candidates);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.finder_generation != generation {
                    return;
                }
                // ⌘E, Return goes back to the file before the current one.
                let current = project.editor.as_ref().map(|e| e.path());
                let back = mode == FinderMode::RecentFiles
                    && query.is_empty()
                    && items.len() > 1
                    && current.is_some_and(|path| items[0].kind == FinderItemKind::File(path.to_owned()));
                if let Some(finder) = &mut project.finder {
                    finder.set_items(items, usize::from(back));
                }
            })
        });
    }

    /// A file was opened, or its tab selected: it goes first in the recent
    /// files.
    pub(super) fn opened_file(&mut self, path: &Path) {
        self.recent_files.retain(|p| p != path);
        self.recent_files.insert(0, path.to_owned());
        self.recent_files.truncate(MAX_RECENT_FILES);
    }
}
