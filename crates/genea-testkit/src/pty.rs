//! The fake PTY (ticket #38): terminal shells the test plays by hand.
//!
//! Every spawn succeeds and gives the test a [`FakePty`]: it replays
//! scripted output with [`FakePty::output`], which returns only once Genea
//! has read all of it, so a `settle()` right after sees it on the grid. What
//! Genea types arrives in [`FakePty::input`].

use std::{
    collections::VecDeque,
    io::{self, Read, Write},
    sync::{Arc, Condvar, Mutex, MutexGuard},
    time::Duration,
};

use genea_host::{Exit, ProcessSpec, Pty, PtyControl, PtySize, Ptys};

/// How long the test-side waits give Genea. Real time: it guards against
/// hangs, it isn't a delay.
const WAIT: Duration = Duration::from_secs(10);

const SIGHUP: i32 = 1;
const SIGKILL: i32 = 9;

/// Pseudo-terminals the test plays. Each spawn is recorded as a
/// [`FakePty`], in order.
#[derive(Default)]
pub struct ScriptedPtys {
    spawned: Mutex<Vec<FakePty>>,
    failure: Mutex<Option<io::ErrorKind>>,
    changed: Condvar,
}

impl ScriptedPtys {
    /// Every terminal started so far, oldest first.
    pub fn spawned(&self) -> Vec<FakePty> {
        self.spawned.lock().unwrap().clone()
    }

    /// The most recent terminal. Panics if none has started.
    pub fn last(&self) -> FakePty {
        self.spawned().pop().expect("no terminal has started")
    }

    /// Waits until `count` terminals have started, and returns the last.
    pub fn wait_for_spawn(&self, count: usize) -> FakePty {
        let spawned = self.spawned.lock().unwrap();
        let (spawned, _) = self.changed.wait_timeout_while(spawned, WAIT, |s| s.len() < count).unwrap();
        assert!(spawned.len() >= count, "only {} terminals started, not {count}", spawned.len());
        spawned[count - 1].clone()
    }

    /// Makes every spawn from now on fail with `kind`, like a missing shell.
    pub fn fail(&self, kind: io::ErrorKind) {
        *self.failure.lock().unwrap() = Some(kind);
    }
}

impl Ptys for ScriptedPtys {
    fn spawn(&self, spec: &ProcessSpec, size: PtySize) -> io::Result<Pty> {
        if let Some(kind) = *self.failure.lock().unwrap() {
            return Err(io::Error::new(kind, format!("can't start {}", spec.program.display())));
        }
        let shared = Arc::new(Shared {
            spec: spec.clone(),
            state: Mutex::new(State {
                output: VecDeque::new(),
                reading: false,
                read_once: false,
                reader_gone: false,
                input: Vec::new(),
                size,
                exit: None,
                hung_up: false,
                interrupted: false,
                killed: false,
            }),
            changed: Condvar::new(),
        });
        self.spawned.lock().unwrap().push(FakePty { shared: shared.clone() });
        self.changed.notify_all();
        Ok(Pty {
            output: Box::new(Output(shared.clone())),
            input: Box::new(Input(shared.clone())),
            control: Box::new(Control(shared)),
        })
    }
}

/// One terminal's program, played by the test.
#[derive(Clone)]
pub struct FakePty {
    shared: Arc<Shared>,
}

struct Shared {
    spec: ProcessSpec,
    state: Mutex<State>,
    changed: Condvar,
}

struct State {
    /// Written by the test, not yet read by Genea.
    output: VecDeque<u8>,
    /// Genea is blocked reading, with nothing left to read.
    reading: bool,
    /// Genea has started reading the output.
    read_once: bool,
    /// Genea dropped its output reader.
    reader_gone: bool,
    /// Everything Genea typed.
    input: Vec<u8>,
    size: PtySize,
    exit: Option<Exit>,
    hung_up: bool,
    /// Genea interrupted the program (SIGINT); it plays on until the test
    /// makes it exit.
    interrupted: bool,
    /// Genea killed the program (SIGKILL), which ended it.
    killed: bool,
}

impl FakePty {
    /// What Genea started: program, arguments, folder and environment.
    pub fn spec(&self) -> ProcessSpec {
        self.shared.spec.clone()
    }

    /// The terminal's current size.
    pub fn size(&self) -> PtySize {
        self.lock().size
    }

    /// Plays output from the program. Returns once Genea has read all of it
    /// and is waiting for more (or has stopped reading).
    pub fn output(&self, bytes: impl AsRef<[u8]>) {
        let mut state = self.lock();
        state.output.extend(bytes.as_ref());
        state.reading = false;
        self.shared.changed.notify_all();
        let (state, _) = self
            .shared
            .changed
            .wait_timeout_while(state, WAIT, |s| !(s.reader_gone || s.hung_up || (s.output.is_empty() && s.reading)))
            .unwrap();
        assert!(state.output.is_empty() || state.reader_gone || state.hung_up, "Genea didn't read the terminal output");
    }

    /// Everything typed into the terminal so far, as text.
    pub fn input(&self) -> String {
        String::from_utf8_lossy(&self.lock().input).into_owned()
    }

    /// Waits until what was typed contains `text`, and returns all of it.
    pub fn wait_for_input(&self, text: &str) -> String {
        let state = self.lock();
        let (state, _) = self
            .shared
            .changed
            .wait_timeout_while(state, WAIT, |s| !String::from_utf8_lossy(&s.input).contains(text))
            .unwrap();
        let input = String::from_utf8_lossy(&state.input).into_owned();
        assert!(input.contains(text), "{text:?} wasn't typed into the terminal; it got {input:?}");
        input
    }

    /// Waits until the terminal has been resized to `size`.
    pub fn wait_for_size(&self, size: PtySize) {
        let state = self.lock();
        let (state, _) = self.shared.changed.wait_timeout_while(state, WAIT, |s| s.size != size).unwrap();
        assert_eq!(state.size, size, "the terminal wasn't resized");
    }

    /// The program exits with `code`: once its output is read, Genea sees
    /// the terminal close.
    ///
    /// If Genea is reading the output, it returns once Genea has seen the
    /// end, so a `settle()` right after shows the exit.
    pub fn exit(&self, code: i32) {
        let mut state = self.lock();
        state.exit.get_or_insert(Exit::code(code));
        self.shared.changed.notify_all();
        drop(state);
        self.wait_for_end_seen();
    }

    /// Waits until Genea's reader has seen the program end (if it was
    /// reading): Genea has then queued what the end changes.
    fn wait_for_end_seen(&self) {
        let state = self.lock();
        if state.read_once {
            let _ = self.shared.changed.wait_timeout_while(state, WAIT, |s| !s.reader_gone).unwrap();
        }
    }

    /// Whether Genea has hung up the terminal (closed it).
    pub fn hung_up(&self) -> bool {
        self.lock().hung_up
    }

    /// Waits until Genea hangs up the terminal.
    pub fn wait_for_hang_up(&self) {
        let state = self.lock();
        let (state, _) = self.shared.changed.wait_timeout_while(state, WAIT, |s| !s.hung_up).unwrap();
        assert!(state.hung_up, "the terminal wasn't hung up");
    }

    /// Whether Genea has interrupted the program (SIGINT). The program
    /// plays on: the test decides whether it exits.
    pub fn interrupted(&self) -> bool {
        self.lock().interrupted
    }

    /// Waits until Genea interrupts the program.
    pub fn wait_for_interrupt(&self) {
        let state = self.lock();
        let (state, _) = self.shared.changed.wait_timeout_while(state, WAIT, |s| !s.interrupted).unwrap();
        assert!(state.interrupted, "the program wasn't interrupted");
    }

    /// Whether Genea has killed the program (SIGKILL), which ends it.
    pub fn killed(&self) -> bool {
        self.lock().killed
    }

    /// Waits until Genea kills the program and has seen it end.
    pub fn wait_for_kill(&self) {
        let state = self.lock();
        let (state, _) = self.shared.changed.wait_timeout_while(state, WAIT, |s| !s.killed).unwrap();
        assert!(state.killed, "the program wasn't killed");
        drop(state);
        self.wait_for_end_seen();
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap()
    }
}

/// Genea's side of the output.
struct Output(Arc<Shared>);

impl Read for Output {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut state = self.0.state.lock().unwrap();
        state.read_once = true;
        loop {
            if !state.output.is_empty() {
                let n = buffer.len().min(state.output.len());
                for (slot, byte) in buffer.iter_mut().zip(state.output.drain(..n)) {
                    *slot = byte;
                }
                return Ok(n);
            }
            if state.exit.is_some() || state.hung_up {
                return Ok(0);
            }
            state.reading = true;
            self.0.changed.notify_all();
            state = self.0.changed.wait(state).unwrap();
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.0.state.lock().unwrap().reader_gone = true;
        self.0.changed.notify_all();
    }
}

/// Genea's side of the input.
struct Input(Arc<Shared>);

impl Write for Input {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let mut state = self.0.state.lock().unwrap();
        if state.hung_up {
            return Err(io::ErrorKind::BrokenPipe.into());
        }
        state.input.extend_from_slice(bytes);
        self.0.changed.notify_all();
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Control(Arc<Shared>);

impl PtyControl for Control {
    fn id(&self) -> Option<u32> {
        None
    }

    fn resize(&self, size: PtySize) -> io::Result<()> {
        self.0.state.lock().unwrap().size = size;
        self.0.changed.notify_all();
        Ok(())
    }

    fn hang_up(&self) -> io::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        if state.exit.is_none() {
            state.hung_up = true;
            state.exit = Some(Exit { code: None, signal: Some(SIGHUP) });
        }
        self.0.changed.notify_all();
        Ok(())
    }

    fn interrupt(&self) -> io::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        if state.exit.is_none() {
            state.interrupted = true;
        }
        self.0.changed.notify_all();
        Ok(())
    }

    fn kill(&self) -> io::Result<()> {
        let mut state = self.0.state.lock().unwrap();
        if state.exit.is_none() {
            state.killed = true;
            state.exit = Some(Exit { code: None, signal: Some(SIGKILL) });
        }
        self.0.changed.notify_all();
        Ok(())
    }

    fn wait(&self) -> io::Result<Exit> {
        let state = self.0.state.lock().unwrap();
        let state = self.0.changed.wait_while(state, |s| s.exit.is_none()).unwrap();
        Ok(state.exit.expect("waited for it"))
    }
}

#[cfg(test)]
mod tests {
    //! Smoke tests of the fake itself, so tests built on it can trust it.

    use super::*;
    use std::sync::mpsc;

    #[test]
    fn output_returns_once_read_and_input_and_hang_up_reach_the_test() {
        let ptys = ScriptedPtys::default();
        let size = PtySize { rows: 24, columns: 80 };
        let pty = ptys.spawn(&ProcessSpec::new("/bin/zsh").arg("-l"), size).unwrap();
        let (tx, rx) = mpsc::channel();
        let mut output = pty.output;
        let reader = std::thread::spawn(move || {
            let mut buffer = [0; 4];
            loop {
                let n = output.read(&mut buffer).unwrap();
                tx.send(buffer[..n].to_vec()).unwrap();
                if n == 0 {
                    return;
                }
            }
        });
        let fake = ptys.wait_for_spawn(1);
        assert_eq!(fake.spec().args, ["-l"]);

        fake.output("hello");
        // Read in pieces of four, all before `output` returned.
        let read: Vec<u8> = rx.try_iter().flatten().collect();
        assert_eq!(read, b"hello");

        let mut input = pty.input;
        input.write_all(b"ls\r").unwrap();
        assert_eq!(fake.wait_for_input("ls"), "ls\r");
        pty.control.resize(PtySize { rows: 30, columns: 100 }).unwrap();
        fake.wait_for_size(PtySize { rows: 30, columns: 100 });

        pty.control.hang_up().unwrap();
        assert!(fake.hung_up());
        reader.join().unwrap();
        assert_eq!(pty.control.wait().unwrap(), Exit { code: None, signal: Some(SIGHUP) });
    }

    #[test]
    fn an_exit_ends_the_output() {
        let ptys = ScriptedPtys::default();
        let pty = ptys.spawn(&ProcessSpec::new("sh"), PtySize { rows: 24, columns: 80 }).unwrap();
        ptys.last().exit(3);
        let mut out = String::new();
        let mut output = pty.output;
        output.read_to_string(&mut out).unwrap();
        assert_eq!(out, "");
        assert_eq!(pty.control.wait().unwrap(), Exit::code(3));
    }
}
