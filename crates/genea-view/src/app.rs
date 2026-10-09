//! The app: the workbench, its windows and the wiring between them.
//!
//! The app lives in a main-thread `thread_local`; Slint callbacks reach it
//! through [`with_app`]. Background work in the core wakes the app through
//! the workbench's notifier, which schedules a `pump` on Slint's event loop.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use genea_core::{Command, UpdateCheck, Workbench};
use genea_host::RealHost;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::{
    AboutWindow, about, dialogs, links,
    keys::{self, Modifiers},
    window::{WindowController, WindowKey},
};

pub struct App {
    workbench: Workbench,
    windows: Vec<WindowController>,
    next_key: WindowKey,
    about: Option<AboutWindow>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// Runs `f` on the app. If the app is busy (a Slint callback fired while we
/// were already inside it, e.g. a `changed` handler during a sync), `f` runs
/// on the next event-loop turn instead.
pub fn with_app(f: impl FnOnce(&mut App) + 'static) {
    APP.with(|cell| match cell.try_borrow_mut() {
        Ok(mut app) => {
            if let Some(app) = app.as_mut() {
                f(app);
            }
        }
        Err(_) => slint::Timer::single_shot(Duration::ZERO, move || with_app(f)),
    });
}

/// Like [`with_app`] for callbacks that must answer now; `None` if busy.
fn try_with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| cell.try_borrow_mut().ok().and_then(|mut app| app.as_mut().map(f)))
}

/// Creates the app with one empty window, then opens `folder` and `file`
/// from the command line, if given.
pub fn start(folder: Option<PathBuf>, file: Option<PathBuf>) -> Result<(), slint::PlatformError> {
    let mut workbench = Workbench::new(RealHost::shared());
    let scheduled = Arc::new(AtomicBool::new(false));
    workbench.set_notifier(move || {
        // Coalesce: one pump per event-loop turn is enough.
        if !scheduled.swap(true, Ordering::AcqRel) {
            let scheduled = scheduled.clone();
            let _ = slint::invoke_from_event_loop(move || {
                scheduled.store(false, Ordering::Release);
                with_app(App::pump);
            });
        }
    });
    APP.with(|cell| *cell.borrow_mut() = Some(App { workbench, windows: Vec::new(), next_key: 0, about: None }));

    with_app(move |app| {
        let key = match app.new_window() {
            Ok(key) => key,
            Err(error) => {
                eprintln!("genea: couldn't create a window: {error}");
                return;
            }
        };
        if let Some(folder) = folder {
            app.open_project(key, &folder);
            if let Some(file) = file {
                app.dispatch(key, Command::OpenFile(file));
            }
        }
        // The core waits a little and checks in the background (#64).
        if let Some(state_file) = application_support().map(|dir| dir.join("update-check.json")) {
            let current_version = env!("CARGO_PKG_VERSION").into();
            app.workbench.start_update_checks(UpdateCheck { current_version, state_file });
        }
    });
    Ok(())
}

/// Genea's folder in `~/Library/Application Support`.
fn application_support() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join("Library/Application Support/Genea"))
}

impl App {
    /// Applies finished background work and refreshes every window.
    fn pump(&mut self) {
        if self.workbench.pump() {
            self.sync_all();
        }
    }

    fn sync_all(&mut self) {
        for controller in &mut self.windows {
            controller.sync(&mut self.workbench);
        }
    }

    fn controller(&mut self, key: WindowKey) -> Option<&mut WindowController> {
        self.windows.iter_mut().find(|c| c.key == key)
    }

    fn dispatch(&mut self, key: WindowKey, command: Command) {
        let Some(controller) = self.windows.iter_mut().find(|c| c.key == key) else { return };
        controller.dispatch(&mut self.workbench, command);
    }

    fn sync(&mut self, key: WindowKey) {
        let Some(controller) = self.windows.iter_mut().find(|c| c.key == key) else { return };
        controller.sync(&mut self.workbench);
    }

    /// Opens `folder` as a project: in window `key` if it has none, else in
    /// a new window. A folder that is already open is just brought forward.
    fn open_project(&mut self, key: WindowKey, folder: &Path) {
        let project = match self.workbench.open_project(folder) {
            Ok(project) => project,
            Err(error) => {
                if let Some(controller) = self.controller(key) {
                    controller.notice = Some(error.to_string());
                }
                self.sync(key);
                return;
            }
        };
        if let Some(existing) = self.windows.iter().find(|c| c.project == Some(project)) {
            existing.show();
            return;
        }
        let target = match self.controller(key) {
            Some(controller) if controller.project.is_none() => key,
            _ => match self.new_window() {
                Ok(new_key) => new_key,
                Err(error) => {
                    eprintln!("genea: couldn't create a window: {error}");
                    return;
                }
            },
        };
        if let Some(controller) = self.controller(target) {
            controller.project = Some(project);
            controller.notice = None;
        }
        self.sync(target);
    }

    fn new_window(&mut self) -> Result<WindowKey, slint::PlatformError> {
        let key = self.next_key;
        self.next_key += 1;
        let controller = WindowController::new(key)?;
        wire(&controller);
        controller.show();
        self.windows.push(controller);
        self.sync(key);
        Ok(key)
    }

    fn window_closed(&mut self, key: WindowKey) {
        let Some(index) = self.windows.iter().position(|c| c.key == key) else { return };
        let controller = self.windows.remove(index);
        if let Some(project) = controller.project {
            self.workbench.close_project(project);
        }
        // Drop the component outside the close handler that is running on it.
        slint::Timer::single_shot(Duration::ZERO, move || drop(controller));
    }

    fn show_about(&mut self) {
        if self.about.is_none() {
            match AboutWindow::new() {
                Ok(about) => {
                    about.set_version(env!("CARGO_PKG_VERSION").into());
                    about.set_licences(about::licence_lines());
                    self.about = Some(about);
                }
                Err(error) => {
                    eprintln!("genea: couldn't create the About window: {error}");
                    return;
                }
            }
        }
        if let Some(about) = &self.about
            && let Err(error) = about.show()
        {
            eprintln!("genea: couldn't show the About window: {error}");
        }
    }

    /// Opens the update notice's release page in the browser.
    fn open_update(&mut self) {
        if let Some(notice) = self.workbench.update_notice() {
            links::open_url(&notice.url);
        }
    }

    fn pick_file(&mut self, key: WindowKey) {
        let Some(project) = self.controller(key).and_then(|c| c.project) else { return };
        let Some(root) = self.workbench.project(project).map(|view| view.root) else { return };
        dialogs::pick_file(&root, move |file| with_app(move |app| app.dispatch(key, Command::OpenFile(file))));
    }
}

/// Connects a window's callbacks to the app.
fn wire(controller: &WindowController) {
    let key = controller.key;
    let window = &controller.window;

    window.on_open_folder(move || {
        dialogs::pick_folder(move |folder| with_app(move |app| app.open_project(key, &folder)));
    });
    window.on_open_file(move || with_app(move |app| app.pick_file(key)));
    window.on_show_about(|| with_app(App::show_about));
    window.on_open_update(|| with_app(App::open_update));

    window.on_scrolled(move |delta_y| {
        let rows = -(delta_y / crate::surface::LINE_HEIGHT) as f64;
        with_app(move |app| app.dispatch(key, Command::ScrollBy { rows }));
    });
    window.on_pressed(move |x, y| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.press(&mut app.workbench, x, y);
        });
    });
    window.on_key(move |text, shift, cmd, alt, ctrl| {
        let Some(command) = keys::command_for(&text, Modifiers { shift, cmd, alt, ctrl }) else { return false };
        try_with_app(|app| app.dispatch(key, command)).is_some()
    });
    window.on_viewport_changed(move || with_app(move |app| app.sync(key)));
    window.window().on_close_requested(move || {
        with_app(move |app| app.window_closed(key));
        CloseRequestResponse::HideWindow
    });
}
