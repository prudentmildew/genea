//! The app: the workbench, its windows and the wiring between them.
//!
//! The app lives in a main-thread `thread_local`; Slint callbacks reach it
//! through [`with_app`]. Background work in the core wakes the app through
//! the workbench's notifier, which schedules a `pump` on Slint's event loop.
//!
//! Windows (ticket #58): each open project has its own window, and the
//! welcome window shows while no project is open (the core decides, through
//! `Workbench::welcome`). Closing the last project window brings the welcome
//! back; closing the welcome quits. The app runs with
//! `run_event_loop_until_quit`, so hiding the last window doesn't end it.

use std::{
    cell::RefCell,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How often background work may refresh the windows: a 120 Hz frame.
const FRAME: Duration = Duration::from_millis(8);

use genea_core::{
    CloseChoice, Command, ConflictChoice, FinderMode, LeftColumnView, ProjectId, SearchQuery, ToolchainPickerKind,
    Workbench,
};
use genea_host::RealHost;
use slint::{CloseRequestResponse, ComponentHandle};

use crate::{
    AboutWindow, FinderKind, LeftView, StructuralAction, about, dialogs, links,
    keys::{self, Modifiers},
    pasteboard::Pasteboard,
    welcome::WelcomeController,
    window::{WindowController, WindowKey},
};

pub struct App {
    workbench: Workbench,
    /// When background work last refreshed the windows, and whether a
    /// refresh is already scheduled (see `pump`).
    last_refresh: Option<Instant>,
    refresh_scheduled: bool,
    /// One window per open project.
    windows: Vec<WindowController>,
    next_key: WindowKey,
    welcome: WelcomeController,
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

/// Creates the app and opens `folder` and `file` from the command line, or
/// shows the welcome window without them.
pub fn start(folder: Option<PathBuf>, file: Option<PathBuf>) -> Result<(), slint::PlatformError> {
    let mut workbench = Workbench::new(Arc::new(RealHost::new().with_clipboard(Pasteboard)));
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
    let welcome = WelcomeController::new()?;
    wire_welcome(&welcome);
    APP.with(|cell| {
        *cell.borrow_mut() = Some(App {
            workbench,
            last_refresh: None,
            refresh_scheduled: false,
            windows: Vec::new(),
            next_key: 0,
            welcome,
            about: None,
        })
    });

    with_app(move |app| {
        if let Some(folder) = folder
            && let Some(key) = app.open_project(None, &folder)
            && let Some(file) = file
        {
            app.dispatch(key, Command::OpenFile(file));
        }
        app.sync_welcome();
        // The core waits a little and checks in the background (#64).
        app.workbench.start_update_checks(env!("CARGO_PKG_VERSION"));
    });
    Ok(())
}

impl App {
    /// Applies finished background work and refreshes every window, at
    /// most once a frame: a flood of terminal output wakes the app far
    /// more often than that (ticket #38). Input refreshes its window at
    /// once, through `dispatch`.
    fn pump(&mut self) {
        if !self.workbench.pump() || self.refresh_scheduled {
            return;
        }
        let since = self.last_refresh.map_or(FRAME, |at| at.elapsed());
        if since >= FRAME {
            self.refresh();
            return;
        }
        self.refresh_scheduled = true;
        slint::Timer::single_shot(FRAME - since, || with_app(App::refresh));
    }

    fn refresh(&mut self) {
        self.refresh_scheduled = false;
        self.last_refresh = Some(Instant::now());
        self.sync_all();
    }

    fn sync_all(&mut self) {
        for controller in &mut self.windows {
            controller.sync(&mut self.workbench);
        }
        self.sync_welcome();
    }

    /// Shows the welcome window while no project is open, and hides it
    /// otherwise.
    fn sync_welcome(&mut self) {
        self.welcome.sync(&self.workbench);
    }

    /// The first project window and the workbench: what the benchmark
    /// harness drives (`src/remote.rs`).
    pub fn first_window(&mut self) -> Option<(&mut WindowController, &mut Workbench)> {
        let controller = self.windows.first_mut()?;
        Some((controller, &mut self.workbench))
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

    /// Opens `folder` as a project in a new window and returns the window.
    /// A folder that is already open just has its window brought to the
    /// front. If the folder can't be opened, window `from` (or the welcome)
    /// says why.
    pub fn open_project(&mut self, from: Option<WindowKey>, folder: &Path) -> Option<WindowKey> {
        let project = match self.workbench.open_project(folder) {
            Ok(project) => project,
            Err(error) => {
                match from.and_then(|key| self.controller(key)) {
                    Some(controller) => {
                        controller.notice = Some(error.to_string());
                        let key = controller.key;
                        self.sync(key);
                    }
                    None => {
                        self.welcome.notice = Some(error.to_string());
                        self.sync_welcome();
                    }
                }
                return None;
            }
        };
        self.welcome.notice = None;
        if let Some(existing) = self.windows.iter().find(|c| c.project == project) {
            existing.focus();
            return Some(existing.key);
        }
        let key = match self.new_window(project) {
            Ok(key) => key,
            Err(error) => {
                eprintln!("genea: couldn't create a window: {error}");
                self.workbench.close_project(project);
                return None;
            }
        };
        self.sync_welcome();
        Some(key)
    }

    /// Creates and shows the window for an open project.
    fn new_window(&mut self, project: ProjectId) -> Result<WindowKey, slint::PlatformError> {
        let key = self.next_key;
        self.next_key += 1;
        let controller = WindowController::new(key, project)?;
        wire(&controller);
        controller.show();
        self.windows.push(controller);
        self.sync(key);
        Ok(key)
    }

    fn window_closed(&mut self, key: WindowKey) {
        let Some(index) = self.windows.iter().position(|c| c.key == key) else { return };
        let controller = self.windows.remove(index);
        self.workbench.close_project(controller.project);
        // Drop the component outside the close handler that is running on it.
        slint::Timer::single_shot(Duration::ZERO, move || drop(controller));
        self.sync_welcome();
    }

    fn welcome_closed(&mut self) {
        self.welcome.closed();
        if self.windows.is_empty() {
            let _ = slint::quit_event_loop();
        }
    }

    fn open_recent(&mut self, index: usize) {
        if let Some(root) = self.welcome.recent_root(index) {
            self.open_project(None, &root);
        }
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

    /// The close prompt in window `key` was answered.
    pub fn resolve_close(&mut self, key: WindowKey, choice: CloseChoice) {
        let Some(controller) = self.windows.iter_mut().find(|c| c.key == key) else { return };
        controller.resolve_close(&mut self.workbench, choice);
    }

    /// A command for the focused pane's active tab (menu items), from the
    /// pane and tab index.
    fn dispatch_for_active_tab(&mut self, key: WindowKey, command: impl FnOnce(usize, usize, usize) -> Command) {
        let Some(controller) = self.windows.iter_mut().find(|c| c.key == key) else { return };
        let Some((pane, view)) = controller.focused_pane(&self.workbench) else { return };
        let Some(active) = view.active else { return };
        controller.dispatch(&mut self.workbench, command(pane, active, view.tabs.len()));
    }

    /// A tab menu item (Close Tab, Select Next Tab, …): on the terminal's
    /// tabs while it has the focus (`terminal` gets the active tab and the
    /// count), else on the focused editor pane's.
    fn dispatch_for_focused_tab(
        &mut self,
        key: WindowKey,
        editor: impl FnOnce(usize, usize, usize) -> Command,
        terminal: impl FnOnce(usize, usize) -> Command,
    ) {
        let Some(controller) = self.windows.iter_mut().find(|c| c.key == key) else { return };
        if !controller.terminal_focused() {
            return self.dispatch_for_active_tab(key, editor);
        }
        let Some(view) = self.workbench.project(controller.project) else { return };
        let count = view.terminal.tabs.len();
        if count > 0 {
            controller.dispatch(&mut self.workbench, terminal(view.terminal.active_tab, count));
        }
    }

    /// Opens the update notice's release page in the browser.
    fn open_update(&mut self) {
        if let Some(notice) = self.workbench.update_notice() {
            links::open_url(&notice.url);
        }
    }
}

/// Connects a window's callbacks to the app.
fn wire(controller: &WindowController) {
    let key = controller.key;
    let window = &controller.window;

    window.on_open_folder(move || {
        dialogs::pick_folder(move |folder| {
            with_app(move |app| {
                app.open_project(Some(key), &folder);
            })
        });
    });
    window.on_show_about(|| with_app(App::show_about));
    window.on_notice_action(move || {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            if let Some(command) = controller.notice_action.clone() {
                controller.dispatch(&mut app.workbench, command);
            }
        });
    });
    window.on_open_update(|| with_app(App::open_update));

    // Panes and tabs arrive as Slint ints; they are never negative.
    let index = |i: i32| usize::try_from(i).unwrap_or(0);
    window.on_scrolled(move |pane, delta_y| {
        let rows = -(delta_y / crate::surface::LINE_HEIGHT) as f64;
        with_app(move |app| app.dispatch(key, Command::ScrollPane { pane: index(pane), rows }));
    });
    window.on_pressed(move |pane, x, y, shift, alt| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.press(&mut app.workbench, index(pane), x, y, shift, alt);
        });
    });
    window.on_dragged(move |pane, x, y| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.drag(&mut app.workbench, index(pane), x, y);
        });
    });
    window.on_double_clicked(move |pane, x, y| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.double_click(&mut app.workbench, index(pane), x, y);
        });
    });
    // Tabs and the split (ticket #31).
    window.on_tab_clicked(move |pane, tab| {
        with_app(move |app| app.dispatch(key, Command::SelectTab { pane: index(pane), tab: index(tab) }));
    });
    window.on_tab_closed(move |pane, tab| {
        with_app(move |app| app.dispatch(key, Command::CloseTab { pane: index(pane), tab: index(tab) }));
    });
    window.on_tab_moved(move |pane, tab| {
        with_app(move |app| app.dispatch(key, Command::MoveTabToOtherSide { pane: index(pane), tab: index(tab) }));
    });
    window.on_close_tab(move || {
        with_app(move |app| {
            app.dispatch_for_focused_tab(
                key,
                |pane, tab, _| Command::CloseTab { pane, tab },
                |tab, _| Command::CloseTerminalTab(tab),
            )
        });
    });
    window.on_move_tab(move || {
        with_app(move |app| app.dispatch_for_active_tab(key, |pane, tab, _| Command::MoveTabToOtherSide { pane, tab }));
    });
    window.on_next_tab(move |forward| {
        with_app(move |app| {
            let next = move |tab: usize, count: usize| {
                if forward { (tab + 1) % count } else { (tab + count - 1) % count }
            };
            app.dispatch_for_focused_tab(
                key,
                |pane, tab, count| Command::SelectTab { pane, tab: next(tab, count) },
                |tab, count| Command::SelectTerminalTab(next(tab, count)),
            )
        });
    });
    // Edits apply synchronously, in order: with_app only defers a key if
    // the app is busy, and then to the very next event-loop turn.
    let clone_caret = std::cell::RefCell::new(keys::CloneCaretGesture::default());
    // ⇧⇧ works from the editor and the terminal alike.
    let double_shift = std::rc::Rc::new(RefCell::new(keys::DoubleShift::default()));
    let editor_shift = double_shift.clone();
    window.on_key(move |text, shift, cmd, alt, ctrl| {
        let modifiers = Modifiers { shift, cmd, alt, ctrl };
        let clone = clone_caret.borrow_mut().command_for(&text, modifiers);
        let everywhere = editor_shift.borrow_mut().command_for(&text);
        if let Some(command) = clone.or(everywhere).or_else(|| keys::command_for(&text, modifiers)) {
            with_app(move |app| app.dispatch(key, command));
        }
    });
    // The finder (ticket #33).
    window.on_open_finder(move |kind| {
        let mode = match kind {
            FinderKind::Files => FinderMode::Files,
            FinderKind::RecentFiles => FinderMode::RecentFiles,
            FinderKind::Actions => FinderMode::Actions,
            FinderKind::Everywhere => FinderMode::Everywhere,
            FinderKind::FileSymbols => FinderMode::FileSymbols,
            FinderKind::ProjectSymbols => FinderMode::ProjectSymbols,
        };
        with_app(move |app| app.dispatch(key, Command::OpenFinder(mode)));
    });
    window.on_finder_edited(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::SetFinderQuery(text)));
    });
    window.on_finder_move(move |by| {
        with_app(move |app| app.dispatch(key, Command::MoveFinderSelection(by as isize)));
    });
    window.on_finder_accept(move || with_app(move |app| app.dispatch(key, Command::AcceptFinder)));
    window.on_finder_close(move || with_app(move |app| app.dispatch(key, Command::CloseFinder)));
    window.on_finder_clicked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.click_finder_item(&mut app.workbench, index);
        });
    });
    window.on_committed(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::InsertText(text)));
    });
    window.on_preedit_changed(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::SetPreedit(text)));
    });
    // Menu items that are one command each.
    let menu = move |command: Command| {
        move || {
            let command = command.clone();
            with_app(move |app| app.dispatch(key, command));
        }
    };
    // Editing items act on the editor; with the terminal focused, Paste
    // types into it and the others do nothing.
    let edit = move |command: Command| {
        move || {
            let command = command.clone();
            with_app(move |app| {
                let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
                let command = match (controller.terminal_focused(), command) {
                    (false, command) => command,
                    (true, Command::Paste) => Command::TerminalPaste,
                    (true, _) => return,
                };
                controller.dispatch(&mut app.workbench, command);
            });
        }
    };
    window.on_save(menu(Command::Save));
    window.on_undo(edit(Command::Undo));
    window.on_redo(edit(Command::Redo));
    window.on_cut(edit(Command::Cut));
    window.on_copy(edit(Command::Copy));
    window.on_paste(edit(Command::Paste));
    window.on_select_all(edit(Command::SelectAll));
    // The terminal pane (ticket #38).
    window.on_toggle_terminal(menu(Command::ToggleTerminal));
    window.on_terminal_focus(move || {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            if !controller.terminal_focused() {
                controller.dispatch(&mut app.workbench, Command::FocusTerminal);
            }
        });
    });
    window.on_terminal_key(move |text, shift, cmd, alt, ctrl| {
        if let Some(command) = double_shift.borrow_mut().command_for(&text) {
            with_app(move |app| app.dispatch(key, command));
            return;
        }
        let text = text.to_string();
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.terminal_key(&mut app.workbench, &text, Modifiers { shift, cmd, alt, ctrl });
        });
    });
    window.on_terminal_committed(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::TerminalText(text)));
    });
    window.on_terminal_preedit_changed(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::TerminalPreedit(text)));
    });
    window.on_terminal_mouse(move |kind, button, x, y, shift, alt, ctrl, cmd| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            let modifiers = Modifiers { shift, cmd, alt, ctrl };
            controller.terminal_mouse(&mut app.workbench, kind, button, x, y, modifiers);
        });
    });
    window.on_terminal_scrolled(move |delta_y, x, y| {
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.terminal_scrolled(&mut app.workbench, delta_y, x, y);
        });
    });
    window.on_terminal_size_changed(move || with_app(move |app| app.sync(key)));
    // Terminal tabs (ticket #39).
    window.on_new_terminal_tab(menu(Command::NewTerminalTab));
    window.on_terminal_tab_closed(move |tab| {
        with_app(move |app| app.dispatch(key, Command::CloseTerminalTab(index(tab))));
    });
    window.on_open_config(menu(Command::OpenConfig));
    window.on_toggle_view(move |view| {
        let view = left_column_view(view);
        with_app(move |app| app.dispatch(key, Command::ToggleLeftColumn(view)));
    });
    window.on_show_view(move |view| {
        let view = left_column_view(view);
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.show_view(&mut app.workbench, view);
        });
    });
    window.on_problem_clicked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.open_problem(&mut app.workbench, index);
        });
    });
    window.on_conflict_resolved(move |pane, reload| {
        let Ok(pane) = usize::try_from(pane) else { return };
        let choice = if reload { ConflictChoice::Reload } else { ConflictChoice::KeepMyEdits };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.resolve_conflict(&mut app.workbench, pane, choice);
        });
    });
    window.on_file_clicked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.click_file(&mut app.workbench, index);
        });
    });
    window.on_search(move |text, regex, case_sensitive, whole_word| {
        let query = SearchQuery { text: text.into(), regex, case_sensitive, whole_word };
        with_app(move |app| app.dispatch(key, Command::Search(query)));
    });
    window.on_search_result_clicked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.open_search_result(&mut app.workbench, index);
        });
    });
    window.on_split_right(menu(Command::SplitRight));
    window.on_close_split(menu(Command::CloseSplit));
    window.on_reload_environment(menu(Command::ReloadEnvironment));
    window.on_install_dependencies(menu(Command::InstallDependencies));
    window.on_restart_language_server(menu(Command::RestartLanguageServer));
    window.on_terminal_tab_clicked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.dispatch(&mut app.workbench, Command::SelectTerminalTab(index));
        });
    });
    window.on_structural(move |action| {
        let command = match action {
            StructuralAction::ToggleLineComment => Command::ToggleLineComment,
            StructuralAction::ExpandFold => Command::ExpandFold,
            StructuralAction::CollapseFold => Command::CollapseFold,
            StructuralAction::ExpandAllFolds => Command::ExpandAllFolds,
            StructuralAction::CollapseAllFolds => Command::CollapseAllFolds,
        };
        // Like the editing items, these act on the editor, so with the
        // terminal focused they do nothing.
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            if !controller.terminal_focused() {
                controller.dispatch(&mut app.workbench, command);
            }
        });
    });
    window.on_rollback_hunk(menu(Command::RollbackHunk));
    window.on_hide_hunk(menu(Command::HideHunk));
    window.on_set_runtime(menu(Command::OpenToolchainPicker(ToolchainPickerKind::Runtime)));
    window.on_set_package_manager(menu(Command::OpenToolchainPicker(ToolchainPickerKind::PackageManager)));
    window.on_update_toolchain(menu(Command::OpenToolchainPicker(ToolchainPickerKind::Update)));
    window.on_remove_unused_toolchains(menu(Command::RemoveUnusedToolchains));
    window.on_picker_filter_edited(move |text| {
        let text = text.to_string();
        with_app(move |app| app.dispatch(key, Command::FilterToolchainPicker(text)));
    });
    window.on_picker_picked(move |index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| {
            let Some(controller) = app.windows.iter_mut().find(|c| c.key == key) else { return };
            controller.pick_toolchain(&mut app.workbench, index);
        });
    });
    window.on_picker_cancelled(menu(Command::CloseToolchainPicker));
    window.on_viewport_changed(move || with_app(move |app| app.sync(key)));
    window.window().on_close_requested(move || {
        with_app(move |app| app.window_closed(key));
        CloseRequestResponse::HideWindow
    });
}

/// The core's name for a left-column view.
fn left_column_view(view: LeftView) -> LeftColumnView {
    match view {
        LeftView::Files => LeftColumnView::Files,
        LeftView::Problems => LeftColumnView::Problems,
        LeftView::Search => LeftColumnView::Search,
    }
}

/// Connects the welcome window's callbacks to the app.
fn wire_welcome(welcome: &WelcomeController) {
    let window = &welcome.window;
    window.on_open_folder(|| {
        dialogs::pick_folder(|folder| {
            with_app(move |app| {
                app.open_project(None, &folder);
            })
        });
    });
    window.on_open_recent(|index| {
        let Ok(index) = usize::try_from(index) else { return };
        with_app(move |app| app.open_recent(index));
    });
    window.on_show_about(|| with_app(App::show_about));
    window.on_open_update(|| with_app(App::open_update));
    window.window().on_close_requested(|| {
        with_app(App::welcome_closed);
        CloseRequestResponse::HideWindow
    });
}
