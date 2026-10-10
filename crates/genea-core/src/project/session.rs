//! A project's side of session state (ticket #59): what it saves, and
//! bringing it back when the project opens.

use std::{
    hash::{DefaultHasher, Hash, Hasher},
    path::PathBuf,
};

use genea_host::Clock;

use super::Project;
use crate::{
    editor::Editor,
    jobs::Jobs,
    reading::{self, Contents},
    session::{PaneSession, ProjectSession},
    view::{DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE, WindowLayout},
};

impl Project {
    /// Brings back the project's saved session, if it has one: the left
    /// column, the terminal's tabs, the zoom and the window's layout at
    /// once, and the editor tabs once their files are read in the
    /// background. Until they are, nothing is saved, so an early save can't
    /// overwrite the session with an empty one.
    pub(crate) fn restore_session(&mut self, jobs: &Jobs) {
        let Some(session) = self.session_file.read() else {
            self.session_restored = true;
            return;
        };
        self.left_column = session.left_column;
        self.zoom = session.zoom;
        self.window_layout = session.layout;
        self.terminal.restore(&session.terminal);
        if session.panes.is_empty() {
            self.session_restored = true;
            return;
        }
        let (id, root) = (self.id, self.root.clone());
        let ProjectSession { panes, focused_pane, .. } = session;
        let rows = self.viewport_rows.ceil() as usize;
        jobs.spawn("restore tabs", move || {
            let mut paths: Vec<PathBuf> = panes.iter().flat_map(|p| &p.tabs).map(|t| t.path.clone()).collect();
            paths.sort();
            paths.dedup();
            // Files that are gone or no longer text are left out.
            let editors: Vec<Editor> = paths
                .into_iter()
                .filter_map(|path| match reading::read(&root.join(&path), rows, |_| {}) {
                    Ok(Contents::Text(text)) => Some(Editor::new(path, text)),
                    Ok(Contents::Invalid(text)) => Some(Editor::new(path, text).read_only()),
                    Ok(Contents::Binary) | Err(_) => None,
                })
                .collect();
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                project.restored_tabs(panes, focused_pane, editors, &jobs);
            })
        });
    }

    fn restored_tabs(&mut self, panes: Vec<PaneSession>, focused: usize, editors: Vec<Editor>, jobs: &Jobs) {
        let paths: Vec<PathBuf> = editors.iter().map(|e| e.path().to_owned()).collect();
        self.restore_tabs(panes, focused, editors);
        for path in &paths {
            self.reparse_file(path, jobs);
            self.git.load_base(path, jobs);
        }
        if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
            self.opened_file(&path);
        }
        self.session_restored = true;
    }

    /// What the session keeps now; `None` while it is being restored.
    fn session(&self) -> Option<ProjectSession> {
        if !self.session_restored {
            return None;
        }
        let (panes, focused_pane) = self.tabs_session();
        Some(ProjectSession {
            panes,
            focused_pane,
            left_column: self.left_column,
            terminal: self.terminal.session(),
            zoom: self.zoom,
            layout: self.window_layout,
        })
    }

    /// A cheap fingerprint of what the session keeps (no allocation, no
    /// line lookups): the session is built only when it moves.
    fn session_stamp(&self) -> u64 {
        let mut state = DefaultHasher::new();
        self.hash_tabs_session(&mut state);
        self.terminal.hash_session(&mut state);
        self.left_column.hash(&mut state);
        self.zoom.hash(&mut state);
        if let Some(layout) = self.window_layout {
            let WindowLayout { frame, left_column_width, terminal_width, terminal_height } = layout;
            for length in [frame.x, frame.y, frame.width, frame.height, left_column_width, terminal_width, terminal_height] {
                length.to_bits().hash(&mut state);
            }
        }
        state.finish()
    }

    /// Saves the session in the background a short pause from now, if it
    /// changed. Called after every command and background result, so it
    /// only looks at a fingerprint unless something the session keeps
    /// changed.
    pub(crate) fn save_session_later(&mut self, jobs: &Jobs, clock: &dyn Clock) {
        if !self.session_restored {
            return;
        }
        let stamp = self.session_stamp();
        if self.session_stamp == Some(stamp) {
            return;
        }
        self.session_stamp = Some(stamp);
        if let Some(session) = self.session() {
            self.session_file.changed(session, jobs, clock);
        }
    }

    /// Saves the session in the background now: the project is closing.
    pub(crate) fn save_session_soon(&mut self, jobs: &Jobs) {
        if let Some(session) = self.session() {
            self.session_file.write_soon(session, jobs);
        }
    }

    /// Saves the session on this thread: Genea is quitting.
    pub(crate) fn save_session_now(&mut self) {
        if let Some(session) = self.session() {
            self.session_file.write_now(session);
        }
    }

    /// ⌘+, ⌘− and ⌘0.
    pub(super) fn zoom_by(&mut self, steps: Option<i32>) {
        let (min, max) = ((MIN_FONT_SIZE - DEFAULT_FONT_SIZE) as i32, (MAX_FONT_SIZE - DEFAULT_FONT_SIZE) as i32);
        self.zoom = match steps {
            Some(steps) => (self.zoom + steps).clamp(min, max),
            None => 0,
        };
    }

    /// The editor's and the terminal's font size.
    pub(super) fn font_size(&self) -> f64 {
        (DEFAULT_FONT_SIZE + f64::from(self.zoom)).clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
    }
}
