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
    event::{Event, EventListener},
    grid::Dimensions,
    term::{
        Config,
        cell::Flags,
    },
    vte::ansi::{CursorShape, Processor, Timeout},
};
use genea_host::{Exit, ProcessSpec, Pty, PtyControl, PtySize, SharedHost};

use crate::{
    environment::ProcessEnv,
    jobs::Jobs,
    view::{TerminalCursor, TerminalLine, TerminalStatus, TerminalView},
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
    /// The visible grid, as last copied from the session.
    lines: Vec<TerminalLine>,
    cursor: Option<TerminalCursor>,
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
/// writer thread.
struct Listener {
    to_pty: mpsc::Sender<ToPty>,
}

impl EventListener for Listener {
    fn send_event(&self, event: Event) {
        if let Event::PtyWrite(text) = event {
            let _ = self.to_pty.send(ToPty::Input(text.into_bytes()));
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
        let lines = blank_lines(DEFAULT_SIZE);
        Terminal { project, size: DEFAULT_SIZE, shell: Shell::Waiting, generation: 0, lines, cursor: None }
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
        let term = Arc::new(Mutex::new(Term::new(config, &self.size, Listener { to_pty: to_pty.clone() })));
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
        self.shell = Shell::Running(Session { term, to_pty, control, dirty });
        self.refresh();
    }

    /// New output was parsed: copy the visible grid.
    fn output_arrived(&mut self, generation: u64) {
        if generation == self.generation {
            self.refresh();
        }
    }

    /// The shell's side of the PTY closed: it exited.
    fn exited(&mut self, generation: u64, exit: Exit) {
        if generation != self.generation {
            return;
        }
        self.refresh();
        self.shell = Shell::Exited(exit);
        self.cursor = None;
    }

    /// Copies the visible grid from the session.
    fn refresh(&mut self) {
        let Shell::Running(session) = &self.shell else { return };
        // Cleared before the copy: output parsed after it posts again.
        session.dirty.store(false, Ordering::SeqCst);
        let term = session.term.lock().unwrap();
        (self.lines, self.cursor) = snapshot(&term);
    }

    pub(crate) fn view(&self) -> TerminalView {
        let status = match &self.shell {
            Shell::Waiting | Shell::Starting => TerminalStatus::Starting,
            Shell::Running(_) => TerminalStatus::Running,
            Shell::Exited(exit) => TerminalStatus::Exited { code: exit.code },
            Shell::Failed(message) => TerminalStatus::Failed(message.clone()),
        };
        TerminalView {
            status,
            rows: self.size.rows,
            columns: self.size.columns,
            lines: self.lines.clone(),
            cursor: self.cursor,
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
                    if let Some(project) = core.project_mut(id) {
                        project.terminal.output_arrived(generation);
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
    (0..size.rows).map(|_| TerminalLine { text: String::new() }).collect()
}

/// The visible rows and the cursor, as the user sees them.
fn snapshot(term: &Term<Listener>) -> (Vec<TerminalLine>, Option<TerminalCursor>) {
    let content = term.renderable_content();
    let offset = content.display_offset as i32;
    let rows = term.screen_lines();
    let mut texts: Vec<String> = vec![String::new(); rows];
    let mut widths = vec![0usize; rows];
    for indexed in content.display_iter {
        let Ok(row) = usize::try_from(indexed.point.line.0 + offset) else { continue };
        let Some(text) = texts.get_mut(row) else { continue };
        let cell = indexed.cell;
        if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
            continue;
        }
        let column = indexed.point.column.0;
        // Pad to the cell's column (cells after a wide char's spacer).
        while widths[row] < column {
            text.push(' ');
            widths[row] += 1;
        }
        let c = if cell.flags.intersects(Flags::HIDDEN | Flags::LEADING_WIDE_CHAR_SPACER) { ' ' } else { cell.c };
        text.push(c);
        if let Some(zero_width) = cell.zerowidth() {
            text.extend(zero_width);
        }
        widths[row] += if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };
    }
    let lines = texts
        .into_iter()
        .map(|text| TerminalLine { text: text.trim_end_matches([' ', '\t']).to_owned() })
        .collect();
    let cursor = content.cursor;
    let line = cursor.point.line.0 + offset;
    let cursor = (cursor.shape != CursorShape::Hidden && (0..rows as i32).contains(&line))
        .then(|| TerminalCursor { line: line as usize, column: cursor.point.column.0 });
    (lines, cursor)
}
