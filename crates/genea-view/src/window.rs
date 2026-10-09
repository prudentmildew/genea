//! One project window: binds a `ProjectWindow` to a project's view state.
//!
//! `sync` is the only place view state flows into Slint. It reads the
//! core's `ProjectView` and sets properties; Slint skips equal values and the
//! surface diffs its slots, so a sync with nothing new repaints nothing.

use genea_core::{Command, ProjectId, Workbench};
use slint::ComponentHandle;

use crate::{ProjectWindow, surface::Surface};

/// Identifies a window for the lifetime of the app (callbacks capture it).
pub type WindowKey = u64;

pub struct WindowController {
    pub key: WindowKey,
    pub window: ProjectWindow,
    pub project: Option<ProjectId>,
    surface: Surface,
    /// A window-level message, e.g. why a folder couldn't be opened.
    pub notice: Option<String>,
}

impl WindowController {
    pub fn new(key: WindowKey) -> Result<Self, slint::PlatformError> {
        let window = ProjectWindow::new()?;
        let surface = Surface::new(&window);
        Ok(WindowController { key, window, project: None, surface, notice: None })
    }

    pub fn show(&self) {
        if let Err(error) = self.window.show() {
            eprintln!("genea: couldn't show a window: {error}");
        }
    }

    /// Converts a press on the surface into a caret placement.
    pub fn press(&mut self, workbench: &mut Workbench, x: f32, y: f32) {
        let (line, column) = self.surface.cell_at(&self.window, x, y);
        self.dispatch(workbench, Command::PlaceCaret { line, column });
    }

    pub fn dispatch(&mut self, workbench: &mut Workbench, command: Command) {
        if let Some(project) = self.project {
            workbench.dispatch(project, command);
            self.sync(workbench);
        }
    }

    pub fn sync(&mut self, workbench: &mut Workbench) {
        let window = &self.window;
        let project = self.project.and_then(|id| {
            if let Some(rows) = self.surface.take_viewport_change(window) {
                workbench.dispatch(id, Command::SetViewport { rows });
            }
            workbench.project(id)
        });

        let Some(view) = project else {
            window.set_window_title("Genea".into());
            window.set_has_project(false);
            window.set_has_editor(false);
            window.set_status_caret("".into());
            window.set_status_notice(self.notice.clone().unwrap_or_default().into());
            self.surface.sync(window, None);
            return;
        };

        let editor = view.editor.as_ref();
        let title = match editor {
            Some(editor) => format!("{} – {}", view.name, editor.path.display()),
            None => view.name.clone(),
        };
        let notice = view.notices.last().map(|n| n.message.clone()).or_else(|| self.notice.clone());
        window.set_window_title(title.into());
        window.set_has_project(true);
        window.set_has_editor(editor.is_some());
        window.set_tab_title(editor.map(|e| e.title.clone()).unwrap_or_default().into());
        window.set_status_caret(view.status.caret.clone().unwrap_or_default().into());
        window.set_status_notice(notice.unwrap_or_default().into());
        self.surface.sync(window, editor);
    }
}
