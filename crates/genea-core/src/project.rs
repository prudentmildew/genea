//! One open project: its folder and what its window shows.

mod tabs;

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use genea_host::{Host, SharedHost};
use ropey::Rope;

use crate::{
    command::Command,
    config::{self, CONFIG_FILE, Config},
    editor::Editor,
    environment::{Environment, ProcessEnv},
    files::FileIndex,
    history::EditKind,
    jobs::Jobs,
    problems::{Problem, ProblemSource, Problems, Severity, TextPosition},
    syntax::ParseJob,
    text::Decoded,
    toolchain::{Toolchain, ToolchainContext},
    view::{InlineProblem, LeftColumnView, Notice, ProjectView, StatusBar},
    watcher::{FileChanges, Watcher},
    workbench::ProjectId,
};
use tabs::Panes;

/// Rows assumed until the view reports its viewport.
const DEFAULT_VIEWPORT_ROWS: f64 = 50.0;

pub(crate) struct Project {
    id: ProjectId,
    root: PathBuf,
    /// The focused tab's file (ticket #31: the other open files wait in
    /// `panes`, and come back here when their tab is focused).
    editor: Option<Editor>,
    /// Tabs, the split, and the open files that aren't focused.
    panes: Panes,
    viewport_rows: f64,
    notices: Vec<Notice>,
    /// Bumped by every OpenFile, so a slow read can't replace a newer one.
    open_generation: u64,
    /// Watches the project folder; `None` until started, or if it failed.
    watcher: Option<Watcher>,
    /// The effective config, and the problems of the root `genea.jsonc`.
    config: Config,
    config_problems: Vec<Problem>,
    /// Bumped by every config read, so a slow read can't replace a newer one.
    config_generation: u64,
    /// `genea.jsonc` files below the root (relative paths), each ignored
    /// with a warning.
    nested_configs: BTreeSet<PathBuf>,
    /// Every source's errors and warnings: the Problems view.
    problems: Problems,
    /// What the left column shows; `None` while it is collapsed.
    left_column: Option<LeftColumnView>,
    /// The project's files, for the Files view (ticket #30).
    pub(crate) files: FileIndex,
    /// The runtime and package manager (ticket #35); set by `start_toolchain`.
    pub(crate) toolchain: Option<Toolchain>,
    /// What its processes get (ticket #36); set by `start_environment`.
    pub(crate) environment: Option<Environment>,
}

impl Project {
    pub(crate) fn new(id: ProjectId, root: PathBuf) -> Self {
        Project {
            id,
            files: FileIndex::new(id, root.clone()),
            root,
            editor: None,
            panes: Panes::default(),
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            notices: Vec::new(),
            open_generation: 0,
            toolchain: None,
            environment: None,
            watcher: None,
            config: Config::default(),
            config_problems: Vec::new(),
            config_generation: 0,
            nested_configs: BTreeSet::new(),
            problems: Problems::default(),
            left_column: None,
        }
    }

    /// Starts the project's background work once it is open: the watcher,
    /// and reading the config.
    pub(crate) fn start(&mut self, jobs: &Jobs, host: &dyn Host) {
        match Watcher::start(&self.root, host.support_dir(), self.id, jobs) {
            Ok(watcher) => self.watcher = Some(watcher),
            Err(error) => self.notices.push(Notice {
                message: format!("Genea can't watch this project, so changes on disk won't show: {error}"),
                action: None,
            }),
        }
        self.load_config(jobs);
        self.find_nested_configs(jobs);
        self.files.start(jobs);
    }

    /// Writes a watcher cookie (see `Watcher::sync`). Returns whether one
    /// was written.
    pub(crate) fn sync_watcher(&self) -> bool {
        self.watcher.as_ref().is_some_and(Watcher::sync)
    }

    /// Files changed on disk (from the watcher). Every area that follows
    /// files on disk hooks in here.
    pub(crate) fn files_changed(&mut self, changes: FileChanges, jobs: &Jobs) {
        let root_config = self.root.join(CONFIG_FILE);
        if changes.rescan {
            self.load_config(jobs);
            self.find_nested_configs(jobs);
            return;
        }
        if changes.paths.contains(&root_config) {
            self.load_config(jobs);
        }
        let nested: Vec<PathBuf> = changes
            .paths
            .iter()
            .filter(|path| **path != root_config && path.file_name().is_some_and(|n| n == CONFIG_FILE))
            .filter_map(|path| path.strip_prefix(&self.root).ok())
            .filter(|path| !path.components().any(|c| is_skipped_dir(c.as_os_str())))
            .map(Path::to_path_buf)
            .collect();
        if !nested.is_empty() {
            self.check_nested_configs(nested, jobs);
        }
    }

    /// Reads the root `genea.jsonc` in the background and applies it. A
    /// missing file is an empty config.
    fn load_config(&mut self, jobs: &Jobs) {
        self.config_generation += 1;
        let generation = self.config_generation;
        let path = self.root.join(CONFIG_FILE);
        let id = self.id;
        jobs.spawn("read config", move || {
            let read = match fs::read_to_string(&path) {
                Ok(text) => Ok(config::parse(&text, Path::new(CONFIG_FILE))),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok((Config::default(), Vec::new())),
                Err(error) => Err(error),
            };
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.config_generation != generation {
                    return;
                }
                let (config, problems) = read.unwrap_or_else(|error| {
                    let problem = Problem {
                        severity: Severity::Error,
                        path: CONFIG_FILE.into(),
                        start: TextPosition::default(),
                        end: TextPosition::default(),
                        message: format!("Couldn't read the config: {error}. It isn't applied."),
                    };
                    (Config::default(), vec![problem])
                });
                project.config = config;
                project.config_problems = problems;
                project.update_config_problems();
            })
        });
    }

    /// Walks the project in the background for `genea.jsonc` files below
    /// the root, skipping `node_modules` and `.git`.
    fn find_nested_configs(&mut self, jobs: &Jobs) {
        let root = self.root.clone();
        let id = self.id;
        jobs.spawn("find nested configs", move || {
            let found: BTreeSet<PathBuf> = ignore::WalkBuilder::new(&root)
                .standard_filters(false)
                .filter_entry(|entry| !is_skipped_dir(entry.file_name()))
                .build()
                .filter_map(Result::ok)
                .filter(|entry| entry.depth() > 1 && entry.file_name() == CONFIG_FILE)
                .filter(|entry| entry.file_type().is_some_and(|t| t.is_file()))
                .filter_map(|entry| entry.path().strip_prefix(&root).ok().map(Path::to_path_buf))
                .collect();
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                project.nested_configs = found;
                project.update_config_problems();
            })
        });
    }

    /// Checks in the background whether these nested config paths (from
    /// watcher events) exist now.
    fn check_nested_configs(&mut self, paths: Vec<PathBuf>, jobs: &Jobs) {
        let root = self.root.clone();
        let id = self.id;
        jobs.spawn("check nested configs", move || {
            let checked: Vec<(bool, PathBuf)> =
                paths.into_iter().map(|path| (root.join(&path).is_file(), path)).collect();
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                for (exists, path) in checked {
                    if exists {
                        project.nested_configs.insert(path);
                    } else {
                        project.nested_configs.remove(&path);
                    }
                }
                project.update_config_problems();
            })
        });
    }

    /// Puts the config's problems (the root file's, plus a warning per
    /// nested config) into Problems.
    fn update_config_problems(&mut self) {
        let nested = self.nested_configs.iter().map(|path| Problem {
            severity: Severity::Warning,
            path: path.clone(),
            start: TextPosition::default(),
            end: TextPosition::default(),
            message: format!("Only the config at the project root applies. This {CONFIG_FILE} is ignored."),
        });
        let problems = self.config_problems.iter().cloned().chain(nested).collect();
        self.problems.replace(ProblemSource::Config, problems);
    }

    /// Captures the login shell's environment in the background, once per
    /// open.
    pub(crate) fn start_environment(&mut self, host: SharedHost, jobs: &Jobs) {
        let environment = self.environment.insert(Environment::new(self.id, self.root.clone(), host));
        environment.capture(jobs);
    }

    /// The environment for a process started for this project: pass every
    /// spec through [`ProcessEnv::apply`] before spawning it. This is the
    /// one way the terminal, scripts and language servers start processes.
    pub(crate) fn process_env(&self) -> ProcessEnv {
        let environment = self.environment.as_ref().expect("the environment starts when the project opens");
        let tools = self.toolchain.iter().flat_map(Toolchain::installed);
        environment.process_env(tools.map(|installed| installed.bin_dir.as_path()))
    }

    /// Reads the toolchain pins and starts the downloads, in the background.
    /// A folder without a root `package.json` has no toolchain. (Checking is
    /// one stat on the main thread, like `open_project`'s folder check.)
    pub(crate) fn start_toolchain(&mut self, context: ToolchainContext, jobs: &Jobs) {
        if !self.root.join("package.json").exists() {
            return;
        }
        let toolchain = self.toolchain.insert(Toolchain::new(self.id, self.root.clone(), context));
        toolchain.load(jobs);
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn dispatch(&mut self, command: Command, jobs: &Jobs, host: &dyn Host) {
        let now = host.clock().now();
        match command {
            Command::OpenFile(path) => self.open_file(path, None, jobs),
            Command::OpenFileAt { path, at } => self.open_file(path, Some(at), jobs),
            Command::OpenConfig => self.open_config(jobs),
            Command::ToggleLeftColumn(view) => {
                self.left_column = if self.left_column == Some(view) { None } else { Some(view) };
            }
            Command::ToggleFolder(path) => self.files.toggle(&path),
            Command::SelectTab { .. }
            | Command::FocusPane(_)
            | Command::CloseTab { .. }
            | Command::ResolveClose(_)
            | Command::SplitRight
            | Command::MoveTabToOtherSide { .. }
            | Command::CloseSplit
            | Command::ScrollPane { .. } => self.tab_command(command, jobs),
            Command::SetViewport { rows } => {
                self.viewport_rows = rows.max(1.0);
                if let Some(editor) = &mut self.editor {
                    editor.scroll_by(0.0, self.viewport_rows);
                }
            }
            Command::ScrollBy { rows } => {
                if let Some(editor) = &mut self.editor {
                    editor.scroll_by(rows, self.viewport_rows);
                }
            }
            Command::MoveCaret(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.move_caret(movement, false, self.viewport_rows);
                }
            }
            Command::Select(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.move_caret(movement, true, self.viewport_rows);
                }
            }
            Command::SelectAll => {
                if let Some(editor) = &mut self.editor {
                    editor.select_all(self.viewport_rows);
                }
            }
            Command::PlaceCaret { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.place_caret(line, column, false, self.viewport_rows);
                }
            }
            Command::RetryToolchain => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.retry(jobs);
                }
            }
            Command::PinToolchainDefaults => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.pin_defaults(jobs);
                }
            }
            Command::ReloadEnvironment => {
                if let Some(environment) = &mut self.environment {
                    environment.capture(jobs);
                }
            }
            Command::ExtendSelection { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.place_caret(line, column, true, self.viewport_rows);
                }
            }
            Command::AddCaret { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.add_caret(line, column, self.viewport_rows);
                }
            }
            Command::SelectNextOccurrence => {
                if let Some(editor) = &mut self.editor {
                    editor.select_next_occurrence(self.viewport_rows);
                }
            }
            Command::UnselectLastOccurrence => {
                if let Some(editor) = &mut self.editor {
                    editor.unselect_last_occurrence(self.viewport_rows);
                }
            }
            Command::SelectAllOccurrences => {
                if let Some(editor) = &mut self.editor {
                    editor.select_all_occurrences(self.viewport_rows);
                }
            }
            Command::CloneCaretAbove | Command::CloneCaretBelow => {
                if let Some(editor) = &mut self.editor {
                    editor.clone_caret(command == Command::CloneCaretAbove, self.viewport_rows);
                }
            }
            Command::CollapseCarets => {
                if let Some(editor) = &mut self.editor {
                    editor.collapse_carets(self.viewport_rows);
                }
            }
            Command::SelectWord { line, column } => {
                if let Some(editor) = &mut self.editor {
                    editor.select_word(line, column, self.viewport_rows);
                }
            }
            Command::SelectLine { line } => {
                if let Some(editor) = &mut self.editor {
                    editor.select_line(line, self.viewport_rows);
                }
            }
            Command::InsertText(text) => {
                if let Some(editor) = &mut self.editor {
                    editor.insert(&text, EditKind::Typing, now, self.viewport_rows);
                }
            }
            Command::SetPreedit(text) => {
                if let Some(editor) = &mut self.editor {
                    editor.set_preedit(text);
                }
            }
            Command::Delete(movement) => {
                if let Some(editor) = &mut self.editor {
                    editor.delete(movement, EditKind::Deleting, now, self.viewport_rows);
                }
            }
            Command::NewLine => {
                if let Some(editor) = &mut self.editor {
                    editor.insert("\n", EditKind::Typing, now, self.viewport_rows);
                }
            }
            Command::Copy => {
                if let Some(text) = self.editor.as_ref().and_then(Editor::selected_text) {
                    host.clipboard().write_text(&text);
                }
            }
            Command::Cut => {
                if let Some(editor) = &mut self.editor
                    && let Some(text) = editor.selected_text()
                {
                    host.clipboard().write_text(&text);
                    editor.delete_selections(now, self.viewport_rows);
                }
            }
            Command::Paste => {
                if let Some(editor) = &mut self.editor
                    && let Some(text) = host.clipboard().read_text()
                {
                    editor.paste(&text, now, self.viewport_rows);
                }
            }
            Command::Undo => {
                if let Some(editor) = &mut self.editor {
                    editor.undo(self.viewport_rows);
                }
            }
            Command::Redo => {
                if let Some(editor) = &mut self.editor {
                    editor.redo(self.viewport_rows);
                }
            }
            Command::Save => {
                if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
                    self.save(path, jobs);
                }
            }
        }
        self.reparse(jobs);
        self.refresh_views();
    }

    /// Starts a background parse of the focused file if its syntax tree is
    /// behind its text and none is running. Only the focused file is edited.
    fn reparse(&mut self, jobs: &Jobs) {
        if let Some(editor) = &mut self.editor
            && let Some(job) = editor.start_parse()
        {
            spawn_parse(self.id, editor.path().to_owned(), job, jobs);
        }
    }

    /// Starts a background parse of an open file (focused or not) if its
    /// syntax tree is behind its text and none is running.
    fn reparse_file(&mut self, path: &Path, jobs: &Jobs) {
        if let Some(job) = self.open_editor_mut(path).and_then(Editor::start_parse) {
            spawn_parse(self.id, path.to_owned(), job, jobs);
        }
    }

    pub(crate) fn view(&self) -> ProjectView {
        let editor = self.editor.as_ref().map(|e| {
            let mut view = e.view(self.viewport_rows);
            view.problems = self.inline_problems(e, &view.lines);
            view
        });
        let mut tabs = self.tabs_view(editor.as_ref());
        // The other side's editor shows its problems too.
        for pane in &mut tabs.panes {
            if let Some(view) = &mut pane.editor
                && let Some(e) = self.open_editor(&view.path)
            {
                view.problems = self.inline_problems(e, &view.lines);
            }
        }
        let (errors, warnings) = self.problems.counts();
        let status = StatusBar {
            caret: editor.as_ref().map(|e| format!("{}:{}", e.caret.line + 1, e.caret.column + 1)),
            errors,
            warnings,
            config_notice: self.config_notice(),
            encoding: self.editor.as_ref().map(|_| "UTF-8".to_owned()),
            line_ending: self.editor.as_ref().map(|e| e.line_ending().label().to_owned()),
            toolchain: self.toolchain.as_ref().and_then(Toolchain::status),
        };
        let mut notices = self.notices.clone();
        notices.extend(self.toolchain.iter().flat_map(Toolchain::notices));
        notices.extend(self.environment.iter().flat_map(Environment::notices));
        ProjectView {
            root: self.root.clone(),
            name: self.root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            editor,
            status,
            notices,
            toolchain: self.toolchain.as_ref().map(Toolchain::view).unwrap_or_default(),
            panes: tabs.panes,
            focused_pane: tabs.focused_pane,
            can_split: tabs.can_split,
            close_prompt: tabs.close_prompt,
            config: self.config.clone(),
            problems: self.problems.items(),
            left_column: self.left_column,
            files: self.files.rows(),
        }
    }

    /// The open file's problems on the visible lines, in grid columns.
    fn inline_problems(&self, editor: &Editor, lines: &[crate::view::VisibleLine]) -> Vec<InlineProblem> {
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else { return Vec::new() };
        let mut inline = Vec::new();
        for problem in self.problems.in_file(editor.path()) {
            for line in problem.start.line.max(first.index)..=problem.end.line.min(last.index) {
                let start = if line == problem.start.line { problem.start.column } else { 0 };
                let end = if line == problem.end.line { problem.end.column } else { usize::MAX };
                let from = editor.grid_column(line, start);
                let to = editor.grid_column(line, end);
                inline.push(InlineProblem {
                    line,
                    columns: from..to.max(from + 1),
                    severity: problem.severity,
                    message: problem.message.clone(),
                });
            }
        }
        inline.sort_by_key(|p| (p.line, p.columns.start));
        inline
    }

    /// The status-bar notice while the config has problems.
    fn config_notice(&self) -> Option<String> {
        let (errors, warnings) =
            self.problems.items().iter().filter(|p| p.source == ProblemSource::Config).fold((0, 0), |(e, w), p| {
                match p.severity {
                    Severity::Error => (e + 1, w),
                    Severity::Warning => (e, w + 1),
                }
            });
        let count = |n: usize, what: &str| format!("{n} {what}{}", if n == 1 { "" } else { "s" });
        match (errors, warnings) {
            (0, 0) => None,
            (e, 0) => Some(format!("{CONFIG_FILE} has {}", count(e, "error"))),
            (0, w) => Some(format!("{CONFIG_FILE} has {}", count(w, "warning"))),
            (e, w) => Some(format!("{CONFIG_FILE} has {} and {}", count(e, "error"), count(w, "warning"))),
        }
    }

    /// Writes an open file in the background, as it is now. Edits made
    /// while it is written stay unsaved; a failed write adds a notice.
    fn save(&mut self, path: PathBuf, jobs: &Jobs) {
        let Some(editor) = self.open_editor(&path).filter(|e| !e.is_read_only()) else { return };
        let snapshot = editor.snapshot();
        let absolute = self.root.join(&snapshot.path);
        let id = self.id;
        jobs.spawn("save file", move || {
            let written = File::create(&absolute).and_then(|file| {
                let mut writer = BufWriter::new(file);
                snapshot.text.write_to(&mut writer)?;
                writer.flush()
            });
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                match written {
                    Ok(()) => {
                        if let Some(editor) = project.open_editor_mut(&snapshot.path) {
                            editor.saved(&snapshot);
                        }
                        // The config applies on save, without waiting for the watcher.
                        if snapshot.path == Path::new(CONFIG_FILE) {
                            project.load_config(&jobs);
                        }
                        project.saved(&snapshot.path);
                    }
                    Err(error) => {
                        project.save_failed(&snapshot.path);
                        project.notices.push(Notice {
                            message: format!("Couldn't save {}: {error}", snapshot.path.display()),
                            action: None,
                        })
                    }
                }
            })
        });
    }

    /// "Open config": opens the root `genea.jsonc`, creating it as `{}`
    /// first if it is missing.
    fn open_config(&mut self, jobs: &Jobs) {
        let path = self.root.join(CONFIG_FILE);
        let id = self.id;
        jobs.spawn("create config", move || {
            let created = match File::create_new(&path) {
                Ok(mut file) => file.write_all(b"{}\n"),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                Err(error) => Err(error),
            };
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                match created {
                    Ok(()) => project.open_file(CONFIG_FILE.into(), None, &jobs),
                    Err(error) => project
                        .notices
                        .push(Notice { message: format!("Couldn't create {CONFIG_FILE}: {error}"), action: None }),
                }
            })
        });
    }

    /// Puts the focused editor's caret at `at` (from `OpenFileAt`).
    fn go_to(&mut self, at: TextPosition) {
        if let Some(editor) = &mut self.editor {
            let column = editor.grid_column(at.line, at.column);
            editor.place_caret(at.line, column, false, self.viewport_rows);
        }
    }

    /// Reads the file in the background and opens it in a new tab, then
    /// puts the caret at `at`. The current editor stays until the new file
    /// is read; a failed read or a binary file leaves it and adds a notice.
    /// A file that isn't valid UTF-8 opens read-only. A file that is
    /// already open just has its tab focused, keeping its buffer.
    fn open_file(&mut self, path: PathBuf, at: Option<TextPosition>, jobs: &Jobs) {
        let absolute = self.root.join(&path);
        let shown = absolute.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or_else(|_| absolute.clone());
        self.open_generation += 1;
        if self.focus_open_file(&shown) {
            if let Some(at) = at {
                self.go_to(at);
            }
            return;
        }
        let generation = self.open_generation;
        let id = self.id;
        jobs.spawn("open file", move || {
            let read = std::fs::read(&absolute).map(Decoded::from_bytes);
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                if project.open_generation != generation {
                    return;
                }
                match read {
                    Ok(Decoded::Text(text)) => {
                        project.open_tab(Editor::new(shown.clone(), Rope::from_str(&text)));
                        project.reparse_file(&shown, &jobs);
                        if let Some(at) = at {
                            project.go_to(at);
                        }
                    }
                    Ok(Decoded::Invalid(text)) => {
                        project.notices.push(Notice {
                            message: format!("{} isn't valid UTF-8, so it's open read-only.", shown.display()),
                            action: None,
                        });
                        project.open_tab(Editor::new(shown.clone(), Rope::from_str(&text)).read_only());
                        project.reparse_file(&shown, &jobs);
                        if let Some(at) = at {
                            project.go_to(at);
                        }
                    }
                    Ok(Decoded::Binary) => project.notices.push(Notice {
                        message: format!("{} is a binary file, so Genea doesn't open it in the editor.", shown.display()),
                        action: None,
                    }),
                    Err(error) => project
                        .notices
                        .push(Notice { message: format!("Couldn't open {}: {error}", shown.display()), action: None }),
                }
            })
        });
    }
}

/// Folders the search for nested configs never looks in.
fn is_skipped_dir(name: &OsStr) -> bool {
    name == "node_modules" || name == ".git"
}

/// Parses a file in the background. When the parse lands, the file (which
/// may have moved to another tab or pane meanwhile) takes it, and the next
/// parse starts if its text changed while this one ran.
fn spawn_parse(id: ProjectId, path: PathBuf, job: ParseJob, jobs: &Jobs) {
    jobs.spawn("parse", move || {
        let parsed = job.run();
        Box::new(move |core| {
            let jobs = core.jobs.clone();
            let Some(project) = core.project_mut(id) else { return };
            if let Some(editor) = project.open_editor_mut(&path) {
                editor.parsed(parsed);
            }
            project.reparse_file(&path, &jobs);
        })
    });
}
