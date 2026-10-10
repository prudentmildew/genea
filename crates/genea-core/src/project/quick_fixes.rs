//! Quick fixes (⌥⏎) and organize imports (⌃⌥O) (ticket #45): a project's
//! side of the language servers' code actions (`crate::lsp::actions`).
//!
//! ⌥⏎ asks every ready server for its fixes at the primary caret and opens
//! the popup with the first answer (later answers add to it). Any command
//! but the quick-fix ones (and scrolling) closes it, and drops answers
//! still to come. Choosing a fix applies its edits to the open files as one
//! undo step each, provided none of them changed since it was offered.
//!
//! ⌃⌥O asks the TypeScript server for `source.organizeImports` and applies
//! it when it answers, unless the file changed meanwhile. Nothing else
//! organizes imports: not saving, not opening.

use std::{
    path::{Path, PathBuf},
    time::Instant,
};

use super::Project;
use crate::{
    command::Command,
    jobs::Jobs,
    lsp::actions::{self, ActionKind, CodeActionFix},
    view::QuickFixesView,
};

/// The project's code-action requests and the quick-fix popup.
#[derive(Default)]
pub(crate) struct QuickFixes {
    /// Bumped by every request, so an answer to an older one is dropped.
    ticket: u64,
    /// The quick-fix request and its popup.
    popup: Option<Offer>,
    /// The organize-imports request in flight.
    organize: Option<Offer>,
}

/// One code-action request and what has come back for it.
struct Offer {
    ticket: u64,
    /// Every open file's version when it was asked (absolute path): a fix
    /// applies only while the files it changes are still at these.
    versions: Vec<(PathBuf, u64)>,
    /// What the servers offered so far.
    fixes: Vec<CodeActionFix>,
    /// A server has answered (or none could be asked): the popup shows.
    answered: bool,
    selected: usize,
    /// When it was asked, on the host clock, and the jobs, for applying
    /// an answer that comes back in the background.
    now: Instant,
    jobs: Jobs,
}

impl Project {
    /// A command closes the popup unless it is one of its own, or scrolls.
    pub(super) fn quick_fixes_before(&mut self, command: &Command) {
        let keeps = matches!(
            command,
            Command::MoveQuickFixSelection(_)
                | Command::ApplyQuickFix(_)
                | Command::SetViewport { .. }
                | Command::ScrollBy { .. }
                | Command::ScrollPane { .. }
        );
        if !keeps {
            self.quick_fixes.popup = None;
        }
    }

    pub(super) fn quick_fix_command(&mut self, command: Command, now: Instant, jobs: &Jobs) {
        match command {
            Command::ShowQuickFixes => {
                self.quick_fixes.popup = self.ask_code_actions(ActionKind::QuickFix, now, jobs);
                if let Some(offer) = &mut self.quick_fixes.popup
                    && offer.ticket == 0
                {
                    // No server has the file: the popup says there is nothing.
                    offer.answered = true;
                }
            }
            Command::MoveQuickFixSelection(by) => {
                if let Some(offer) = self.quick_fixes.popup.as_mut().filter(|o| o.answered && !o.fixes.is_empty()) {
                    let count = offer.fixes.len() as isize;
                    offer.selected = (offer.selected as isize + by).rem_euclid(count) as usize;
                }
            }
            Command::ApplyQuickFix(index) => {
                if let Some(offer) = self.quick_fixes.popup.take()
                    && let Some(fix) = offer.fixes.get(index)
                {
                    self.apply_fix(fix, &offer.versions, now, jobs);
                }
            }
            Command::CloseQuickFixes => self.quick_fixes.popup = None,
            Command::OrganizeImports => {
                self.quick_fixes.organize = self.ask_code_actions(ActionKind::OrganizeImports, now, jobs);
            }
            _ => {}
        }
    }

    /// Asks the servers about the focused file at its primary selection.
    /// `None` without a focused file. An offer with ticket 0 went to no
    /// server.
    fn ask_code_actions(&mut self, kind: ActionKind, now: Instant, jobs: &Jobs) -> Option<Offer> {
        // The servers must have the text as it is now.
        self.sync_language();
        let editor = self.editor.as_ref()?;
        let (path, text, selection) = (editor.path().to_owned(), editor.text().clone(), editor.primary_selection());
        let versions = self.open_editors().map(|e| (self.root.join(e.path()), e.version())).collect();
        self.quick_fixes.ticket += 1;
        let ticket = self.quick_fixes.ticket;
        let mut asked = false;
        for server in self.language_servers_mut(kind) {
            asked |= server.code_actions(&path, &text, selection.clone(), kind, ticket);
        }
        let ticket = if asked { ticket } else { 0 };
        Some(Offer { ticket, versions, fixes: Vec::new(), answered: false, selected: 0, now, jobs: jobs.clone() })
    }

    /// A server's answer to a code-action request.
    pub(super) fn code_actions_answered(&mut self, ticket: u64, fixes: Vec<CodeActionFix>) {
        if let Some(offer) = self.quick_fixes.popup.as_mut().filter(|o| o.ticket == ticket) {
            offer.fixes.extend(fixes);
            offer.answered = true;
        } else if self.quick_fixes.organize.as_ref().is_some_and(|o| o.ticket == ticket) {
            let organize = |fix: &&CodeActionFix| fix.kind.split('.').take(2).eq(["source", "organizeImports"]);
            if let Some(fix) = fixes.iter().find(organize) {
                let offer = self.quick_fixes.organize.take().expect("checked above");
                self.apply_fix(fix, &offer.versions, offer.now, &offer.jobs);
            }
        }
    }

    /// Applies a fix's edits to the open files it changes, each as one undo
    /// step. Nothing is applied if any of them changed since it was offered
    /// or isn't open: a notice says why.
    fn apply_fix(&mut self, fix: &CodeActionFix, versions: &[(PathBuf, u64)], now: Instant, jobs: &Jobs) {
        let mut targets = Vec::new();
        for (file, edits) in &fix.edits {
            let path = file.strip_prefix(&self.root).unwrap_or(file).to_owned();
            let Some(editor) = self.open_editor(&path) else {
                let name = path.display();
                return self.notify(format!("\"{}\" wasn't applied: it changes {name}, which isn't open.", fix.title));
            };
            if !versions.iter().any(|(f, version)| f == file && *version == editor.version()) {
                let name = path.display();
                return self.notify(format!("\"{}\" wasn't applied: {name} changed since it was offered.", fix.title));
            }
            let ranges: Vec<_> = edits
                .iter()
                .map(|edit| (actions::char_range(editor.text(), edit, fix.encoding), edit.text.clone()))
                .collect();
            targets.push((path, ranges));
        }
        let rows = self.viewport_rows;
        for (path, ranges) in targets {
            if let Some(editor) = self.open_editor_mut(&path) {
                editor.apply_text_edits(ranges, now, rows);
            }
            self.after_fix(&path, jobs);
        }
    }

    /// What follows an edit to an open file: its syntax and git markers.
    fn after_fix(&mut self, path: &Path, jobs: &Jobs) {
        self.reparse_file(path, jobs);
        self.diff_file(path, jobs);
    }

    /// The quick-fix popup's view state, once there is something to show.
    pub(super) fn quick_fixes_view(&self) -> Option<QuickFixesView> {
        let offer = self.quick_fixes.popup.as_ref().filter(|o| o.answered)?;
        let items = offer.fixes.iter().map(|fix| fix.title.clone()).collect();
        Some(QuickFixesView { items, selected: offer.selected })
    }
}
