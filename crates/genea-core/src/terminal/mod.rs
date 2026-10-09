//! The terminal (ticket #38): one shell per project, on a PTY from the
//! host, emulated by `alacritty_terminal`.
//!
//! The shell is the user's `$SHELL` as a login shell, in the project root,
//! with the project environment and `TERM=xterm-256color`. It starts once
//! that environment is final: the login-shell capture has landed and the
//! toolchain has settled (so the pinned tools are on its PATH).
//!
//! Threads (spec #19, Threading): a reader thread reads the PTY and parses
//! the output into the [`Term`] under a lock; a writer thread writes typed
//! input and resizes the PTY. Neither the reads, the writes nor the parsing
//! happen on the main thread. The reader posts an Apply only when none is
//! pending, and that Apply copies the visible grid into view state, so a
//! flood of output is copied at most once per main-thread turn.

use std::{
    ffi::OsStr,
    io::{self, Read, Write},
    path::PathBuf,
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

use grid::Screen;

use crate::{
    command::{Command, Modifiers, TerminalKey},
    environment::ProcessEnv,
    jobs::Jobs,
    view::{TerminalLine, TerminalStatus, TerminalView},
    workbench::{Core, ProjectId},
};

/// Lines of scrollback (spec #19).
const SCROLLBACK: usize = 10_000;

/// The size until the view reports one.
const DEFAULT_SIZE: Size = Size { rows: 24, columns: 80 };

/// The reader parses at most this much per hold of the grid's lock, so the
/// main thread's copy never waits long behind a flood.
const PARSE_CHUNK: usize = 16 * 1024;

/// The shell when the environment names none: macOS's default.
const DEFAULT_SHELL: &str = "/bin/zsh";

pub(crate) struct Terminal {
    project: ProjectId,
    size: Size,
    shell: Shell,
    /// Bumped by every start, so a stale session's results are dropped.
    generation: u64,
    /// What the terminal showed when last copied from the session.
    screen: Screen,
    /// The running shell's file name, the title until the program sets one.
    shell_name: String,
    /// The title the program set.
    title: Option<String>,
    /// The pane is showing.
    visible: bool,
    /// The pane has the keyboard focus.
    focused: bool,
    /// The IME composition being typed.
    preedit: Option<String>,
}

enum Shell {
    /// Waiting for the project environment.
    Waiting,
    Starting,
    Running(Session),
    Exited(Exit),
    Failed(String),
}

/// A running shell: its emulator, and the threads around its PTY.
struct Session {
    term: Arc<Mutex<Term<Listener>>>,
    /// To the writer thread.
    to_pty: mpsc::Sender<ToPty>,
    control: Arc<dyn PtyControl>,
    /// An Apply with new output is pending.
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
}

impl Drop for Session {
    /// Closing the terminal (or the project) hangs up the shell.
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
/// project closed meanwhile), it hangs the shell up.
struct Started(Option<Pty>);

impl Drop for Started {
    fn drop(&mut self) {
        if let Some(pty) = &self.0 {
            let _ = pty.control.hang_up();
        }
    }
}

impl Terminal {
    pub(crate) fn new(project: ProjectId) -> Self {
        let screen = Screen {
            lines: blank_lines(DEFAULT_SIZE),
            cursor: None,
            history: 0,
            scrolled_back: 0,
            mode: TermMode::empty(),
        };
        Terminal {
            project,
            size: DEFAULT_SIZE,
            shell: Shell::Waiting,
            generation: 0,
            screen,
            shell_name: String::new(),
            title: None,
            visible: true,
            focused: false,
            preedit: None,
        }
    }

    /// Whether the terminal waits for the project environment to start its
    /// shell.
    pub(crate) fn is_waiting(&self) -> bool {
        matches!(self.shell, Shell::Waiting)
    }

    /// Starts the shell in the background with `env`, the project's
    /// environment.
    pub(crate) fn start(&mut self, env: ProcessEnv, host: SharedHost, jobs: &Jobs) {
        self.generation += 1;
        self.shell = Shell::Starting;
        let (generation, id, size) = (self.generation, self.project, self.size);
        jobs.spawn("start the terminal's shell", move || {
            // The login shell's own SHELL, else the one it was started as.
            let launch = host.launch_environment();
            let launch_shell = launch.iter().rev().find(|(key, _)| key == "SHELL").map(|(_, value)| value.as_os_str());
            let shell = env.var("SHELL").or(launch_shell).filter(|s| !s.is_empty()).unwrap_or(OsStr::new(DEFAULT_SHELL));
            let shell = PathBuf::from(shell);
            let mut spec = ProcessSpec::new(&shell).arg("-l").env("TERM", "xterm-256color").env("COLORTERM", "truecolor");
            // Apps started from Finder have no locale; shells then mangle
            // anything but ASCII.
            if ["LANG", "LC_ALL", "LC_CTYPE"].iter().all(|key| env.var(key).is_none()) {
                spec = spec.env("LANG", "en_US.UTF-8");
            }
            let spec = env.apply(spec);
            let started = host.ptys().spawn(&spec, size.into()).map(|pty| Started(Some(pty)));
            Box::new(move |core: &mut Core| {
                let jobs = core.jobs.clone();
                if let Some(project) = core.project_mut(id) {
                    project.terminal.started(generation, shell, started, &jobs);
                }
            })
        });
    }

    fn started(&mut self, generation: u64, shell: PathBuf, started: io::Result<Started>, jobs: &Jobs) {
        if generation != self.generation {
            return;
        }
        let pty = match started {
            Ok(mut started) => started.0.take().expect("taken once"),
            Err(error) => {
                self.shell = Shell::Failed(format!("Couldn't start your shell ({}): {error}", shell.display()));
                return;
            }
        };
        let Pty { output, input, control } = pty;
        let control: Arc<dyn PtyControl> = Arc::from(control);
        let (to_pty, from_core) = mpsc::channel();
        let config = Config { scrolling_history: SCROLLBACK, ..Config::default() };
        let requests = Arc::new(Mutex::new(Requests::default()));
        let size = Arc::new(Mutex::new(self.size));
        let listener = Listener { to_pty: to_pty.clone(), requests: requests.clone(), size: size.clone() };
        let term = Arc::new(Mutex::new(Term::new(config, &self.size, listener)));
        let dirty = Arc::new(AtomicBool::new(false));

        let writer_control = control.clone();
        std::thread::Builder::new()
            .name("genea: terminal input".into())
            .spawn(move || write_pty(input, writer_control.as_ref(), from_core))
            .expect("spawn the terminal's writer");
        let reader = Reader {
            output,
            term: term.clone(),
            control: control.clone(),
            dirty: dirty.clone(),
            jobs: jobs.clone(),
            project: self.project,
            generation,
        };
        std::thread::Builder::new()
            .name("genea: terminal output".into())
            .spawn(move || reader.run())
            .expect("spawn the terminal's reader");
        self.shell_name = shell.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
        self.title = None;
        self.shell = Shell::Running(Session { term, to_pty, control, dirty, requests, size });
        self.refresh();
    }

    /// New output was parsed: copy the visible grid, and do what the
    /// program asked.
    fn output_arrived(&mut self, generation: u64, host: &dyn Host) {
        if generation != self.generation {
            return;
        }
        self.refresh();
        let Shell::Running(session) = &self.shell else { return };
        let requests = std::mem::take(&mut *session.requests.lock().unwrap());
        if let Some(title) = requests.title {
            self.title = title;
        }
        if let Some(text) = requests.copy {
            host.clipboard().write_text(&text);
        }
    }

    /// The shell's side of the PTY closed: it exited.
    fn exited(&mut self, generation: u64, exit: Exit) {
        if generation != self.generation {
            return;
        }
        self.refresh();
        self.shell = Shell::Exited(exit);
        self.screen.cursor = None;
    }

    /// Copies the visible grid from the session.
    fn refresh(&mut self) {
        let Shell::Running(session) = &self.shell else { return };
        // Cleared before the copy: output parsed after it posts again.
        session.dirty.store(false, Ordering::SeqCst);
        let term = session.term.lock().unwrap();
        self.screen = grid::snapshot(&term);
    }

    /// A terminal command from the user.
    pub(crate) fn command(&mut self, command: Command, host: &dyn Host) {
        match command {
            Command::ToggleTerminal => {
                if self.visible && self.focused {
                    self.visible = false;
                    self.unfocus();
                } else {
                    self.visible = true;
                    self.focus();
                }
            }
            Command::FocusTerminal => {
                self.visible = true;
                self.focus();
            }
            Command::TerminalKey(TerminalKey::Enter, _) if matches!(self.shell, Shell::Exited(_) | Shell::Failed(_)) => {
                self.restart()
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

    /// Tells a program that asked (focus reporting) about a focus change.
    fn report_focus(&mut self, report: &[u8]) {
        if self.mode().is_some_and(|mode| mode.contains(TermMode::FOCUS_IN_OUT)) {
            self.send(report.to_vec());
        }
    }

    /// Clears the pane and waits to start a new shell.
    fn restart(&mut self) {
        self.shell = Shell::Waiting;
        self.screen.lines = blank_lines(self.size);
        self.screen.cursor = None;
        self.title = None;
    }

    /// The modes the running program has set.
    fn mode(&self) -> Option<TermMode> {
        let Shell::Running(session) = &self.shell else { return None };
        Some(*session.term.lock().unwrap().mode())
    }

    fn resize(&mut self, size: Size) {
        if size == self.size {
            return;
        }
        self.size = size;
        match &self.shell {
            Shell::Running(session) => {
                *session.size.lock().unwrap() = size;
                session.term.lock().unwrap().resize(size);
                let _ = session.to_pty.send(ToPty::Resize(size.into()));
                self.refresh();
            }
            _ => self.screen.lines.resize_with(size.rows, blank_line),
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
        } else if let Shell::Running(session) = &self.shell {
            session.term.lock().unwrap().scroll_display(Scroll::Delta(-rows));
            self.refresh();
        }
    }

    /// Sends input to the program, scrolling back to the bottom first.
    fn send(&mut self, bytes: Vec<u8>) {
        let Shell::Running(session) = &self.shell else { return };
        if bytes.is_empty() {
            return;
        }
        let _ = session.to_pty.send(ToPty::Input(bytes));
        if self.screen.scrolled_back > 0 {
            session.term.lock().unwrap().scroll_display(Scroll::Bottom);
            self.refresh();
        }
    }

    pub(crate) fn view(&self) -> TerminalView {
        let status = match &self.shell {
            Shell::Waiting | Shell::Starting => TerminalStatus::Starting,
            Shell::Running(_) => TerminalStatus::Running,
            Shell::Exited(exit) => TerminalStatus::Exited { code: exit.code },
            Shell::Failed(message) => TerminalStatus::Failed(message.clone()),
        };
        let screen = &self.screen;
        TerminalView {
            visible: self.visible,
            focused: self.focused,
            status,
            title: self.title.clone().unwrap_or_else(|| self.shell_name.clone()),
            rows: self.size.rows,
            columns: self.size.columns,
            lines: screen.lines.clone(),
            cursor: screen.cursor,
            history: screen.history,
            scrolled_back: screen.scrolled_back,
            alternate_screen: screen.mode.contains(TermMode::ALT_SCREEN),
            mouse_reporting: screen.mode.intersects(TermMode::MOUSE_MODE),
            preedit: self.preedit.clone(),
        }
    }
}

/// The reader thread: PTY output into the emulator, then an Apply.
struct Reader {
    output: Box<dyn Read + Send>,
    term: Arc<Mutex<Term<Listener>>>,
    control: Arc<dyn PtyControl>,
    dirty: Arc<AtomicBool>,
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
            if !self.dirty.swap(true, Ordering::SeqCst) {
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
    TerminalLine { text: String::new(), runs: Vec::new() }
}
