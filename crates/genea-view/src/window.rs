//! One project window: binds a `ProjectWindow` to a project's view state.
//! Each open project has exactly one window (ticket #58).
//!
//! `sync` is the only place view state flows into Slint. It reads the
//! core's `ProjectView` and sets properties; Slint skips equal values and the
//! surface diffs its slots, so a sync with nothing new repaints nothing.

use std::{
    rc::Rc,
    time::{Duration, Instant},
};

use genea_core::{Command, LeftColumnView, ProblemItem, ProjectId, Severity, Theme as ConfigTheme, Workbench};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSView};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{LeftView, ProblemRow, ProjectWindow, Theme, surface::Surface};

/// Identifies a window for the lifetime of the app (callbacks capture it).
pub type WindowKey = u64;

pub struct WindowController {
    pub key: WindowKey,
    pub window: ProjectWindow,
    pub project: ProjectId,
    surface: Surface,
    /// A window-level message, e.g. why a folder couldn't be opened.
    pub notice: Option<String>,
    /// When and on which line the last double-click was, to spot a third.
    last_double_click: Option<(Instant, usize)>,
    /// The Problems items as last pushed, so a click maps to the item the
    /// user saw and an unchanged list isn't pushed again.
    problems: Vec<ProblemItem>,
    problem_rows: Rc<VecModel<ProblemRow>>,
}

/// A press this soon after a double-click on the same line is a triple-click.
const TRIPLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

impl WindowController {
    pub fn new(key: WindowKey, project: ProjectId) -> Result<Self, slint::PlatformError> {
        let window = ProjectWindow::new()?;
        let surface = Surface::new(&window);
        let problem_rows = Rc::new(VecModel::default());
        window.set_problems(ModelRc::from(problem_rows.clone()));
        Ok(WindowController {
            key,
            window,
            project,
            surface,
            notice: None,
            last_double_click: None,
            problems: Vec::new(),
            problem_rows,
        })
    }

    pub fn show(&self) {
        if let Err(error) = self.window.show() {
            eprintln!("genea: couldn't show a window: {error}");
            return;
        }
        // The native window exists only once the event loop has run, and
        // Slint drops an IME request made before then (ADR 0004).
        let window = self.window.as_weak();
        slint::Timer::single_shot(Duration::ZERO, move || {
            if let Some(window) = window.upgrade() {
                window.invoke_refocus();
            }
        });
    }

    /// Brings the window to the front and makes it key, un-minimising it.
    /// Slint's `show` does nothing for a window that is already shown.
    pub fn focus(&self) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        let handle = self.window.window().window_handle();
        let Ok(handle) = handle.window_handle() else { return };
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else { return };
        // SAFETY: an AppKit window handle's `ns_view` is a live NSView for as
        // long as the window exists, and we are on the main thread.
        let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
        if let Some(ns_window) = view.window() {
            if ns_window.isMiniaturized() {
                ns_window.deminiaturize(None);
            }
            ns_window.makeKeyAndOrderFront(None);
        }
        NSApplication::sharedApplication(mtm).activate();
    }

    /// Converts a press on the surface into a caret placement: ⇧ extends
    /// the selection, and a press soon after a double-click (a third click)
    /// selects the line.
    pub fn press(&mut self, workbench: &mut Workbench, x: f32, y: f32, shift: bool) {
        let (line, column) = self.surface.cell_at(&self.window, x, y);
        let triple = self.last_double_click.take().is_some_and(|(at, clicked_line)| {
            clicked_line == line && at.elapsed() < TRIPLE_CLICK_INTERVAL
        });
        let command = if triple {
            Command::SelectLine { line }
        } else if shift {
            Command::ExtendSelection { line, column }
        } else {
            Command::PlaceCaret { line, column }
        };
        self.dispatch(workbench, command);
    }

    /// A drag with the button down extends the selection.
    pub fn drag(&mut self, workbench: &mut Workbench, x: f32, y: f32) {
        let (line, column) = self.surface.cell_at(&self.window, x, y);
        self.dispatch(workbench, Command::ExtendSelection { line, column });
    }

    pub fn double_click(&mut self, workbench: &mut Workbench, x: f32, y: f32) {
        let (line, column) = self.surface.cell_under(&self.window, x, y);
        self.last_double_click = Some((Instant::now(), line));
        self.dispatch(workbench, Command::SelectWord { line, column });
    }

    pub fn dispatch(&mut self, workbench: &mut Workbench, command: Command) {
        workbench.dispatch(self.project, command);
        self.sync(workbench);
    }

    pub fn sync(&mut self, workbench: &mut Workbench) {
        let window = &self.window;
        if let Some(rows) = self.surface.take_viewport_change(window) {
            workbench.dispatch(self.project, Command::SetViewport { rows });
        }
        // The project closes with its window, so this is always there.
        let Some(view) = workbench.project(self.project) else { return };

        let editor = view.editor.as_ref();
        let title = match editor {
            Some(editor) if editor.modified => format!("{} – {} — Edited", view.name, editor.path.display()),
            Some(editor) => format!("{} – {}", view.name, editor.path.display()),
            None => view.name.clone(),
        };
        let notice = view.notices.last().map(|n| n.message.clone()).or_else(|| self.notice.clone());
        window.set_window_title(title.into());
        window.set_has_editor(editor.is_some());
        window.set_tab_title(editor.map(|e| e.title.clone()).unwrap_or_default().into());
        window.set_tab_modified(editor.is_some_and(|e| e.modified));
        window.set_status_caret(view.status.caret.clone().unwrap_or_default().into());
        window.set_status_notice(notice.unwrap_or_default().into());
        window.set_status_config_notice(view.status.config_notice.clone().unwrap_or_default().into());
        window.set_status_problems(problem_counts(view.status.errors, view.status.warnings).into());
        window.set_status_has_errors(view.status.errors > 0);

        let theme = window.global::<Theme>();
        theme.set_follow_system(view.config.theme == ConfigTheme::System);
        theme.set_pinned_dark(view.config.theme == ConfigTheme::Dark);

        window.set_left_column_visible(view.left_column.is_some());
        if let Some(LeftColumnView::Problems) = view.left_column {
            window.set_left_view(LeftView::Problems);
        }
        if view.problems != self.problems {
            let rows: Vec<ProblemRow> = view
                .problems
                .iter()
                .map(|p| ProblemRow {
                    error: p.severity == Severity::Error,
                    message: p.message.as_str().into(),
                    location: format!("{}:{}", p.path.display(), p.location).into(),
                })
                .collect();
            self.problem_rows.set_vec(rows);
            self.problems = view.problems.clone();
        }
        self.surface.sync(window, editor);
    }

    /// A Problems item was clicked: open its file at the problem.
    pub fn open_problem(&mut self, workbench: &mut Workbench, index: usize) {
        let Some(item) = self.problems.get(index) else { return };
        let command = Command::OpenFileAt { path: item.path.clone(), at: item.position };
        self.dispatch(workbench, command);
    }

    /// Shows a left-column view, leaving it showing if it already is.
    pub fn show_view(&mut self, workbench: &mut Workbench, view: LeftColumnView) {
        if workbench.project(self.project).is_some_and(|p| p.left_column != Some(view)) {
            self.dispatch(workbench, Command::ToggleLeftColumn(view));
        }
    }
}

/// The status bar's problem counts, e.g. "1 error  2 warnings"; empty with
/// none.
fn problem_counts(errors: usize, warnings: usize) -> String {
    let count = |n: usize, what: &str| match n {
        0 => None,
        1 => Some(format!("1 {what}")),
        n => Some(format!("{n} {what}s")),
    };
    [count(errors, "error"), count(warnings, "warning")].into_iter().flatten().collect::<Vec<_>>().join("  ")
}
