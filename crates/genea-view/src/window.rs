//! One project window: binds a `ProjectWindow` to a project's view state.
//! Each open project has exactly one window (ticket #58). Its editor area
//! has a pane of tabs, or two after a split (ticket #31).
//!
//! `sync` is the only place view state flows into Slint. It reads the
//! core's `ProjectView` and sets properties; Slint skips equal values and the
//! surface diffs its slots, so a sync with nothing new repaints nothing.

use std::{
    path::PathBuf,
    rc::Rc,
    sync::Arc,
    time::{Duration, Instant},
};

use genea_core::{
    CloseChoice, Command, ConflictChoice, FileRow, FileRowKind, LeftColumnView, MAX_SEARCH_MATCHES, PaneView,
    ProblemItem, ProjectId, SearchFile, SearchView, Severity, TextPosition, Theme as ConfigTheme, ToolchainOption,
    Workbench,
};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSView};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use slint::{ComponentHandle, ModelRc, VecModel};

use crate::{
    FileEntry, LeftView, PickerRow, ProblemRow, ProjectWindow, SearchRow, TabEntry, Theme, app::with_app, dialogs, fonts,
    keys::Modifiers, links, surface::Surface,
    terminal::{self, TerminalSurface},
};

/// Identifies a window for the lifetime of the app (callbacks capture it).
pub type WindowKey = u64;

pub struct WindowController {
    pub key: WindowKey,
    pub window: ProjectWindow,
    pub project: ProjectId,
    /// The left pane's surface and the right one's.
    surfaces: [Surface; 2],
    /// The tab strips last pushed to Slint, to skip unchanged ones.
    tabs: [Vec<TabEntry>; 2],
    /// The pane last given the keyboard focus.
    focused_pane: usize,
    /// The terminal pane (ticket #38).
    terminal: TerminalSurface,
    /// The terminal had the keyboard focus at the last sync.
    terminal_focused: bool,
    /// The close prompt's sheet is showing.
    prompting: bool,
    /// A window-level message, e.g. why a folder couldn't be opened.
    pub notice: Option<String>,
    /// The command behind the shown notice's button, if it has one.
    pub notice_action: Option<Command>,
    /// When and on which line the last double-click was, to spot a third.
    last_double_click: Option<(Instant, usize)>,
    /// The Problems items as last pushed, so a click maps to the item the
    /// user saw and an unchanged list isn't pushed again.
    problems: Vec<ProblemItem>,
    problem_rows: Rc<VecModel<ProblemRow>>,
    /// The Files view's rows as last pushed, likewise.
    files: Arc<[FileRow]>,
    file_rows: Rc<VecModel<FileEntry>>,
    /// The Search view's results as last pushed.
    search: SearchResults,
    /// The button went down with ⌥ (adding a caret) or on a fold marker,
    /// so a drag doesn't select.
    option_press: bool,
    /// The toolchain picker is showing.
    picker_open: bool,
    /// The picker's options as last pushed, so a click maps to the option
    /// the user saw and an unchanged list isn't pushed again.
    picker_options: Vec<ToolchainOption>,
    picker_rows: Rc<VecModel<PickerRow>>,
}

/// How far left of the text a press still hits a git gutter marker: its
/// column and the fold-marker column to its right (ui/editor-surface.slint).
/// A press on a line's fold marker toggles the fold instead.
const GIT_MARKER_WIDTH: f32 = 22.0;

/// A press this soon after a double-click on the same line is a triple-click.
const TRIPLE_CLICK_INTERVAL: Duration = Duration::from_millis(500);

impl WindowController {
    pub fn new(key: WindowKey, project: ProjectId) -> Result<Self, slint::PlatformError> {
        let window = ProjectWindow::new()?;
        crate::journal::attach(window.window());
        let surfaces = [Surface::new(&window, 0), Surface::new(&window, 1)];
        let terminal = TerminalSurface::new(&window);
        let problem_rows = Rc::new(VecModel::default());
        window.set_problems(ModelRc::from(problem_rows.clone()));
        let picker_rows = Rc::new(VecModel::default());
        window.set_picker_options(ModelRc::from(picker_rows.clone()));
        let file_rows = Rc::new(VecModel::default());
        window.set_files(ModelRc::from(file_rows.clone()));
        let search = SearchResults::default();
        window.set_search_rows(ModelRc::from(search.rows.clone()));
        Ok(WindowController {
            key,
            window,
            project,
            surfaces,
            tabs: Default::default(),
            focused_pane: 0,
            terminal,
            terminal_focused: false,
            prompting: false,
            notice: None,
            notice_action: None,
            last_double_click: None,
            problems: Vec::new(),
            problem_rows,
            files: Arc::from([]),
            file_rows,
            search,
            option_press: false,
            picker_open: false,
            picker_options: Vec::new(),
            picker_rows,
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
    /// the selection, ⌥ adds a caret, and a press soon after a double-click
    /// (a third click) selects the line.
    pub fn press(&mut self, workbench: &mut Workbench, pane: usize, x: f32, y: f32, shift: bool, alt: bool) {
        self.focus_pane(workbench, pane);
        if let Some(line) = self.surfaces[pane].fold_marker_at(&self.window, x, y) {
            self.option_press = true;
            self.dispatch(workbench, Command::ToggleFold { line });
            return;
        }
        let (line, column) = self.surfaces[pane].cell_at(&self.window, x, y);
        if self.on_git_marker(workbench, pane, x, line) {
            self.dispatch(workbench, Command::ShowHunk { line });
            return;
        }
        let triple = self.last_double_click.take().is_some_and(|(at, clicked_line)| {
            clicked_line == line && at.elapsed() < TRIPLE_CLICK_INTERVAL
        });
        self.option_press = alt && !triple;
        let command = if triple {
            Command::SelectLine { line }
        } else if alt {
            Command::AddCaret { line, column }
        } else if shift {
            Command::ExtendSelection { line, column }
        } else {
            Command::PlaceCaret { line, column }
        };
        self.dispatch(workbench, command);
    }

    /// Whether a press at `x` on `line` hits the line's git gutter marker
    /// (the strip just left of the text).
    fn on_git_marker(&self, workbench: &Workbench, pane: usize, x: f32, line: usize) -> bool {
        let text_left = self.window.get_text_left();
        if !(text_left - GIT_MARKER_WIDTH..text_left).contains(&x) {
            return false;
        }
        let Some(view) = workbench.project(self.project) else { return false };
        let editor = view.panes.get(pane).and_then(|p| p.editor.as_ref());
        editor.is_some_and(|e| e.gutter.iter().any(|m| m.line == line))
    }

    /// A drag with the button down extends the selection.
    pub fn drag(&mut self, workbench: &mut Workbench, pane: usize, x: f32, y: f32) {
        if self.option_press {
            return;
        }
        self.focus_pane(workbench, pane);
        let (line, column) = self.surfaces[pane].cell_at(&self.window, x, y);
        self.dispatch(workbench, Command::ExtendSelection { line, column });
    }

    pub fn double_click(&mut self, workbench: &mut Workbench, pane: usize, x: f32, y: f32) {
        self.focus_pane(workbench, pane);
        let (line, column) = self.surfaces[pane].cell_under(&self.window, x, y);
        self.last_double_click = Some((Instant::now(), line));
        self.dispatch(workbench, Command::SelectWord { line, column });
    }

    /// A press in a pane without the focus focuses it first.
    fn focus_pane(&mut self, workbench: &mut Workbench, pane: usize) {
        if pane != self.focused_pane || self.terminal_focused {
            workbench.dispatch(self.project, Command::FocusPane(pane));
        }
    }

    /// Whether keys go to the terminal.
    pub fn terminal_focused(&self) -> bool {
        self.terminal_focused
    }

    /// A key press in the terminal.
    pub fn terminal_key(&mut self, workbench: &mut Workbench, text: &str, modifiers: Modifiers) {
        if let Some(command) = terminal::command_for(text, modifiers) {
            self.dispatch(workbench, command);
        }
    }

    /// The mouse over the terminal's grid. A ⌘-click opens a link.
    pub fn terminal_mouse(&mut self, workbench: &mut Workbench, kind: i32, button: i32, x: f32, y: f32, modifiers: Modifiers) {
        let (line, column) = self.terminal.cell_at(&self.window, x, y);
        if modifiers.cmd {
            if kind == 0
                && let Some(link) = self.terminal.link_at(line, column)
            {
                links::open_url(&link);
            }
            return;
        }
        if let Some(command) = self.terminal.mouse(kind, button, line, column, modifiers) {
            self.dispatch(workbench, command);
        }
    }

    /// The scroll wheel over the terminal's grid.
    pub fn terminal_scrolled(&mut self, workbench: &mut Workbench, delta_y: f32, x: f32, y: f32) {
        let (line, column) = self.terminal.cell_at(&self.window, x, y);
        if let Some(rows) = self.terminal.scroll(delta_y) {
            self.dispatch(workbench, Command::ScrollTerminal { rows, line, column });
        }
    }

    /// The focused pane and its tabs, for menu items that act on the active
    /// tab.
    pub fn focused_pane(&self, workbench: &Workbench) -> Option<(usize, PaneView)> {
        let view = workbench.project(self.project)?;
        let pane = view.focused_pane;
        Some((pane, view.panes.into_iter().nth(pane)?))
    }

    pub fn dispatch(&mut self, workbench: &mut Workbench, command: Command) {
        workbench.dispatch(self.project, command);
        self.sync(workbench);
    }

    pub fn sync(&mut self, workbench: &mut Workbench) {
        let window = &self.window;
        // Both panes have the same height; the core has one viewport.
        let viewport_change = self.surfaces[0].take_viewport_change(window);
        self.surfaces[1].take_viewport_change(window);
        if let Some(rows) = viewport_change {
            workbench.dispatch(self.project, Command::SetViewport { rows });
        }
        if let Some((rows, columns)) = self.terminal.take_size_change(window) {
            workbench.dispatch(self.project, Command::SetTerminalSize { rows, columns });
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
        window.set_status_caret(view.status.caret.clone().unwrap_or_default().into());
        window.set_status_notice(notice.unwrap_or_default().into());
        window.set_status_config_notice(view.status.config_notice.clone().unwrap_or_default().into());
        window.set_status_problems(problem_counts(view.status.errors, view.status.warnings).into());
        window.set_status_has_errors(view.status.errors > 0);

        let theme = window.global::<Theme>();
        theme.set_follow_system(view.config.theme == ConfigTheme::System);
        theme.set_pinned_dark(view.config.theme == ConfigTheme::Dark);

        window.set_left_column_visible(view.left_column.is_some());
        match view.left_column {
            Some(LeftColumnView::Files) => window.set_left_view(LeftView::Files),
            Some(LeftColumnView::Problems) => window.set_left_view(LeftView::Problems),
            Some(LeftColumnView::Search) => window.set_left_view(LeftView::Search),
            None => {}
        }
        self.search.sync(window, &view.search);
        // The core shares an unchanged tree, so this is a pointer compare.
        if view.files != self.files {
            let rows: Vec<FileEntry> = view
                .files
                .iter()
                .map(|row| FileEntry {
                    name: row.name.as_str().into(),
                    depth: row.depth as i32,
                    folder: matches!(row.kind, FileRowKind::Folder { .. }),
                    expanded: row.kind == FileRowKind::Folder { expanded: true },
                })
                .collect();
            for row in &rows {
                fonts::prepare(&row.name);
            }
            self.file_rows.set_vec(rows);
            self.files = view.files.clone();
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
        window.set_status_encoding(view.status.encoding.clone().unwrap_or_default().into());
        window.set_status_line_ending(view.status.line_ending.clone().unwrap_or_default().into());
        let action = view.notices.last().and_then(|n| n.action.clone());
        window.set_status_notice_action(action.as_ref().map(|a| a.label.clone()).unwrap_or_default().into());
        self.notice_action = action.map(|a| a.command);
        window.set_status_toolchain(view.status.toolchain.clone().unwrap_or_default().into());
        window.set_status_branch(view.status.branch.clone().unwrap_or_default().into());
        window.set_status_large_file(view.status.large_file.clone().unwrap_or_default().into());
        window.set_status_loading(editor.is_some_and(|e| e.loading));

        let picker = view.toolchain_picker.as_ref();
        window.set_picker_visible(picker.is_some());
        if let Some(picker) = picker {
            window.set_picker_title(picker.title.as_str().into());
            window.set_picker_message(picker.message.clone().unwrap_or_default().into());
            window.set_picker_loading(picker.loading);
        }
        let options = picker.map(|p| p.options.clone()).unwrap_or_default();
        if options != self.picker_options {
            let rows: Vec<PickerRow> = options
                .iter()
                .map(|o| PickerRow { label: format!("{} {}", o.tool, o.version).into(), detail: o.detail.as_str().into() })
                .collect();
            self.picker_rows.set_vec(rows);
            self.picker_options = options;
        }
        // The editor gets the keyboard back once the picker closes.
        if self.picker_open && picker.is_none() {
            window.invoke_refocus();
        }
        self.picker_open = picker.is_some();

        window.set_split(view.panes.len() > 1);
        window.set_can_split(view.can_split);
        window.set_focused_pane(view.focused_pane as i32);
        for pane in 0..2 {
            let shown = view.panes.get(pane);
            let tabs: Vec<TabEntry> = shown.map_or_else(Vec::new, |p| {
                p.tabs
                    .iter()
                    .enumerate()
                    .map(|(i, tab)| TabEntry {
                        title: tab.title.as_str().into(),
                        modified: tab.modified,
                        active: p.active == Some(i),
                    })
                    .collect()
            });
            if tabs != self.tabs[pane] {
                let model = ModelRc::new(VecModel::from(tabs.clone()));
                if pane == 0 { window.set_left_tabs(model) } else { window.set_right_tabs(model) }
                self.tabs[pane] = tabs;
            }
            let editor = shown.and_then(|p| p.editor.as_ref());
            let conflict = editor.is_some_and(|e| e.conflict);
            if pane == 0 {
                window.set_left_has_editor(editor.is_some());
                window.set_left_conflict(conflict);
            } else {
                window.set_right_has_editor(editor.is_some());
                window.set_right_conflict(conflict);
            }
            // Before Slint shapes the text: registers fallback fonts it needs.
            let titles = shown.iter().flat_map(|p| &p.tabs).map(|tab| &tab.title);
            let lines = editor.iter().flat_map(|e| &e.lines).map(|line| &line.text);
            for text in titles.chain(lines) {
                fonts::prepare(text);
            }
            self.surfaces[pane].sync(window, editor);
        }
        self.terminal.sync(window, &view.terminal);
        crate::journal::mark_synced(editor.is_some_and(|e| !e.lines.is_empty()));
        if let Some(editor) = editor.filter(|e| !e.lines.is_empty()) {
            crate::journal::mark_shown(&editor.path);
        }
        if view.focused_pane != self.focused_pane || view.terminal.focused != self.terminal_focused {
            self.focused_pane = view.focused_pane;
            self.terminal_focused = view.terminal.focused;
            window.invoke_refocus();
        }

        if let Some(prompt) = &view.close_prompt
            && !self.prompting
        {
            self.prompting = true;
            let key = self.key;
            dialogs::ask_to_save(&self.window, &prompt.title, move |choice| {
                with_app(move |app| app.resolve_close(key, choice))
            });
        }
    }

    /// The close prompt was answered.
    pub fn resolve_close(&mut self, workbench: &mut Workbench, choice: CloseChoice) {
        self.prompting = false;
        self.dispatch(workbench, Command::ResolveClose(choice));
    }

    /// A pane's conflict bar was answered: for the file that pane shows.
    pub fn resolve_conflict(&mut self, workbench: &mut Workbench, pane: usize, choice: ConflictChoice) {
        let Some(view) = workbench.project(self.project) else { return };
        let Some(editor) = view.panes.get(pane).and_then(|p| p.editor.as_ref()) else { return };
        let command = Command::ResolveConflict { path: editor.path.clone(), choice };
        self.dispatch(workbench, command);
    }

    /// A toolchain picker option was picked: pin it.
    pub fn pick_toolchain(&mut self, workbench: &mut Workbench, index: usize) {
        let Some(option) = self.picker_options.get(index) else { return };
        let command = option.command.clone();
        self.dispatch(workbench, command);
    }

    /// A Problems item was clicked: open its file at the problem.
    pub fn open_problem(&mut self, workbench: &mut Workbench, index: usize) {
        let Some(item) = self.problems.get(index) else { return };
        let command = Command::OpenFileAt { path: item.path.clone(), at: item.position };
        self.dispatch(workbench, command);
    }

    /// A Files view row was clicked: open the file, or expand or collapse
    /// the folder.
    pub fn click_file(&mut self, workbench: &mut Workbench, index: usize) {
        let Some(row) = self.files.get(index) else { return };
        let command = match row.kind {
            FileRowKind::File => Command::OpenFile(row.path.clone()),
            FileRowKind::Folder { .. } => Command::ToggleFolder(row.path.clone()),
        };
        self.dispatch(workbench, command);
    }

    /// A Search view row was clicked: open the file, at the match for a
    /// match row.
    pub fn open_search_result(&mut self, workbench: &mut Workbench, index: usize) {
        let Some((path, at)) = self.search.targets.get(index) else { return };
        let command = match at {
            Some(at) => Command::OpenFileAt { path: path.clone(), at: *at },
            None => Command::OpenFile(path.clone()),
        };
        self.dispatch(workbench, command);
    }

}

/// The Search view's rows as last pushed, so a click maps to the result the
/// user saw and unchanged results aren't pushed again.
#[derive(Default)]
struct SearchResults {
    files: Arc<[SearchFile]>,
    rows: Rc<VecModel<SearchRow>>,
    /// What each row opens: a file, at a match for a match row.
    targets: Vec<(PathBuf, Option<TextPosition>)>,
}

impl SearchResults {
    /// Pushes the Search view's query, status line and results.
    fn sync(&mut self, window: &ProjectWindow, search: &SearchView) {
        window.set_search_query(search.query.text.as_str().into());
        window.set_search_regex(search.query.regex);
        window.set_search_case_sensitive(search.query.case_sensitive);
        window.set_search_whole_word(search.query.whole_word);
        window.set_search_status(search_status(search).into());
        window.set_search_status_error(search.error.is_some());
        // The core shares unchanged results, so this is a pointer compare.
        if search.files == self.files {
            return;
        }
        let mut rows = Vec::new();
        let mut targets = Vec::new();
        for file in search.files.iter() {
            let path = file.path.display().to_string();
            fonts::prepare(&path);
            rows.push(SearchRow { file: true, label: path.into(), ..SearchRow::default() });
            targets.push((file.path.clone(), None));
            for m in &file.matches {
                for text in [&m.before, &m.matched, &m.after] {
                    fonts::prepare(text);
                }
                rows.push(SearchRow {
                    file: false,
                    label: m.location.as_str().into(),
                    before: m.before.as_str().into(),
                    matched: m.matched.as_str().into(),
                    after: m.after.as_str().into(),
                });
                targets.push((file.path.clone(), Some(m.position)));
            }
        }
        self.rows.set_vec(rows);
        self.targets = targets;
        self.files = search.files.clone();
    }
}

impl WindowController {
    /// Shows a left-column view, leaving it showing if it already is.
    pub fn show_view(&mut self, workbench: &mut Workbench, view: LeftColumnView) {
        if workbench.project(self.project).is_some_and(|p| p.left_column != Some(view)) {
            self.dispatch(workbench, Command::ToggleLeftColumn(view));
        }
    }
}

/// The Search view's status line: why the query can't run, or how many
/// matches there are so far.
fn search_status(search: &SearchView) -> String {
    if let Some(error) = &search.error {
        return error.lines().last().unwrap_or(error).trim().to_owned();
    }
    if search.query.text.is_empty() {
        return String::new();
    }
    let files = search.files.len();
    let counts = match (search.match_count, files) {
        (0, _) => "No matches".to_owned(),
        (1, _) => "1 match in 1 file".to_owned(),
        (n, 1) => format!("{n} matches in 1 file"),
        (n, f) => format!("{n} matches in {f} files"),
    };
    if search.searching {
        if search.match_count == 0 { "Searching…".to_owned() } else { format!("{counts}, searching…") }
    } else if search.limited {
        format!("{counts} (stopped at {MAX_SEARCH_MATCHES})")
    } else {
        counts
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
