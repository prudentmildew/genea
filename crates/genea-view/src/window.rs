//! One project window: binds a `ProjectWindow` to a project's view state.
//! Each open project has exactly one window (ticket #58).
//!
//! `sync` is the only place view state flows into Slint. It reads the
//! core's `ProjectView` and sets properties; Slint skips equal values and the
//! surface diffs its slots, so a sync with nothing new repaints nothing.

use std::time::{Duration, Instant};

use genea_core::{Command, ProjectId, Workbench};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSView};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::ComponentHandle;

use crate::{ProjectWindow, fonts, surface::Surface};

/// Identifies a window for the lifetime of the app (callbacks capture it).
pub type WindowKey = u64;

pub struct WindowController {
    pub key: WindowKey,
    pub window: ProjectWindow,
    pub project: ProjectId,
    surface: Surface,
    /// A window-level message, e.g. why a folder couldn't be opened.
    pub notice: Option<String>,
    /// The command behind the shown notice's button, if it has one.
    pub notice_action: Option<Command>,
    /// When and on which line the last double-click was, to spot a third.
    last_double_click: Option<(Instant, usize)>,
}

/// A press this soon after a double-click on the same line is a triple-click.
const TRIPLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

impl WindowController {
    pub fn new(key: WindowKey, project: ProjectId) -> Result<Self, slint::PlatformError> {
        let window = ProjectWindow::new()?;
        let surface = Surface::new(&window);
        Ok(WindowController {
            key,
            window,
            project,
            surface,
            notice: None,
            notice_action: None,
            last_double_click: None,
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
        let update = workbench.update_notice().map(|notice| notice.message).unwrap_or_default();
        window.set_status_update(update.into());
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
        window.set_status_encoding(view.status.encoding.clone().unwrap_or_default().into());
        window.set_status_line_ending(view.status.line_ending.clone().unwrap_or_default().into());
        for line in editor.iter().flat_map(|e| &e.lines) {
            fonts::prepare(&line.text);
        }
        let action = view.notices.last().and_then(|n| n.action.clone());
        window.set_status_notice_action(action.as_ref().map(|a| a.label.clone()).unwrap_or_default().into());
        self.notice_action = action.map(|a| a.command);
        window.set_status_toolchain(view.status.toolchain.clone().unwrap_or_default().into());
        self.surface.sync(window, editor);
    }
}
