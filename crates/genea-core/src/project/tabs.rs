//! Tabs and one split (ticket #31).
//!
//! The editor area has one pane, or two side by side after a split. Each
//! pane has tabs; a file has at most one tab per pane. An open file has one
//! [`Editor`] however many tabs show it, so an edit shows in both panes;
//! each tab keeps its own [`Cursor`] (caret, selection, scroll).
//!
//! The focused tab's editor lives in `Project::editor`, so every command
//! that edits "the open file" works on it unchanged. The others are parked
//! here and swapped in when their tab gets the focus. A pane without the
//! focus is drawn with its tab's cursor, which needs the editor mutably,
//! so its view is computed after every change (`refresh_views`) and kept.

use std::path::{Path, PathBuf};

use super::Project;
use crate::{
    command::{CloseChoice, Command},
    editor::{Cursor, Editor},
    jobs::Jobs,
    session::{PaneSession, TabSession},
    view::{ClosePrompt, EditorTab, EditorView, PaneView},
};

pub(super) struct Panes {
    /// Left to right: one, or two after a split. Never empty.
    sides: Vec<Pane>,
    focused: usize,
    /// Open files other than the focused one, one editor per path.
    parked: Vec<Editor>,
    /// The tab waiting for an answer to the close prompt: (pane, path).
    prompt: Option<(usize, PathBuf)>,
}

impl Default for Panes {
    fn default() -> Self {
        Panes { sides: vec![Pane::default()], focused: 0, parked: Vec::new(), prompt: None }
    }
}

#[derive(Default)]
struct Pane {
    tabs: Vec<Tab>,
    /// Index into `tabs` of the tab shown (0 without tabs).
    active: usize,
    /// The active tab's view while the pane doesn't have the focus.
    view: Option<EditorView>,
}

impl Panes {
    /// The open files other than the focused one.
    pub(super) fn parked(&self) -> &[Editor] {
        &self.parked
    }
}

impl Pane {
    fn position(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.path == path)
    }

    /// Removes a tab; the tab to its right (or else its left) becomes
    /// active if it was.
    fn remove(&mut self, tab: usize) -> Tab {
        let removed = self.tabs.remove(tab);
        if tab < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        removed
    }
}

struct Tab {
    path: PathBuf,
    /// Where this tab left its file. Stale while the tab has the focus: its
    /// editor holds the live cursor then.
    cursor: Cursor,
    /// Closes as soon as its file is saved (Save in the close prompt).
    closing: bool,
}

impl Tab {
    fn new(path: PathBuf, cursor: Cursor) -> Self {
        Tab { path, cursor, closing: false }
    }
}

/// The tab part of a `ProjectView`.
pub(super) struct TabsView {
    pub(super) panes: Vec<PaneView>,
    pub(super) focused_pane: usize,
    pub(super) can_split: bool,
    pub(super) close_prompt: Option<ClosePrompt>,
}

impl Project {
    pub(super) fn tab_command(&mut self, command: Command, jobs: &Jobs) {
        match command {
            Command::SelectTab { pane, tab } => {
                self.focus(pane, tab);
                if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
                    self.opened_file(&path);
                }
            }
            Command::FocusPane(pane) => {
                if let Some(side) = self.panes.sides.get(pane)
                    && !side.tabs.is_empty()
                {
                    self.focus(pane, side.active);
                }
            }
            Command::CloseTab { pane, tab } => self.close_tab(pane, tab),
            Command::ResolveClose(choice) => self.resolve_close(choice, jobs),
            Command::SplitRight => self.split_right(),
            Command::MoveTabToOtherSide { pane, tab } => self.move_tab(pane, tab),
            Command::CloseSplit => self.close_split(),
            Command::ScrollPane { pane, rows } => self.scroll_pane(pane, rows),
            _ => {}
        }
    }

    /// The focused pane, its active tab and how many tabs it has; `None`
    /// without tabs.
    pub(super) fn active_tab(&self) -> Option<(usize, usize, usize)> {
        let side = &self.panes.sides[self.panes.focused];
        (!side.tabs.is_empty()).then_some((self.panes.focused, side.active, side.tabs.len()))
    }

    /// The editor of an open file, focused or not.
    pub(super) fn open_editor(&self, path: &Path) -> Option<&Editor> {
        self.editor.iter().chain(&self.panes.parked).find(|e| e.path() == path)
    }

    /// Every open file's editor, focused or not.
    pub(crate) fn open_editors(&self) -> impl Iterator<Item = &Editor> {
        self.editor.iter().chain(&self.panes.parked)
    }

    pub(super) fn open_editor_mut(&mut self, path: &Path) -> Option<&mut Editor> {
        self.editor.iter_mut().chain(&mut self.panes.parked).find(|e| e.path() == path)
    }

    /// Focuses the tab of an open file, in the focused pane if it is open
    /// there. Returns whether the file was open.
    pub(super) fn focus_open_file(&mut self, path: &Path) -> bool {
        let focused = self.panes.focused;
        let order = std::iter::once(focused).chain((0..self.panes.sides.len()).filter(|&p| p != focused));
        for pane in order {
            if let Some(tab) = self.panes.sides[pane].position(path) {
                self.focus(pane, tab);
                return true;
            }
        }
        false
    }

    /// Shows a newly read file in a new tab at the end of the focused pane.
    pub(super) fn open_tab(&mut self, editor: Editor) {
        if self.focus_open_file(editor.path()) {
            return;
        }
        self.park();
        let pane = &mut self.panes.sides[self.panes.focused];
        pane.tabs.push(Tab::new(editor.path().to_owned(), Cursor::default()));
        pane.active = pane.tabs.len() - 1;
        self.panes.parked.push(editor);
        self.unpark();
    }

    /// Shows an editor in a new tab at the end of the focused pane, behind
    /// the focused tab (a rename's edits to a file that wasn't open). It
    /// gets the focus only if the pane has no other tab.
    pub(super) fn open_tab_behind(&mut self, editor: Editor) {
        if self.open_editor(editor.path()).is_some() {
            return;
        }
        let pane = &mut self.panes.sides[self.panes.focused];
        pane.tabs.push(Tab::new(editor.path().to_owned(), Cursor::default()));
        self.panes.parked.push(editor);
        if self.editor.is_none() {
            self.unpark();
        }
    }

    /// A save of `path` finished: tabs waiting on it close, unless it was
    /// edited again meanwhile.
    pub(super) fn saved(&mut self, path: &Path) {
        let modified = self.open_editor(path).is_some_and(Editor::is_modified);
        while let Some((pane, tab)) = self.closing_tab(path) {
            if modified {
                self.panes.sides[pane].tabs[tab].closing = false;
            } else {
                self.remove_tab(pane, tab);
            }
        }
    }

    /// A save of `path` failed: its tabs stay open.
    pub(super) fn save_failed(&mut self, path: &Path) {
        while let Some((pane, tab)) = self.closing_tab(path) {
            self.panes.sides[pane].tabs[tab].closing = false;
        }
    }

    fn closing_tab(&self, path: &Path) -> Option<(usize, usize)> {
        self.panes.sides.iter().enumerate().find_map(|(p, side)| {
            side.tabs.iter().position(|t| t.closing && t.path == path).map(|t| (p, t))
        })
    }

    pub(super) fn tabs_view(&self, focused_editor: Option<&EditorView>) -> TabsView {
        let panes = &self.panes;
        let title = |path: &Path| path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        TabsView {
            panes: panes
                .sides
                .iter()
                .enumerate()
                .map(|(i, side)| PaneView {
                    tabs: side
                        .tabs
                        .iter()
                        .map(|tab| EditorTab {
                            path: tab.path.clone(),
                            title: title(&tab.path),
                            modified: self.open_editor(&tab.path).is_some_and(Editor::is_modified),
                        })
                        .collect(),
                    active: (!side.tabs.is_empty()).then_some(side.active),
                    editor: if i == panes.focused { focused_editor.cloned() } else { side.view.clone() },
                })
                .collect(),
            focused_pane: panes.focused,
            can_split: panes.sides.len() == 1 && self.editor.is_some(),
            close_prompt: panes.prompt.as_ref().map(|(_, path)| ClosePrompt { path: path.clone(), title: title(path) }),
        }
    }

    /// Recomputes the views of panes without the focus. Called after every
    /// command and every background result.
    pub(crate) fn refresh_views(&mut self) {
        let Project { editor, panes: Panes { sides, focused, parked, .. }, viewport_rows, .. } = self;
        for (i, side) in sides.iter_mut().enumerate() {
            side.view = None;
            if i == *focused {
                continue;
            }
            let Some(tab) = side.tabs.get_mut(side.active) else { continue };
            let Some(editor) = editor.iter_mut().chain(parked.iter_mut()).find(|e| e.path() == tab.path) else {
                continue;
            };
            let live = editor.cursor();
            editor.set_cursor(&tab.cursor, *viewport_rows);
            tab.cursor = editor.cursor();
            side.view = Some(editor.view(*viewport_rows));
            editor.set_cursor(&live, *viewport_rows);
        }
    }

    fn focused_tab_mut(&mut self) -> Option<&mut Tab> {
        let pane = &mut self.panes.sides[self.panes.focused];
        pane.tabs.get_mut(pane.active)
    }

    /// Moves the focused editor out of `Project::editor`, keeping its cursor
    /// in its tab. Every change to the tabs runs between `park` and
    /// `unpark`.
    fn park(&mut self) {
        if let Some(editor) = self.editor.take() {
            if let Some(tab) = self.focused_tab_mut() {
                tab.cursor = editor.cursor().parked();
            }
            self.panes.parked.push(editor);
        }
    }

    /// Moves the focused tab's editor into `Project::editor`, with the tab's
    /// cursor.
    fn unpark(&mut self) {
        let rows = self.viewport_rows;
        let Some(tab) = self.focused_tab_mut() else { return };
        let (path, cursor) = (tab.path.clone(), tab.cursor.clone());
        if let Some(i) = self.panes.parked.iter().position(|e| e.path() == path) {
            let mut editor = self.panes.parked.swap_remove(i);
            editor.set_cursor(&cursor, rows);
            self.editor = Some(editor);
        }
    }

    fn focus(&mut self, pane: usize, tab: usize) {
        if self.panes.sides.get(pane).is_none_or(|side| tab >= side.tabs.len()) {
            return;
        }
        self.park();
        self.panes.focused = pane;
        self.panes.sides[pane].active = tab;
        self.unpark();
    }

    /// Closes a tab, or asks first if that would lose unsaved edits.
    fn close_tab(&mut self, pane: usize, tab: usize) {
        let Some(path) = self.panes.sides.get(pane).and_then(|s| s.tabs.get(tab)).map(|t| t.path.clone()) else {
            return;
        };
        let last_tab = self.panes.sides.iter().flat_map(|s| &s.tabs).filter(|t| t.path == path).count() == 1;
        if last_tab && self.open_editor(&path).is_some_and(Editor::is_modified) {
            self.panes.prompt = Some((pane, path));
            return;
        }
        self.remove_tab(pane, tab);
    }

    fn resolve_close(&mut self, choice: CloseChoice, jobs: &Jobs) {
        let Some((pane, path)) = self.panes.prompt.take() else { return };
        let Some(tab) = self.panes.sides.get(pane).and_then(|s| s.position(&path)) else { return };
        match choice {
            CloseChoice::Cancel => {}
            CloseChoice::Discard => self.remove_tab(pane, tab),
            CloseChoice::Save => {
                self.panes.sides[pane].tabs[tab].closing = true;
                self.save(path, jobs);
            }
        }
    }

    /// Removes a tab. A pane left without tabs closes if it is one of two;
    /// a file left without tabs closes.
    fn remove_tab(&mut self, pane: usize, tab: usize) {
        self.park();
        let removed = self.panes.sides[pane].remove(tab);
        self.close_empty_pane(pane);
        if !self.panes.sides.iter().any(|s| s.position(&removed.path).is_some()) {
            self.panes.parked.retain(|e| e.path() != removed.path);
            self.inline_diffs.forget(&removed.path);
        }
        self.unpark();
    }

    /// Closes every tab of a file, without asking.
    pub(super) fn close_file(&mut self, path: &Path) {
        while let Some((pane, tab)) =
            self.panes.sides.iter().enumerate().find_map(|(p, side)| side.position(path).map(|t| (p, t)))
        {
            self.remove_tab(pane, tab);
        }
    }

    /// Closes `pane` if it has no tabs and isn't the only one. The other
    /// pane gets the focus.
    fn close_empty_pane(&mut self, pane: usize) {
        if self.panes.sides[pane].tabs.is_empty() && self.panes.sides.len() > 1 {
            self.panes.sides.remove(pane);
            self.panes.focused = 0;
        }
    }

    fn split_right(&mut self) {
        if self.panes.sides.len() != 1 || self.editor.is_none() {
            return;
        }
        self.park();
        let left = &self.panes.sides[0];
        let tab = &left.tabs[left.active];
        let copy = Tab::new(tab.path.clone(), tab.cursor.clone());
        self.panes.sides.push(Pane { tabs: vec![copy], ..Pane::default() });
        self.panes.focused = 1;
        self.unpark();
    }

    fn move_tab(&mut self, pane: usize, tab: usize) {
        if self.panes.sides.get(pane).is_none_or(|side| tab >= side.tabs.len()) {
            return;
        }
        self.park();
        if self.panes.sides.len() == 1 {
            self.panes.sides.push(Pane::default());
        }
        let target = 1 - pane;
        let moved = self.panes.sides[pane].remove(tab);
        let side = &mut self.panes.sides[target];
        side.active = side.position(&moved.path).unwrap_or_else(|| {
            side.tabs.push(moved);
            side.tabs.len() - 1
        });
        self.panes.focused = target;
        self.close_empty_pane(pane);
        self.unpark();
    }

    fn close_split(&mut self) {
        if self.panes.sides.len() < 2 {
            return;
        }
        self.park();
        let focused = &self.panes.sides[self.panes.focused];
        let focused_path = focused.tabs.get(focused.active).map(|t| t.path.clone());
        let right = self.panes.sides.pop().expect("two panes");
        let left = &mut self.panes.sides[0];
        for tab in right.tabs {
            if left.position(&tab.path).is_none() {
                left.tabs.push(tab);
            }
        }
        if let Some(active) = focused_path.and_then(|path| left.position(&path)) {
            left.active = active;
        }
        self.panes.focused = 0;
        self.unpark();
    }

    /// The tabs as the session keeps them (ticket #59): each side's tabs
    /// with their carets and scroll positions, and the focused side.
    pub(super) fn tabs_session(&self) -> (Vec<PaneSession>, usize) {
        let panes = &self.panes;
        let sides = panes.sides.iter().enumerate().map(|(p, side)| PaneSession {
            active: side.active,
            tabs: side
                .tabs
                .iter()
                .enumerate()
                .filter_map(|(t, tab)| {
                    let editor = self.open_editor(&tab.path)?;
                    // The focused tab's editor holds its live cursor.
                    let focused = p == panes.focused && t == side.active && self.editor.is_some();
                    let cursor = if focused { editor.cursor() } else { tab.cursor.clone() };
                    let selections = editor.cursor_positions(&cursor);
                    Some(TabSession { path: tab.path.clone(), selections, scroll_top: cursor.scroll_top() })
                })
                .collect(),
        });
        (sides.filter(|side| !side.tabs.is_empty()).collect(), panes.focused)
    }

    /// Brings back a session's tabs (ticket #59), once their files are read
    /// into `editors`; tabs whose file couldn't be read are left out. Tabs
    /// opened meanwhile (a file named on the command line) stay, after the
    /// restored ones, and keep the focus.
    pub(super) fn restore_tabs(&mut self, panes: Vec<PaneSession>, focused: usize, editors: Vec<Editor>) {
        let keep = self.active_tab().map(|(pane, tab, _)| (pane, self.panes.sides[pane].tabs[tab].path.clone()));
        self.park();
        for editor in editors {
            if self.open_editor(editor.path()).is_none() {
                self.panes.parked.push(editor);
            }
        }
        let mut focus = None;
        for (p, pane) in panes.into_iter().take(2).enumerate() {
            let active_path = pane.tabs.get(pane.active).map(|t| t.path.clone());
            if p == self.panes.sides.len() {
                self.panes.sides.push(Pane::default());
            }
            let Project { panes: Panes { sides, parked, .. }, .. } = self;
            let side = &mut sides[p];
            let restored: Vec<Tab> = pane
                .tabs
                .into_iter()
                .filter(|tab| side.position(&tab.path).is_none())
                .filter_map(|tab| {
                    let editor = parked.iter().find(|e| e.path() == tab.path)?;
                    Some(Tab::new(tab.path, editor.cursor_at(&tab.selections, tab.scroll_top)))
                })
                .collect();
            let had_tabs = !side.tabs.is_empty();
            let count = restored.len();
            side.tabs.splice(0..0, restored);
            if had_tabs {
                side.active += count;
            } else if let Some(active) = active_path.and_then(|path| side.position(&path)) {
                side.active = active;
            }
            if p == focused
                && let Some(tab) = side.tabs.get(side.active)
            {
                focus = Some((p, tab.path.clone()));
            }
        }
        // The tab opened meanwhile keeps the focus; else the session's.
        let focus = keep.or(focus);
        // A side left without tabs (its files are gone) closes.
        self.panes.sides.retain(|side| !side.tabs.is_empty());
        if self.panes.sides.is_empty() {
            self.panes.sides.push(Pane::default());
        }
        self.panes.focused = 0;
        if let Some((pane, path)) = focus {
            let sides = &mut self.panes.sides;
            // Its side, or the one left of it if a side before it closed.
            let found = std::iter::once(pane.min(sides.len() - 1))
                .chain(0..sides.len())
                .find_map(|p| sides[p].position(&path).map(|t| (p, t)));
            if let Some((pane, tab)) = found {
                self.panes.focused = pane;
                sides[pane].active = tab;
            }
        }
        self.unpark();
    }

    fn scroll_pane(&mut self, pane: usize, rows: f64) {
        let viewport_rows = self.viewport_rows;
        if pane == self.panes.focused {
            if let Some(editor) = &mut self.editor {
                editor.scroll_by(rows, viewport_rows);
            }
            return;
        }
        let Project { editor, panes: Panes { sides, parked, .. }, .. } = self;
        let Some(side) = sides.get_mut(pane) else { return };
        let Some(tab) = side.tabs.get_mut(side.active) else { return };
        let Some(editor) = editor.iter_mut().chain(parked.iter_mut()).find(|e| e.path() == tab.path) else {
            return;
        };
        let live = editor.cursor();
        editor.set_cursor(&tab.cursor, viewport_rows);
        editor.scroll_by(rows, viewport_rows);
        tab.cursor = editor.cursor();
        editor.set_cursor(&live, viewport_rows);
    }
}
