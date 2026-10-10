//! One open project: its folder and what its window shows.

mod changes;
mod check;
mod decorations;
mod assist;
mod external;
mod inline_diff;
mod language;
mod oxlint;
mod finder;
mod quick_fixes;
mod symbols;
mod tabs;

use std::{
    collections::BTreeSet,
    ffi::OsStr,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::{Path, PathBuf},
};

use genea_host::{Host, SharedHost};

use crate::{
    command::Command,
    config::{self, CONFIG_FILE, Config},
    dependencies::Dependencies,
    editor::Editor,
    environment::{Environment, ProcessEnv},
    files::FileIndex,
    finder::Finder,
    git::Git,
    history::EditKind,
    indentation::{self, Indentation, IndentationConfig},
    jobs::Jobs,
    problems::{Problem, ProblemSource, Problems, Severity, TextPosition},
    reading::{self, Contents, FirstScreen},
    review::{
        Review,
        store::{hash_bytes, hash_rope},
    },
    search::Search,
    syntax::ParseJob,
    terminal::Terminal,
    toolchain::{LOCKFILES, Toolchain, ToolchainContext},
    view::{InlineProblem, LeftColumnView, Notice, NoticeAction, ProjectView, StatusBar},
    watcher::{FileChanges, Watcher},
    workbench::ProjectId,
    workspace::Workspace,
};
use check::Check;
use inline_diff::InlineDiffs;
use assist::Assist;
use language::Language;
use quick_fixes::QuickFixes;
use tabs::Panes;

/// The status-bar item for a large file.
const LARGE_FILE_NOTICE: &str = "Over 5 MB: no highlighting or language features";

/// The package manager's arguments for "Install dependencies".
const INSTALL: [&str; 1] = ["install"];

/// Why a folder without a root `package.json` has no toolchain.
const NO_PACKAGE_JSON: &str =
    "Genea manages the runtime and package manager of a project with a package.json at its root, and this folder has none.";

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
    /// What `.oxfmtrc.json` and `.editorconfig` say about indentation
    /// (ticket #26), and a counter like `config_generation`'s.
    indentation: IndentationConfig,
    indentation_generation: u64,
    /// `genea.jsonc` files below the root (relative paths), each ignored
    /// with a warning.
    nested_configs: BTreeSet<PathBuf>,
    /// Every source's errors and warnings: the Problems view.
    problems: Problems,
    /// What the left column shows; `None` while it is collapsed.
    left_column: Option<LeftColumnView>,
    /// The change whose popover is open, by file and line (ticket #56).
    shown_hunk: Option<(PathBuf, usize)>,
    /// The project's files, for the Files view (ticket #30).
    pub(crate) files: FileIndex,
    /// The Search view's query and results (ticket #34).
    pub(crate) search: Search,
    /// The runtime and package manager (ticket #35); set by `start_toolchain`.
    pub(crate) toolchain: Option<Toolchain>,
    /// What its processes get (ticket #36); set by `start_environment`.
    pub(crate) environment: Option<Environment>,
    /// External changes against the review baseline (ticket #53).
    pub(crate) review: Review,
    /// TypeScript 7 and tsgo (ticket #42); set up by `start_language`.
    language: Language,
    /// The project check (ticket #48).
    check: Check,
    /// Completion, hover and signature help (ticket #43).
    assist: Assist,
    /// The branch and the open files at HEAD (ticket #56).
    pub(crate) git: Git,
    /// The terminal pane's shell (ticket #38).
    pub(crate) terminal: Terminal,
    /// Whether `node_modules` is there (ticket #41).
    pub(crate) dependencies: Dependencies,
    /// The packages and their scripts (ticket #40).
    pub(crate) workspace: Workspace,
    /// "Install dependencies" came while `package.json` was being read: it
    /// runs once it is read.
    install_requested: bool,
    /// The fuzzy finder, while it is open (ticket #33).
    finder: Option<Finder>,
    /// Bumped by every finder match, so an older match can't replace a
    /// newer one.
    finder_generation: u64,
    /// The file index version the last finder match used.
    finder_files: u64,
    /// Files opened lately, most recent first: Recent Files (⌘E).
    recent_files: Vec<PathBuf>,
    /// Commands only the workbench can carry out (New Project…), from a
    /// menu or the finder; it takes them after each dispatch.
    for_workbench: Vec<Command>,
    /// Code-action requests and the quick-fix popup (ticket #45).
    quick_fixes: QuickFixes,
    /// Files shown with an inline diff (ticket #55).
    inline_diffs: InlineDiffs,
}

impl Project {
    pub(crate) fn new(id: ProjectId, root: PathBuf, jobs: &Jobs) -> Self {
        Project {
            id,
            review: Review::new(id, root.clone(), jobs),
            files: FileIndex::new(id, root.clone()),
            git: Git::new(id, root.clone()),
            terminal: Terminal::new(id, root.clone()),
            dependencies: Dependencies::new(id, &root),
            workspace: Workspace::new(id, root.clone()),
            search: Search::new(id, root.clone()),
            root,
            editor: None,
            panes: Panes::default(),
            viewport_rows: DEFAULT_VIEWPORT_ROWS,
            notices: Vec::new(),
            open_generation: 0,
            toolchain: None,
            environment: None,
            language: Language::default(),
            check: Check::default(),
            assist: Assist::default(),
            watcher: None,
            config: Config::default(),
            config_problems: Vec::new(),
            config_generation: 0,
            indentation: IndentationConfig::default(),
            indentation_generation: 0,
            nested_configs: BTreeSet::new(),
            problems: Problems::default(),
            left_column: Some(LeftColumnView::Files),
            shown_hunk: None,
            install_requested: false,
            finder: None,
            quick_fixes: QuickFixes::default(),
            inline_diffs: InlineDiffs::default(),
            finder_generation: 0,
            finder_files: 0,
            recent_files: Vec::new(),
            for_workbench: Vec::new(),
        }
    }

    /// The commands this project's last dispatch left for the workbench.
    pub(crate) fn take_workbench_commands(&mut self) -> Vec<Command> {
        std::mem::take(&mut self.for_workbench)
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
        self.load_indentation(jobs);
        self.find_nested_configs(jobs);
        self.files.start(jobs);
        self.review.start(host.support_dir(), jobs);
        self.git.reload(Vec::new(), jobs);
        self.dependencies.check(jobs);
        self.workspace.discover(jobs);
    }

    /// Writes a watcher cookie (see `Watcher::sync`). Returns whether one
    /// was written.
    pub(crate) fn sync_watcher(&self) -> bool {
        self.watcher.as_ref().is_some_and(Watcher::sync)
    }

    /// Files changed on disk (from the watcher). Every area that follows
    /// files on disk hooks in here.
    pub(crate) fn files_changed(&mut self, changes: FileChanges, jobs: &Jobs) {
        self.files.files_changed(&changes, jobs);
        self.dependencies.files_changed(&changes, jobs);
        self.workspace.files_changed(&changes, jobs);
        self.language_files_changed(&changes);
        self.project_check_files_changed(&changes);
        if self.git.head_may_have_moved(&changes) {
            let open = self.open_editors().map(|e| e.path().to_owned()).collect();
            self.git.reload(open, jobs);
        }
        self.check_open_files(&changes, jobs);
        self.review.files_changed(&changes, jobs);
        let root_config = self.root.join(CONFIG_FILE);
        if let Some(toolchain) = &mut self.toolchain
            && (changes.rescan || LOCKFILES.iter().any(|name| changes.paths.contains(&self.root.join(name))))
        {
            toolchain.check_lockfiles(jobs);
        }
        if changes.rescan || changes.paths.iter().any(|path| self.is_indentation_config(path)) {
            self.load_indentation(jobs);
        }
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
                project.files.set_exclude(&config.exclude);
                project.config = config;
                project.config_problems = problems;
                project.update_config_problems();
            })
        });
    }

    /// Reads what `.oxfmtrc.json` and `.editorconfig` say about indentation
    /// in the background. Open files follow it at once: it is resolved per
    /// keystroke and per view.
    fn load_indentation(&mut self, jobs: &Jobs) {
        self.indentation_generation += 1;
        let generation = self.indentation_generation;
        let root = self.root.clone();
        let id = self.id;
        jobs.spawn("read indentation config", move || {
            let config = IndentationConfig::read(&root);
            Box::new(move |core| {
                let Some(project) = core.project_mut(id) else { return };
                if project.indentation_generation == generation {
                    project.indentation = config;
                }
            })
        });
    }

    /// Whether a changed path is a file the indentation is read from. Only
    /// the root's can change: the watcher doesn't see the folders above it.
    fn is_indentation_config(&self, path: &Path) -> bool {
        path.parent() == Some(&self.root)
            && path.file_name().and_then(OsStr::to_str).is_some_and(indentation::is_config_file)
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

    /// Whether the environment for new processes is final for now: the
    /// login-shell capture has landed and the toolchain has settled, so the
    /// pinned tools are on PATH.
    fn environment_ready(&self) -> bool {
        self.environment.as_ref().is_some_and(Environment::is_ready)
            && self.toolchain.as_ref().is_none_or(Toolchain::is_settled)
    }

    /// Starts the terminal's shell once the environment is ready. Called
    /// after opening and after every background result.
    pub(crate) fn start_terminal_when_ready(&mut self, host: &SharedHost, jobs: &Jobs) {
        if self.install_requested && !self.toolchain.as_ref().is_some_and(Toolchain::is_loading) {
            self.install_requested = false;
            self.install_dependencies();
        }
        if self.terminal.is_waiting() && self.environment_ready() {
            let env = self.process_env();
            let package_manager = match &self.toolchain {
                Some(toolchain) => toolchain.package_manager_program(),
                None => Err(NO_PACKAGE_JSON.into()),
            };
            self.terminal.start(env, package_manager, host.clone(), jobs);
        }
    }

    /// "Install dependencies": the pinned package manager's `install` in a
    /// terminal tab, which starts once the environment is ready.
    fn install_dependencies(&mut self) {
        if self.toolchain.as_ref().is_some_and(Toolchain::is_loading) {
            self.install_requested = true;
            return;
        }
        match self.toolchain.as_ref().map(Toolchain::package_manager_name) {
            Some(Ok(name)) => self.terminal.run_package_manager(name, &INSTALL),
            Some(Err(reason)) => self.notify(reason),
            None => self.notify(NO_PACKAGE_JSON.into()),
        }
    }

    /// Runs a package's script in a terminal tab (ticket #40), which starts
    /// once the environment is ready.
    fn run_script(&mut self, package: &Path, script: &str) {
        let Some(found) = self.workspace.package(package) else { return };
        if !found.scripts.iter().any(|s| s.name == script) {
            return;
        }
        let title = format!("{}: {script}", found.name);
        self.terminal.run_script(self.root.join(package), title, script);
    }

    /// Reads the toolchain pins and starts the downloads, in the background.
    /// A folder without a root `package.json` has no toolchain. (Checking is
    /// one stat on the main thread, like `open_project`'s folder check.)
    pub(crate) fn start_toolchain(&mut self, context: ToolchainContext, jobs: &Jobs) {
        if !self.root.join("package.json").exists() {
            return;
        }
        let own_writes = self.review.own_writes();
        let toolchain = self.toolchain.insert(Toolchain::new(self.id, self.root.clone(), context, own_writes));
        toolchain.load(jobs);
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Replaces a whole-project source's problems.
    pub(crate) fn replace_problems(&mut self, source: ProblemSource, problems: Vec<Problem>) {
        self.problems.replace(source, problems);
    }

    /// Shows a message in the project's window.
    pub(crate) fn notify(&mut self, message: String) {
        self.notices.push(Notice { message, action: None });
    }

    pub(crate) fn dispatch(&mut self, command: Command, jobs: &Jobs, host: &dyn Host) {
        let now = host.clock().now();
        // A shown change closes on anything but scrolling.
        let scrolling = matches!(command, Command::SetViewport { .. } | Command::ScrollBy { .. } | Command::ScrollPane { .. });
        let shown = if scrolling { None } else { self.shown_hunk.take() };
        self.quick_fixes_before(&command);
        if !scrolling {
            self.clear_hint();
        }
        let assist = assist::Trigger::of(&command);
        match command {
            Command::ShowHunk { line } => {
                self.shown_hunk = self.editor.as_ref().map(|e| (e.path().to_owned(), line));
            }
            Command::HideHunk => {}
            Command::RollbackHunk => {
                // `shown_hunk` was taken above; it is still the one shown.
                if let Some(editor) = &mut self.editor
                    && let Some((path, line)) = shown.filter(|(path, _)| path == editor.path())
                    && let Some((lines, text)) = self.git.rollback(&path, editor.version(), line)
                {
                    editor.replace_lines(lines, &text, now, self.viewport_rows);
                }
            }
            Command::OpenConfig => self.open_config(jobs),
            Command::ToggleLeftColumn(view) => {
                self.left_column = if self.left_column == Some(view) { None } else { Some(view) };
            }
            Command::ToggleFolder(path) => self.files.toggle(&path),
            Command::Search(query) => self.search.start(query, &self.config.exclude, jobs),
            Command::SelectTab { .. } | Command::FocusPane(_) => {
                self.terminal.unfocus();
                self.tab_command(command, jobs)
            }
            Command::OpenFile(path) => {
                self.terminal.unfocus();
                self.open_file(path, None, jobs)
            }
            Command::OpenFileAt { path, at } => {
                self.terminal.unfocus();
                self.open_file(path, Some(at), jobs)
            }
            Command::OpenTerminalLink { line, column } => {
                if let Some((path, at)) = self.terminal.file_link_at(line, column) {
                    self.terminal.unfocus();
                    self.open_file(path, Some(at), jobs)
                }
            }
            Command::CloseTab { .. }
            | Command::ResolveClose(_)
            | Command::SplitRight
            | Command::MoveTabToOtherSide { .. }
            | Command::CloseSplit
            | Command::ScrollPane { .. } => self.tab_command(command, jobs),
            Command::ToggleTerminal
            | Command::FocusTerminal
            | Command::SelectTerminalTab(_)
            | Command::SetTerminalSize { .. }
            | Command::TerminalText(_)
            | Command::TerminalPreedit(_)
            | Command::TerminalKey(..)
            | Command::ScrollTerminal { .. }
            | Command::TerminalMouse { .. }
            | Command::TerminalPaste
            | Command::NewTerminalTab
            | Command::CloseTerminalTab(_)
            | Command::StopTerminalTab(_)
            | Command::RerunTerminalTab(_) => self.terminal.command(command, host),
            Command::ResolveConflict { path, choice } => self.resolve_conflict(&path, choice, now, jobs),
            Command::KeepChange(path) => self.review.keep(Some(path), jobs),
            Command::RevertChange(path) => self.review.revert(Some(path), jobs),
            Command::KeepAllChanges => self.review.keep(None, jobs),
            Command::RevertAllChanges => self.review.revert(None, jobs),
            Command::OpenChange(_) => {
                self.terminal.unfocus();
                self.inline_diff_command(command, jobs)
            }
            Command::CloseInlineDiff => self.inline_diff_command(command, jobs),
            Command::ShowQuickFixes
            | Command::MoveQuickFixSelection(_)
            | Command::ApplyQuickFix(_)
            | Command::CloseQuickFixes
            | Command::OrganizeImports => self.quick_fix_command(command, now, jobs),
            Command::RunProjectCheck => self.run_project_check(),
            Command::RestartLanguageServer => {
                self.restart_language_server();
                self.restart_oxlint();
            }
            Command::ShowCompletion
            | Command::MoveCompletionSelection(_)
            | Command::SelectCompletionItem(_)
            | Command::AcceptCompletion
            | Command::CloseCompletion
            | Command::HoverAt { .. }
            | Command::ShowHover
            | Command::HideHover
            | Command::ShowSignatureHelp
            | Command::HideSignatureHelp => self.assist_command(command, now),
            Command::AddTypeScript => self.add_typescript(jobs),
            Command::AddOxlintAndOxfmt => self.add_oxc(jobs),
            Command::OpenFinder(_)
            | Command::SetFinderQuery(_)
            | Command::MoveFinderSelection(_)
            | Command::SelectFinderItem(_)
            | Command::AcceptFinder
            | Command::CloseFinder => self.finder_command(command, jobs, host),
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
            Command::OpenToolchainPicker(kind) => match &mut self.toolchain {
                Some(toolchain) => toolchain.open_picker(kind, jobs),
                None => self.notify(NO_PACKAGE_JSON.into()),
            },
            Command::FilterToolchainPicker(query) => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.filter_picker(query);
                }
            }
            Command::CloseToolchainPicker => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.close_picker();
                }
            }
            Command::SetRuntime(pin) => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.set_runtime(pin, jobs);
                }
            }
            Command::SetPackageManager(pin) => {
                if let Some(toolchain) = &mut self.toolchain {
                    toolchain.set_package_manager(pin, jobs);
                }
            }
            // The workbench handles it: it needs the recent projects.
            Command::RemoveUnusedToolchains => {}
            // The dialog isn't the project's (ticket #61).
            Command::NewProject => self.for_workbench.push(command),
            Command::ReloadEnvironment => {
                if let Some(environment) = &mut self.environment {
                    environment.capture(jobs);
                }
            }
            Command::InstallDependencies => self.install_dependencies(),
            Command::RunScript { package, script } => self.run_script(&package, &script),
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
                let indentation = self.focused_indentation();
                if let Some(editor) = &mut self.editor {
                    editor.new_line(indentation, now, self.viewport_rows);
                }
            }
            Command::Indent | Command::Outdent => {
                let indentation = self.focused_indentation();
                if let Some(editor) = &mut self.editor {
                    if command == Command::Indent {
                        editor.indent(indentation, now, self.viewport_rows);
                    } else {
                        editor.outdent(indentation, now, self.viewport_rows);
                    }
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
            Command::ToggleLineComment => {
                if let Some(editor) = &mut self.editor {
                    editor.toggle_line_comment(now, self.viewport_rows);
                }
            }
            Command::ExpandSelection => {
                if let Some(editor) = &mut self.editor {
                    editor.expand_selection(self.viewport_rows);
                }
            }
            Command::ShrinkSelection => {
                if let Some(editor) = &mut self.editor {
                    editor.shrink_selection(self.viewport_rows);
                }
            }
            Command::ToggleFold { line } => {
                if let Some(editor) = &mut self.editor {
                    editor.toggle_fold(line, self.viewport_rows);
                }
            }
            Command::CollapseFold | Command::ExpandFold | Command::CollapseAllFolds | Command::ExpandAllFolds => {
                if let Some(editor) = &mut self.editor {
                    match command {
                        Command::CollapseFold => editor.collapse_fold(self.viewport_rows),
                        Command::ExpandFold => editor.expand_fold(self.viewport_rows),
                        Command::CollapseAllFolds => editor.collapse_all_folds(self.viewport_rows),
                        _ => editor.expand_all_folds(self.viewport_rows),
                    }
                }
            }
            Command::Save => {
                if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
                    self.save(path, jobs);
                }
            }
            Command::GoToDefinition
            | Command::GoToTypeDefinition
            | Command::GoToImplementation
            | Command::FindUsages
            | Command::StartRename
            | Command::Rename(_)
            | Command::CancelRename => self.navigation_command(command),
        }
        self.reparse(jobs);
        if let Some(path) = self.editor.as_ref().map(|e| e.path().to_owned()) {
            self.diff_file(&path, jobs);
        }
        self.refresh_views();
        self.sync_language();
        self.sync_project_check();
        self.assist_after(assist);
    }

    /// Starts a background diff of an open file with its text at HEAD, if
    /// its gutter markers are behind and none is running (ticket #56).
    pub(crate) fn diff_file(&mut self, path: &Path, jobs: &Jobs) {
        self.diff_inline(path, jobs);
        let Some(editor) = self.open_editor(path) else { return };
        let (version, text) = (editor.version(), editor.snapshot().text);
        if let Some(job) = self.git.start_diff(path, version, text) {
            let (id, path) = (self.id, path.to_owned());
            jobs.spawn("diff with git HEAD", move || {
                let diffed = job.run();
                Box::new(move |core| {
                    let jobs = core.jobs.clone();
                    let Some(project) = core.project_mut(id) else { return };
                    project.git.diffed(&path, diffed);
                    project.diff_file(&path, &jobs);
                })
            });
        }
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
            view.gutter = self.gutter(e, &view.lines);
            view.hunk = self
                .shown_hunk
                .as_ref()
                .filter(|(path, _)| path == e.path())
                .and_then(|(path, line)| self.git.hunk_view(path, e.version(), *line));
            self.complete_inline_diff(&mut view);
            self.assist_view(e, &mut view);
            view
        });
        let mut tabs = self.tabs_view(editor.as_ref());
        // The other side's editor shows its problems too.
        for pane in &mut tabs.panes {
            if let Some(view) = &mut pane.editor
                && let Some(e) = self.open_editor(&view.path)
            {
                view.problems = self.inline_problems(e, &view.lines);
                view.gutter = self.gutter(e, &view.lines);
                self.complete_inline_diff(view);
            }
        }
        let (errors, warnings) = self.problems.counts();
        let status = StatusBar {
            caret: self.editor.as_ref().map(Editor::caret_label),
            errors,
            warnings,
            config_notice: self.config_notice(),
            encoding: self.editor.as_ref().map(|_| "UTF-8".to_owned()),
            line_ending: self.editor.as_ref().map(|e| e.line_ending().label().to_owned()),
            indentation: self.editor.as_ref().map(|e| self.indentation_of(e.path()).label()),
            toolchain: self.toolchain.as_ref().and_then(Toolchain::status),
            branch: self.git.branch().map(str::to_owned),
            large_file: self.editor.as_ref().filter(|e| e.is_large()).map(|_| LARGE_FILE_NOTICE.to_owned()),
            language_servers: self.language_status().into_iter().chain(self.oxlint_status()).collect(),
            project_check: self.project_check_status(),
            script_links: self.terminal.script_links(),
        };
        let mut notices = self.notices.clone();
        notices.extend(self.toolchain.iter().flat_map(Toolchain::notices));
        notices.extend(self.install_notice());
        notices.extend(self.environment.iter().flat_map(Environment::notices));
        notices.extend(self.language_notices());
        notices.extend(self.oxlint_notices());
        notices.extend(self.project_check_notices());
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
            terminal: self.terminal.view(),
            toolchain_picker: self.toolchain.as_ref().and_then(Toolchain::picker_view),
            files: self.files.rows(),
            changes: self.review.rows(),
            review_banner: self.review.banner(),
            search: self.search.view(),
            finder: self.finder.as_ref().map(Finder::view),
            quick_fixes: self.quick_fixes_view(),
            scripts: self.workspace.packages().to_vec(),
            usages: self.usages_view(),
            rename: self.rename_prompt(),
            hint: self.hint(),
        }
    }

    /// "Install dependencies", while `node_modules` is missing in a project
    /// whose package manager Genea runs, and no install is under way.
    fn install_notice(&self) -> Option<Notice> {
        let offer = self.dependencies.are_missing()
            && self.toolchain.as_ref().is_some_and(Toolchain::can_install)
            && !self.install_requested
            && !self.terminal.is_running_package_manager(&INSTALL);
        offer.then(|| Notice {
            message: "This project's dependencies aren't installed.".into(),
            action: Some(NoticeAction { label: "Install dependencies".into(), command: Command::InstallDependencies }),
        })
    }

    /// How a file (relative to the root, or absolute) is indented.
    fn indentation_of(&self, path: &Path) -> Indentation {
        self.indentation.resolve(&self.root.join(path))
    }

    /// How the focused file is indented.
    fn focused_indentation(&self) -> Indentation {
        self.editor.as_ref().map(|e| self.indentation_of(e.path())).unwrap_or_default()
    }

    /// The open file's git gutter markers on the visible lines; lines
    /// hidden in a collapsed fold have none.
    fn gutter(&self, editor: &Editor, lines: &[crate::view::VisibleLine]) -> Vec<crate::view::GutterMark> {
        let (Some(first), Some(last)) = (lines.first(), lines.last()) else { return Vec::new() };
        let mut marks = self.git.gutter(editor.path(), first.index..last.index + 1);
        marks.retain(|m| lines.binary_search_by_key(&m.line, |l| l.index).is_ok());
        marks
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
        let Some(editor) = self.open_editor_mut(&path).filter(|e| !e.is_read_only()) else { return };
        let snapshot = editor.start_save();
        let absolute = self.root.join(&snapshot.path);
        let id = self.id;
        let own = self.review.own_writes();
        jobs.spawn("save file", move || {
            // Review knows this write as Genea's own (ticket #53).
            let writing = own.writing(&snapshot.path, Some(hash_rope(&snapshot.text)));
            let written = File::create(&absolute).and_then(|file| {
                let mut writer = BufWriter::new(file);
                snapshot.text.write_to(&mut writer)?;
                writer.flush()
            });
            drop(writing);
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
        let own = self.review.own_writes();
        jobs.spawn("create config", move || {
            const EMPTY: &[u8] = b"{}\n";
            let created = if path.exists() {
                Ok(())
            } else {
                // Review knows this write as Genea's own (ticket #53).
                let _writing = own.writing(Path::new(CONFIG_FILE), Some(hash_bytes(EMPTY)));
                match File::create_new(&path) {
                    Ok(mut file) => file.write_all(EMPTY),
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => Ok(()),
                    Err(error) => Err(error),
                }
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
    ///
    /// A file that may be large opens as soon as its first screen is read,
    /// read-only until the rest is in (ticket #27, `reading`).
    fn open_file(&mut self, path: PathBuf, at: Option<TextPosition>, jobs: &Jobs) {
        let absolute = self.root.join(&path);
        let shown = absolute.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or_else(|_| absolute.clone());
        self.open_generation += 1;
        if self.focus_open_file(&shown) {
            self.opened_file(&shown);
            if let Some(at) = at {
                self.go_to(at);
            }
            return;
        }
        let generation = self.open_generation;
        let id = self.id;
        let rows = self.viewport_rows.ceil() as usize;
        let first_screen_jobs = jobs.clone();
        jobs.spawn("open file", move || {
            let read = reading::read(&absolute, rows, |first_screen| {
                let shown = shown.clone();
                first_screen_jobs.busy().finish(Box::new(move |core| {
                    let Some(project) = core.project_mut(id) else { return };
                    if project.open_generation == generation {
                        let FirstScreen { text, size } = first_screen;
                        project.open_tab(Editor::loading(shown, text, size, generation));
                    }
                }));
            });
            Box::new(move |core| {
                let jobs = core.jobs.clone();
                let Some(project) = core.project_mut(id) else { return };
                project.file_read(shown, generation, at, read, &jobs);
            })
        });
    }

    /// The OpenFile with this generation has read its file: shows it in a
    /// new tab, or in the tab already showing its first screen.
    fn file_read(
        &mut self,
        path: PathBuf,
        generation: u64,
        at: Option<TextPosition>,
        read: io::Result<Contents>,
        jobs: &Jobs,
    ) {
        let loading = self.open_editor(&path).is_some_and(|e| e.is_loading(generation));
        if !loading && self.open_generation != generation {
            return;
        }
        let editor = match read {
            Ok(Contents::Text(text)) => Editor::new(path.clone(), text),
            Ok(Contents::Invalid(text)) => {
                self.notices.push(Notice {
                    message: format!("{} isn't valid UTF-8, so it's open read-only.", path.display()),
                    action: None,
                });
                Editor::new(path.clone(), text).read_only()
            }
            Ok(Contents::Binary) => {
                let message = format!("{} is a binary file, so Genea doesn't open it in the editor.", path.display());
                return self.open_failed(&path, generation, message);
            }
            Err(error) => return self.open_failed(&path, generation, format!("Couldn't open {}: {error}", path.display())),
        };
        let rows = self.viewport_rows;
        match self.open_editor_mut(&path).filter(|e| e.is_loading(generation)) {
            Some(first_screen) => first_screen.finish_loading(editor, rows),
            None => self.open_tab(editor),
        }
        self.opened_file(&path);
        if let Some(at) = at
            && self.editor.as_ref().is_some_and(|e| e.path() == path)
        {
            self.go_to(at);
        }
        self.reparse_file(&path, jobs);
        self.git.load_base(&path, jobs);
        self.diff_inline(&path, jobs);
    }

    /// Reading a file failed: a notice says why. A tab showing its first
    /// screen keeps that, read-only.
    fn open_failed(&mut self, path: &Path, generation: u64, message: String) {
        if let Some(first_screen) = self.open_editor_mut(path).filter(|e| e.is_loading(generation)) {
            first_screen.stop_loading();
        }
        self.notices.push(Notice { message, action: None });
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
