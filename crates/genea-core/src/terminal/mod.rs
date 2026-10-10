//! The terminal (tickets #38, #39, #41): a pane of tabs, each a program on
//! a PTY from the host, emulated by `alacritty_terminal`.
//!
//! The first tab is a shell: the user's `$SHELL` as a login shell, in the
//! project root; `NewTerminalTab` opens more, and any tab can be closed.
//! "Install dependencies" adds a tab running the pinned package manager
//! (`run_package_manager`). Every tab's program gets the
//! project environment and `TERM=xterm-256color`, and starts once that
//! environment is final: the login-shell capture has landed and the
//! toolchain has settled (so the pinned tools are on its PATH).
//!
//! Threads (spec #19, Threading): per running tab, a reader thread reads
//! the PTY and parses the output into the [`Term`] under a lock; a writer
//! thread writes typed input and resizes the PTY. Neither the reads, the
//! writes nor the parsing happen on the main thread. The visible grid is
//! copied into view state only when view state is read after new output,
//! so however much a program prints, it is copied at most once per read:
//! the app reads once per frame. The reader wakes the main thread only when
//! the last copy has been taken (or the program asked for something, like a
//! new title).
//!
//! `path:line:col` references in a tab's output (`links.rs`) are found
//! when its grid is copied, and resolved against the tab's directory when
//! one is clicked (`file_link_at`).

use std::{
    cell::{Ref, RefCell},
    ffi::OsStr,
    io::{self, Read, Write},
    path::{Component, Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::Duration,
};

use alacritty_terminal::{
    Term,
    event::{Event, EventListener, WindowSize},
    grid::{Dimensions, Scroll},
    term::{Config, TermMode},
    vte::ansi::{Processor, Timeout},
};
use genea_host::{Exit, Host, ProcessSpec, Pty, PtyControl, PtySize, SharedHost};

mod grid;
mod input;
mod links;
mod urls;

use grid::Screen;
use urls::UrlFinder;

use crate::{
    command::{Command, Modifiers, TerminalKey},
    environment::ProcessEnv,
    jobs::Jobs,
    problems::TextPosition,
    view::{ScriptLink, TerminalLine, TerminalStatus, TerminalTab, TerminalView},
    workbench::{Core, ProjectId},
};

/// Lines of scrollback (spec #19).
const SCROLLBACK: usize = 10_000;

/// The size until the view reports one.
const DEFAULT_SIZE: Size = Size { rows: 24, columns: 80 };

/// The reader parses at most this much per hold of the grid's lock, so the
/// main thread's copy never waits long behind a flood.
const PARSE_CHUNK: usize = 16 * 1024;

/// How long Stop waits for a program to end on SIGINT before it kills it
/// (ticket #40). On the host clock.
pub const SCRIPT_STOP_TIMEOUT: Duration = Duration::from_secs(5);

/// The shell when the environment names none: macOS's default.
const DEFAULT_SHELL: &str = "/bin/zsh";

/// The terminal pane.
pub(crate) struct Terminal {
    project: ProjectId,
    /// The project root: the folder the tabs' programs run in.
    root: PathBuf,
    /// Every tab's grid size.
    size: Size,
    /// The tabs, left to right: a shell first, then the tabs opened since.
    /// Empty once the user closed the last one; showing the pane again
    /// opens a new shell.
    tabs: Vec<Tab>,
    /// Index into `tabs` of the one showing.
    active: usize,
    /// Bumped by every start of any tab, so a stale session's results are
    /// dropped and a session's results find their tab.
    generation: u64,
    /// The pane is showing.
    visible: bool,
    /// The pane has the keyboard focus.
    focused: bool,
    /// The IME composition being typed.
    preedit: Option<String>,
}

/// What a tab runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Launch {
    /// The user's login shell.
    Shell,
    /// The project's package manager, `name` (`pnpm`), with `args`.
    PackageManager { name: String, args: Vec<String> },
    /// A `package.json` script, through the package manager's `run`, in the
    /// tab's directory (ticket #40).
    Script { script: String },
}

/// One tab: its program, and what it showed.
struct Tab {
    launch: Launch,
    /// The folder its program runs in, which relative paths in its output
    /// are relative to.
    directory: PathBuf,
    process: Process,
    /// The generation of its last start.
    generation: u64,
    /// What the tab showed when last copied from the session; copied again
    /// when read after new output (see [`Tab::screen`]).
    screen: RefCell<Screen>,
    /// The tab's name until the program sets a title: the running shell's
    /// file name, or the package manager's command line.
    name: String,
    /// The title the program set.
    title: Option<String>,
    /// The user stopped its program (ticket #40): its end shows as stopped.
    stopped: bool,
    /// A script's link: the first local URL its program printed, while it
    /// runs (ticket #40).
    url: Option<String>,
}

enum Process {
    /// Waiting for the project environment.
    Waiting,
    Starting,
    Running(Session),
    Exited(Exit),
    Failed(String),
}

/// A running program: its emulator, and the threads around its PTY.
struct Session {
    term: Arc<Mutex<Term<Listener>>>,
    /// To the writer thread.
    to_pty: mpsc::Sender<ToPty>,
    control: Arc<dyn PtyControl>,
    /// The emulator has changed since the screen was last copied.
    dirty: Arc<AtomicBool>,
    /// What the program asked of Genea while its output was parsed.
    requests: Arc<Mutex<Requests>>,
    /// The size the emulator's listener reports.
    size: Arc<Mutex<Size>>,
}

/// What a program asks of the terminal beyond drawing, gathered on the
/// reader thread and carried out on the main thread.
#[derive(Default)]
struct Requests {
    /// The title it set (`Some(None)`: it reset it).
    title: Option<Option<String>>,
    /// Text to put on the clipboard (OSC 52).
    copy: Option<String>,
    /// A script printed its first local URL.
    url: Option<String>,
}

impl Requests {
    fn is_asked(&self) -> bool {
        self.title.is_some() || self.copy.is_some() || self.url.is_some()
    }
}

impl Drop for Session {
    /// Closing the terminal (or the project) hangs up its programs.
    fn drop(&mut self) {
        let _ = self.control.hang_up();
    }
}

/// What the writer thread does to the PTY.
enum ToPty {
    Input(Vec<u8>),
    Resize(PtySize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Size {
    rows: usize,
    columns: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }

    fn screen_lines(&self) -> usize {
        self.rows
    }

    fn columns(&self) -> usize {
        self.columns
    }
}

impl From<Size> for PtySize {
    fn from(size: Size) -> Self {
        let clamp = |n: usize| u16::try_from(n).unwrap_or(u16::MAX).max(1);
        PtySize { rows: clamp(size.rows), columns: clamp(size.columns) }
    }
}

/// The emulator's events: answers it writes back to the program go to the
/// writer thread; requests wait for the main thread.
struct Listener {
    to_pty: mpsc::Sender<ToPty>,
    requests: Arc<Mutex<Requests>>,
    /// The grid's size, for programs that ask for it in pixels.
    size: Arc<Mutex<Size>>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        match event {
            Event::PtyWrite(text) => {
                let _ = self.to_pty.send(ToPty::Input(text.into_bytes()));
            }
            Event::Title(title) => self.requests.lock().unwrap().title = Some(Some(title)),
            Event::ResetTitle => self.requests.lock().unwrap().title = Some(None),
            Event::ClipboardStore(_, text) => self.requests.lock().unwrap().copy = Some(text),
            Event::TextAreaSizeRequest(answer) => {
                // Cells of Menlo 13 pt are about 8 × 16 px.
                let grid = PtySize::from(*self.size.lock().unwrap());
                let size = WindowSize {
                    num_lines: grid.rows,
                    num_cols: grid.columns,
                    cell_width: 8,
                    cell_height: 16,
                };
                let _ = self.to_pty.send(ToPty::Input(answer(size).into_bytes()));
            }
            _ => {}
        }
    }
}

/// Synchronized updates (DEC mode 2026) are applied as they arrive: the
/// reader blocks on the PTY, so it couldn't end one that never finishes.
#[derive(Default)]
struct Unsynchronized;

impl Timeout for Unsynchronized {
    fn set_timeout(&mut self, _: Duration) {}

    fn clear_timeout(&mut self) {}

    fn pending_timeout(&self) -> bool {
        false
    }
}

/// A started PTY on its way to the main thread. Dropped unused (the
/// project closed meanwhile), it hangs the program up.
struct Started(Option<Pty>);

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(pty) = &self.0 {
            let _ = pty.control.hang_up();
        }
    }
}

impl Tab {
    fn new(launch: Launch, size: Size, directory: PathBuf) -> Self {
        let name = match &launch {
            Launch::Shell => String::new(),
            Launch::PackageManager { name, args } => format!("{name} {}", args.join(" ")),
            Launch::Script { script } => script.clone(),
        };
        let screen = Screen {
            lines: blank_lines(size),
            cursor: None,
            history: 0,
            scrolled_back: 0,
            mode: TermMode::empty(),
        };
        Tab {
            launch,
            directory,
            process: Process::Waiting,
            generation: 0,
            screen: RefCell::new(screen),
            name,
            title: None,
            stopped: false,
            url: None,
        }
    }

    /// What the tab shows, copied from the emulator if it changed since the
    /// last copy.
    fn screen(&self) -> Ref<'_, Screen> {
        if let Process::Running(session) = &self.process
            // Cleared before the copy: output parsed after it wakes the
            // main thread again.
            && session.dirty.swap(false, Ordering::SeqCst)
        {
            let term = session.term.lock().unwrap();
            *self.screen.borrow_mut() = grid::snapshot(&term);
        }
        self.screen.borrow()
    }

    /// The emulator changed on the main thread (a resize, a scroll).
    fn touched(&self) {
        if let Process::Running(session) = &self.process {
            session.dirty.store(true, Ordering::SeqCst);
        }
    }

    /// The modes the running program has set.
    fn mode(&self) -> Option<TermMode> {
        let Process::Running(session) = &self.process else { return None };
        Some(*session.term.lock().unwrap().mode())
    }

    /// Clears the tab and waits to start its program again.
    fn restart(&mut self, size: Size) {
        self.process = Process::Waiting;
        let screen = self.screen.get_mut();
        screen.lines = blank_lines(size);
        screen.cursor = None;
        self.title = None;
        self.stopped = false;
        self.url = None;
    }

    fn has_ended(&self) -> bool {
        matches!(self.process, Process::Exited(_) | Process::Failed(_))
    }

    fn title(&self) -> String {
        self.title.clone().unwrap_or_else(|| self.name.clone())
    }

    fn status(&self) -> TerminalStatus {
        match &self.process {
            Process::Waiting | Process::Starting => TerminalStatus::Starting,
            Process::Running(_) => TerminalStatus::Running,
            Process::Exited(_) if self.stopped => TerminalStatus::Stopped,
            Process::Exited(exit) => TerminalStatus::Exited { code: exit.code },
            Process::Failed(message) => TerminalStatus::Failed(message.clone()),
        }
    }
}

impl Terminal {
    pub(crate) fn new(project: ProjectId, root: PathBuf) -> Self {
        Terminal {
            project,
            tabs: vec![Tab::new(Launch::Shell, DEFAULT_SIZE, root.clone())],
            root,
            size: DEFAULT_SIZE,
            active: 0,
            generation: 0,
            visible: true,
            focused: false,
            preedit: None,
        }
    }

    /// The showing tab; `None` once the last one is closed.
    fn tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    /// The showing tab's program's modes, if it runs.
    fn mode(&self) -> Option<TermMode> {
        self.tab().and_then(Tab::mode)
    }

    /// The tab whose session was started as `generation`.
    fn started_as(&mut self, generation: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|tab| tab.generation == generation)
    }

    /// Whether a tab waits for the project environment to start its
    /// program.
    pub(crate) fn is_waiting(&self) -> bool {
        self.tabs.iter().any(|tab| matches!(tab.process, Process::Waiting))
    }

    /// Runs the package manager in a tab of its own and shows it: a new
    /// tab, or the tab that ran the same command before, run again if it
    /// has ended. A tab still running it is just shown. It starts once the
    /// environment is ready (`start`).
    pub(crate) fn run_package_manager(&mut self, name: &str, args: &[&str]) {
        let launch = Launch::PackageManager { name: name.to_owned(), args: args.iter().map(|&a| a.to_owned()).collect() };
        let index = match self.tabs.iter().position(|tab| tab.launch == launch) {
            Some(index) => {
                if self.tabs[index].has_ended() {
                    self.tabs[index].restart(self.size);
                }
                index
            }
            None => {
                self.tabs.push(Tab::new(launch, self.size, self.root.clone()));
                self.tabs.len() - 1
            }
        };
        self.visible = true;
        self.select(index);
    }

    /// Runs a `package.json` script in a tab of its own named `title`, in
    /// `directory`, and shows it: like [`run_package_manager`], a tab that
    /// ran it before is reused.
    ///
    /// [`run_package_manager`]: Self::run_package_manager
    pub(crate) fn run_script(&mut self, directory: PathBuf, title: String, script: &str) {
        let launch = Launch::Script { script: script.to_owned() };
        let index = match self.tabs.iter().position(|tab| tab.launch == launch && tab.directory == directory) {
            Some(index) => {
                if self.tabs[index].has_ended() {
                    self.tabs[index].restart(self.size);
                }
                index
            }
            None => {
                let mut tab = Tab::new(launch, self.size, directory);
                tab.name = title;
                self.tabs.push(tab);
                self.tabs.len() - 1
            }
        };
        self.visible = true;
        self.select(index);
    }

    /// Whether a tab runs (or is about to run) the package manager with
    /// `args`.
    pub(crate) fn is_running_package_manager(&self, args: &[&str]) -> bool {
        self.tabs.iter().any(|tab| {
            matches!(&tab.launch, Launch::PackageManager { args: a, .. } if a.iter().map(String::as_str).eq(args.iter().copied()))
                && !tab.has_ended()
        })
    }

    /// Shows tab `index`, telling the programs that asked about the focus
    /// moving between them.
    fn select(&mut self, index: usize) {
        if index == self.active || index >= self.tabs.len() {
            return;
        }
        if self.focused {
            self.report_focus(b"\x1b[O");
        }
        self.active = index;
        self.preedit = None;
        if self.focused {
            self.report_focus(b"\x1b[I");
        }
    }

    /// Starts every waiting tab's program in the background with `env`, the
    /// project's environment. `package_manager` is the pinned package
    /// manager's executable, or why there is none.
    pub(crate) fn start(
        &mut self,
        env: ProcessEnv,
        package_manager: Result<PathBuf, String>,
        host: SharedHost,
        jobs: &Jobs,
    ) {
        let (id, size) = (self.project, self.size);
        for tab in self.tabs.iter_mut().filter(|tab| matches!(tab.process, Process::Waiting)) {
            self.generation += 1;
            tab.generation = self.generation;
            let spec = match &tab.launch {
                Launch::Shell => None,
                Launch::PackageManager { args, .. } => match &package_manager {
                    Ok(program) => Some(ProcessSpec::new(program).args(args)),
                    Err(reason) => {
                        tab.process = Process::Failed(reason.clone());
                        continue;
                    }
                },
                Launch::Script { script } => match &package_manager {
                    Ok(program) => Some(ProcessSpec::new(program).args(["run", script]).cwd(&tab.directory)),
                    Err(reason) => {
                        tab.process = Process::Failed(reason.clone());
                        continue;
                    }
                },
            };
            tab.process = Process::Starting;
            let (generation, env, host) = (self.generation, env.clone(), host.clone());
            jobs.spawn("start a terminal program", move || {
                let spec = spec.unwrap_or_else(|| shell(&env, host.as_ref()));
                let mut spec = spec.env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
                // Apps started from Finder have no locale; shells then mangle
                // anything but ASCII.
                if ["LANG", "LC_ALL", "LC_CTYPE"].iter().all(|key| env.var(key).is_none()) {
                    spec = spec.env("LANG", "en_US.UTF-8");
                }
                let spec = env.apply(spec);
                let program = spec.program.clone();
                let started = host.ptys().spawn(&spec, size.into()).map(|pty| Started(Some(pty)));
                Box::new(move |core: &mut Core| {
                    let jobs = core.jobs.clone();
                    if let Some(project) = core.project_mut(id) {
                        project.terminal.started(generation, program, started, &jobs);
                    }
                })
            });
        }
    }

    fn started(&mut self, generation: u64, program: PathBuf, started: io::Result<Started>, jobs: &Jobs) {
        let (project, size) = (self.project, self.size);
        let Some(tab) = self.started_as(generation) else { return };
        let pty = match started {
            Ok(mut started) => started.0.take().expect("taken once"),
            Err(error) => {
                tab.process = Process::Failed(match tab.launch {
                    Launch::Shell => format!("Couldn't start your shell ({}): {error}", program.display()),
                    Launch::PackageManager { .. } | Launch::Script { .. } => {
                        format!("Couldn't start {}: {error}", tab.name)
                    }
                });
                return;
            }
        };
        let Pty { output, input, control } = pty;
        let control: Arc<dyn PtyControl> = Arc::from(control);
        let (to_pty, from_core) = mpsc::channel();
        let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
        let requests = Arc::new(Mutex::new(Requests::default()));
        let shared_size = Arc::new(Mutex::new(size));
        let listener = Listener { to_pty: to_pty.clone(), requests: requests.clone(), size: shared_size.clone() };
        let term = Arc::new(Mutex::new(Term::new(config, &size, listener)));
        let dirty = Arc::new(AtomicBool::new(true));

        let writer_control = control.clone();
        std::thread::Builder::new()
            .name("genea: terminal input".into())
            .spawn(move || write_pty(input, writer_control.as_ref(), from_core))
            .expect("spawn the terminal's writer");
        let reader = Reader {
            urls: matches!(tab.launch, Launch::Script { .. }).then(UrlFinder::new),
            output,
            term: term.clone(),
            control: control.clone(),
            dirty: dirty.clone(),
            requests: requests.clone(),
            jobs: jobs.clone(),
            project,
            generation,
        };
        std::thread::Builder::new()
            .name("genea: terminal output".into())
            .spawn(move || reader.run())
            .expect("spawn the terminal's reader");
        if tab.launch == Launch::Shell {
            tab.name = program.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        }
        tab.title = None;
        tab.process = Process::Running(Session { term, to_pty, control, dirty, requests, size: shared_size });
    }

    /// New output was parsed: do what the program asked. The grid is
    /// copied when view state is next read.
    fn output_arrived(&mut self, generation: u64, host: &dyn Host) {
        let Some(tab) = self.started_as(generation) else { return };
        let Process::Running(session) = &tab.process else { return };
        let requests = std::mem::take(&mut *session.requests.lock().unwrap());
        if let Some(title) = requests.title {
            tab.title = title;
        }
        if let Some(text) = requests.copy {
            host.clipboard().write_text(&text);
        }
        if let Some(url) = requests.url {
            tab.url.get_or_insert(url);
        }
    }

    /// The program's side of the PTY closed: it exited.
    fn exited(&mut self, generation: u64, exit: Exit) {
        let Some(tab) = self.started_as(generation) else { return };
        // The last screen stays.
        drop(tab.screen());
        tab.process = Process::Exited(exit);
        tab.url = None;
        tab.screen.get_mut().cursor = None;
    }

    /// A terminal command from the user.
    pub(crate) fn command(&mut self, command: Command, host: &dyn Host) {
        match command {
            Command::ToggleTerminal => {
                if self.visible && self.focused {
                    self.visible = false;
                    self.unfocus();
                } else {
                    self.show();
                }
            }
            Command::FocusTerminal => self.show(),
            Command::SelectTerminalTab(index) => self.select(index),
            Command::NewTerminalTab => {
                self.tabs.push(Tab::new(Launch::Shell, self.size, self.root.clone()));
                self.select(self.tabs.len() - 1);
                self.show();
            }
            Command::CloseTerminalTab(index) if index < self.tabs.len() => self.close(index),
            Command::StopTerminalTab(index) => self.stop(index, host),
            Command::RerunTerminalTab(index) => self.rerun(index),
            Command::TerminalKey(TerminalKey::Enter, _)
                if self.tab().is_some_and(|tab| tab.launch == Launch::Shell && tab.has_ended()) =>
            {
                let size = self.size;
                self.tabs[self.active].restart(size)
            }
            Command::TerminalPaste => {
                if let (Some(text), Some(mode)) = (host.clipboard().read_text(), self.mode()) {
                    self.send(input::paste(&text, mode));
                }
            }
            Command::SetTerminalSize { rows, columns } => {
                self.resize(Size { rows: rows.max(1), columns: columns.max(2) })
            }
            Command::TerminalText(text) => self.send(text.into_bytes()),
            Command::TerminalPreedit(text) => self.preedit = Some(text).filter(|text| !text.is_empty()),
            Command::ScrollTerminal { rows, line, column } => self.scroll(rows, line, column),
            Command::TerminalMouse { action, line, column, modifiers } => {
                if let Some(mode) = self.mode() {
                    self.send(input::mouse(action, line, column, modifiers, mode));
                }
            }
            Command::TerminalKey(key, modifiers) => {
                if let Some(mode) = self.mode() {
                    self.send(input::key(key, modifiers, mode));
                }
            }
            _ => {}
        }
    }

    /// Stops a command's tab: interrupts its program, and kills it if it
    /// hasn't ended after [`SCRIPT_STOP_TIMEOUT`]. A program not started
    /// yet never starts.
    fn stop(&mut self, index: usize, host: &dyn Host) {
        let Some(tab) = self.tabs.get_mut(index).filter(|tab| tab.launch != Launch::Shell) else { return };
        match &tab.process {
            Process::Running(session) => {
                tab.stopped = true;
                let _ = session.control.interrupt();
                let control = session.control.clone();
                host.clock().after(
                    SCRIPT_STOP_TIMEOUT,
                    Box::new(move || {
                        // Does nothing once it has ended.
                        let _ = control.kill();
                    }),
                );
            }
            Process::Waiting => {
                tab.stopped = true;
                tab.process = Process::Exited(Exit { code: None, signal: None });
            }
            // A start in flight: it is started as a new generation, so
            // forgetting it drops its PTY (and hangs it up) when it lands.
            Process::Starting => {
                tab.generation = 0;
                tab.stopped = true;
                tab.process = Process::Exited(Exit { code: None, signal: None });
            }
            Process::Exited(_) | Process::Failed(_) => {}
        }
    }

    /// Runs a command's tab again: a running program is hung up (its
    /// session dropped) and replaced.
    fn rerun(&mut self, index: usize) {
        let size = self.size;
        let Some(tab) = self.tabs.get_mut(index).filter(|tab| tab.launch != Launch::Shell) else { return };
        tab.restart(size);
        // Results of the replaced program no longer find the tab.
        tab.generation = 0;
        self.visible = true;
        self.select(index);
    }

    /// Shows the pane and focuses it, with a new shell if it has no tabs.
    fn show(&mut self) {
        if self.tabs.is_empty() {
            self.tabs.push(Tab::new(Launch::Shell, self.size, self.root.clone()));
            self.active = 0;
        }
        self.visible = true;
        self.focus();
    }

    /// Closes tab `index`; dropping its session hangs up its program. The
    /// tab to its right shows, else the one to its left; closing the last
    /// collapses the pane.
    fn close(&mut self, index: usize) {
        let closing_active = index == self.active;
        if closing_active && self.focused {
            self.report_focus(b"\x1b[O");
        }
        self.tabs.remove(index);
        if self.tabs.is_empty() {
            self.active = 0;
            self.visible = false;
            self.focused = false;
            self.preedit = None;
            return;
        }
        if index < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        if closing_active {
            self.preedit = None;
            if self.focused {
                self.report_focus(b"\x1b[I");
            }
        }
    }

    /// The file and place the reference at the showing tab's cell points
    /// to: relative to the project root if it is inside it, else absolute.
    pub(crate) fn file_link_at(&self, line: usize, column: usize) -> Option<(PathBuf, TextPosition)> {
        let tab = self.tab()?;
        let screen = tab.screen();
        let link = screen.lines.get(line)?.links.iter().find(|link| link.columns.contains(&column))?;
        let path = normalize(&tab.directory.join(&link.path));
        let path = path.strip_prefix(&self.root).map(Path::to_path_buf).unwrap_or(path);
        Some((path, link.at))
    }

    /// Whether the terminal has the keyboard focus rather than the editor.
    pub(crate) fn is_focused(&self) -> bool {
        self.focused
    }

    /// The editor takes the keyboard focus.
    pub(crate) fn unfocus(&mut self) {
        if std::mem::replace(&mut self.focused, false) {
            self.report_focus(b"\x1b[O");
        }
    }

    fn focus(&mut self) {
        if !std::mem::replace(&mut self.focused, true) {
            self.report_focus(b"\x1b[I");
        }
    }

    /// Tells the showing tab's program, if it asked (focus reporting),
    /// about a focus change.
    fn report_focus(&mut self, report: &[u8]) {
        if self.mode().is_some_and(|mode| mode.contains(TermMode::FOCUS_IN_OUT)) {
            self.send(report.to_vec());
        }
    }

    fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        for tab in &mut self.tabs {
            match &tab.process {
                Process::Running(session) => {
                    *session.size.lock().unwrap() = size;
                    session.term.lock().unwrap().resize(size);
                    let _ = session.to_pty.send(ToPty::Resize(size.into()));
                    tab.touched();
                }
                _ => tab.screen.get_mut().lines.resize_with(size.rows, blank_line),
            }
        }
    }

    /// The scroll wheel: the program's if it takes it, else the scrollback.
    fn scroll(&mut self, rows: i32, line: usize, column: usize) {
        let Some(mode) = self.mode() else { return };
        if rows == 0 {
            return;
        }
        let up = rows < 0;
        let count = rows.unsigned_abs() as usize;
        if mode.intersects(TermMode::MOUSE_MODE) {
            self.send(input::wheel(up, line, column, mode).repeat(count));
        } else if mode.contains(TermMode::ALT_SCREEN | TermMode::ALTERNATE_SCROLL) {
            let key = if up { TerminalKey::Up } else { TerminalKey::Down };
            self.send(input::key(key, Modifiers::default(), mode).repeat(count));
        } else if let Some(tab) = self.tab()
            && let Process::Running(session) = &tab.process
        {
            session.term.lock().unwrap().scroll_display(Scroll::Delta(-rows));
            tab.touched();
        }
    }

    /// Sends input to the showing tab's program, scrolling back to the
    /// bottom first.
    fn send(&mut self, bytes: Vec<u8>) {
        let Some(tab) = self.tab() else { return };
        let Process::Running(session) = &tab.process else { return };
        if bytes.is_empty() {
            return;
        }
        let _ = session.to_pty.send(ToPty::Input(bytes));
        if tab.screen().scrolled_back > 0 {
            session.term.lock().unwrap().scroll_display(Scroll::Bottom);
            tab.touched();
        }
    }

    pub(crate) fn view(&self) -> TerminalView {
        let tab = self.tab();
        let blank;
        let screen = match tab {
            Some(tab) => tab.screen(),
            None => {
                blank = RefCell::new(Screen {
                    lines: blank_lines(self.size),
                    cursor: None,
                    history: 0,
                    scrolled_back: 0,
                    mode: TermMode::empty(),
                });
                blank.borrow()
            }
        };
        TerminalView {
            visible: self.visible,
            focused: self.focused,
            status: tab.map_or(TerminalStatus::Starting, Tab::status),
            title: tab.map(Tab::title).unwrap_or_default(),
            tabs: self
                .tabs
                .iter()
                .map(|tab| TerminalTab { title: tab.title(), status: tab.status(), shell: tab.launch == Launch::Shell })
                .collect(),
            active_tab: self.active,
            rows: self.size.rows,
            columns: self.size.columns,
            lines: screen.lines.clone(),
            cursor: screen.cursor,
            history: screen.history,
            scrolled_back: screen.scrolled_back,
            alternate_screen: screen.mode.contains(TermMode::ALT_SCREEN),
            mouse_reporting: screen.mode.intersects(TermMode::MOUSE_MODE),
            preedit: self.preedit.clone(),
            url: tab.and_then(|tab| tab.url.clone()),
        }
    }

    /// The running scripts' links, in tab order.
    pub(crate) fn script_links(&self) -> Vec<ScriptLink> {
        let links = self.tabs.iter().enumerate().filter(|(_, tab)| matches!(tab.process, Process::Running(_)));
        links
            .filter_map(|(index, tab)| Some(ScriptLink { tab: index, title: tab.title(), url: tab.url.clone()? }))
            .collect()
    }
}

/// The user's login shell: the login shell's own `SHELL`, else the one
/// Genea was started with, else macOS's default.
fn shell(env: &ProcessEnv, host: &dyn Host) -> ProcessSpec {
    let launch = host.launch_environment();
    let launch_shell = launch.iter().rev().find(|(key, _)| key == "SHELL").map(|(_, value)| value.as_os_str());
    let shell = env.var("SHELL").or(launch_shell).filter(|s| !s.is_empty()).unwrap_or(OsStr::new(DEFAULT_SHELL));
    ProcessSpec::new(shell).arg("-l")
}


/// The reader thread: PTY output into the emulator, then an Apply.
struct Reader {
    /// Looks for a script's link; `None` for other tabs, or once found.
    urls: Option<UrlFinder>,
    output: Box<dyn Read + Send>,
    term: Arc<Mutex<Term<Listener>>>,
    control: Arc<dyn PtyControl>,
    dirty: Arc<AtomicBool>,
    requests: Arc<Mutex<Requests>>,
    jobs: Jobs,
    project: ProjectId,
    generation: u64,
}

impl Reader {
    fn run(mut self) {
        let mut processor: Processor<Unsynchronized> = Processor::new();
        let mut buffer = vec![0; 64 * 1024];
        let (id, generation) = (self.project, self.generation);
        loop {
            let n = match self.output.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                // macOS answers EIO once the shell's side is closed.
                Err(_) => break,
            };
            for chunk in buffer[..n].chunks(PARSE_CHUNK) {
                processor.advance(&mut *self.term.lock().unwrap(), chunk);
            }
            if let Some(urls) = &mut self.urls
                && let Some(url) = urls.advance(&buffer[..n])
            {
                self.requests.lock().unwrap().url = Some(url);
                self.urls = None;
            }
            // Wake the main thread if it has taken the last copy, or if the
            // program asked for something.
            let asked = self.requests.lock().unwrap().is_asked();
            if !self.dirty.swap(true, Ordering::SeqCst) || asked {
                self.jobs.busy().finish(Box::new(move |core| {
                    let host = core.host.clone();
                    if let Some(project) = core.project_mut(id) {
                        project.terminal.output_arrived(generation, host.as_ref());
                    }
                }));
            }
        }
        let exit = self.control.wait().unwrap_or(Exit { code: None, signal: None });
        self.jobs.busy().finish(Box::new(move |core| {
            if let Some(project) = core.project_mut(id) {
                project.terminal.exited(generation, exit);
            }
        }));
    }
}

/// The writer thread: typed input and resizes, in order.
fn write_pty(mut input: Box<dyn Write + Send>, control: &dyn PtyControl, from_core: mpsc::Receiver<ToPty>) {
    for message in from_core {
        let written = match message {
            ToPty::Input(bytes) => input.write_all(&bytes).and_then(|()| input.flush()),
            ToPty::Resize(size) => control.resize(size),
        };
        if written.is_err() {
            return;
        }
    }
}

fn blank_lines(size: Size) -> Vec<TerminalLine> {
    (0..size.rows).map(|_| blank_line()).collect()
}

fn blank_line() -> TerminalLine {
    TerminalLine { text: String::new(), runs: Vec::new(), links: Vec::new() }
}

/// `path` with `.` and `..` worked out, without touching the disk.
fn normalize(path: &Path) -> PathBuf {
    let mut normal = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normal.pop();
            }
            component => normal.push(component),
        }
    }
    normal
}
