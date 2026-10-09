//! The test host: a manual clock, scripted processes and scripted downloads.

use std::{
    collections::HashMap,
    ffi::{OsStr, OsString},
    io::{self, PipeReader, PipeWriter, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant, SystemTime},
};

use genea_host::{
    Child, Clock, DownloadError, Downloads, Exit, Host, ProcessControl, ProcessSpec, Processes, SharedHost,
    TimerCallback,
};

/// A host whose effects are scripted by the test.
///
/// It is a cheap handle: clone it, give [`shared`](Self::shared) to the
/// workbench and keep a clone to script processes, serve downloads and
/// advance the clock.
///
/// Each test host has its own application-support folder in a temp dir,
/// deleted with the last clone. Two workbenches on clones of one host share
/// it, which is how a test restarts Genea.
#[derive(Clone, Default)]
pub struct TestHost {
    inner: Arc<Inner>,
}

struct Inner {
    clock: ManualClock,
    processes: ScriptedProcesses,
    downloads: ScriptedDownloads,
    support: tempfile::TempDir,
}

impl Default for Inner {
    fn default() -> Self {
        Inner {
            clock: ManualClock::default(),
            processes: ScriptedProcesses::default(),
            downloads: ScriptedDownloads::default(),
            support: tempfile::Builder::new().prefix("genea-support-").tempdir().expect("create a temp dir"),
        }
    }
}

impl TestHost {
    pub fn new() -> Self {
        Self::default()
    }

    /// The host to hand to `Workbench::new`.
    pub fn shared(&self) -> SharedHost {
        Arc::new(self.clone())
    }

    pub fn clock(&self) -> &ManualClock {
        &self.inner.clock
    }

    pub fn processes(&self) -> &ScriptedProcesses {
        &self.inner.processes
    }

    pub fn downloads(&self) -> &ScriptedDownloads {
        &self.inner.downloads
    }
}

impl Host for TestHost {
    fn clock(&self) -> &dyn Clock {
        &self.inner.clock
    }

    fn processes(&self) -> &dyn Processes {
        &self.inner.processes
    }

    fn downloads(&self) -> &dyn Downloads {
        &self.inner.downloads
    }

    fn support_dir(&self) -> &Path {
        self.inner.support.path()
    }
}

// --- Clock -------------------------------------------------------------------

/// A clock that only moves when the test calls [`advance`](Self::advance).
///
/// Pending timers are not background work: `Workbench::settle` doesn't wait
/// for them. Advance the clock, then settle.
///
/// Its wall-clock time starts at [`ManualClock::epoch`] and moves with it.
pub struct ManualClock {
    start: Instant,
    state: Mutex<ClockState>,
}

#[derive(Default)]
struct ClockState {
    elapsed: Duration,
    timers: Vec<(Duration, u64, TimerCallback)>,
    next_id: u64,
}

impl Default for ManualClock {
    fn default() -> Self {
        ManualClock { start: Instant::now(), state: Mutex::default() }
    }
}

impl ManualClock {
    /// The wall-clock time a new manual clock starts at: 2026-01-01 00:00 UTC.
    pub fn epoch() -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_secs(1_767_225_600)
    }

    /// Moves time forward, firing every timer that comes due, in deadline
    /// order and on this thread.
    pub fn advance(&self, by: Duration) {
        let target = self.state.lock().unwrap().elapsed + by;
        loop {
            let due = {
                let mut state = self.state.lock().unwrap();
                let next = state
                    .timers
                    .iter()
                    .enumerate()
                    .filter(|(_, (at, _, _))| *at <= target)
                    .min_by_key(|(_, (at, id, _))| (*at, *id))
                    .map(|(i, _)| i);
                match next {
                    Some(i) => {
                        let (at, _, fire) = state.timers.remove(i);
                        state.elapsed = state.elapsed.max(at);
                        Some(fire)
                    }
                    None => {
                        state.elapsed = target;
                        None
                    }
                }
            };
            match due {
                Some(fire) => fire(),
                None => break,
            }
        }
    }

    /// How many timers are waiting to fire.
    pub fn pending_timers(&self) -> usize {
        self.state.lock().unwrap().timers.len()
    }
}

impl Clock for ManualClock {
    fn now(&self) -> Instant {
        self.start + self.state.lock().unwrap().elapsed
    }

    fn after(&self, delay: Duration, fire: TimerCallback) {
        let mut state = self.state.lock().unwrap();
        let id = state.next_id;
        state.next_id += 1;
        let at = state.elapsed + delay;
        state.timers.push((at, id, fire));
    }

    fn system_time(&self) -> SystemTime {
        Self::epoch() + self.state.lock().unwrap().elapsed
    }
}

// --- Processes ---------------------------------------------------------------

type Script = Arc<dyn Fn(&ProcessSpec, FakeProcess) -> i32 + Send + Sync>;

/// Child processes played by closures.
///
/// [`script`](Self::script) a program by file name; each spawn of it runs
/// the closure on its own thread with the process's pipes, and the closure's
/// return value is the exit code. Spawning an unscripted program fails with
/// `NotFound`, like a missing binary.
#[derive(Default)]
pub struct ScriptedProcesses {
    scripts: Mutex<HashMap<OsString, Script>>,
    spawned: Mutex<Vec<ProcessSpec>>,
}

/// The process side of a scripted child: read what Genea writes, write what
/// it should read.
pub struct FakeProcess {
    pub stdin: PipeReader,
    pub stdout: PipeWriter,
    pub stderr: PipeWriter,
    killed: Arc<AtomicBool>,
}

impl FakeProcess {
    /// Whether Genea has killed this process. A long-running script should
    /// stop when this turns true.
    pub fn killed(&self) -> bool {
        self.killed.load(Ordering::SeqCst)
    }
}

impl ScriptedProcesses {
    /// Plays every spawn of `program` (matched on its file name) with `script`.
    pub fn script(
        &self,
        program: impl AsRef<OsStr>,
        script: impl Fn(&ProcessSpec, FakeProcess) -> i32 + Send + Sync + 'static,
    ) {
        self.scripts.lock().unwrap().insert(program.as_ref().to_owned(), Arc::new(script));
    }

    /// Every spawn so far, scripted or not, in order.
    pub fn spawned(&self) -> Vec<ProcessSpec> {
        self.spawned.lock().unwrap().clone()
    }
}

impl Processes for ScriptedProcesses {
    fn spawn(&self, spec: &ProcessSpec) -> io::Result<Child> {
        self.spawned.lock().unwrap().push(spec.clone());
        let script = self.scripts.lock().unwrap().get(spec.program_name()).cloned().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no scripted process for {}", spec.program.display()))
        })?;
        let (stdin_r, stdin_w) = io::pipe()?;
        let (stdout_r, stdout_w) = io::pipe()?;
        let (stderr_r, stderr_w) = io::pipe()?;
        let killed = Arc::new(AtomicBool::new(false));
        let fake = FakeProcess { stdin: stdin_r, stdout: stdout_w, stderr: stderr_w, killed: killed.clone() };
        let (exit_tx, exit_rx) = mpsc::channel();
        let spec = spec.clone();
        std::thread::Builder::new().name(format!("fake {}", spec.program.display())).spawn(move || {
            let code = script(&spec, fake);
            let _ = exit_tx.send(Exit::code(code));
        })?;
        Ok(Child {
            stdin: Some(Box::new(stdin_w) as Box<dyn Write + Send>),
            stdout: Some(Box::new(stdout_r)),
            stderr: Some(Box::new(stderr_r)),
            control: Box::new(FakeControl { exit: exit_rx, killed, exited: None }),
        })
    }
}

struct FakeControl {
    exit: mpsc::Receiver<Exit>,
    killed: Arc<AtomicBool>,
    exited: Option<Exit>,
}

const SIGKILL: i32 = 9;

impl ProcessControl for FakeControl {
    fn id(&self) -> Option<u32> {
        None
    }

    fn kill(&mut self) -> io::Result<()> {
        self.killed.store(true, Ordering::SeqCst);
        if self.exited.is_none() {
            self.exited = Some(Exit { code: None, signal: Some(SIGKILL) });
        }
        Ok(())
    }

    fn wait(&mut self) -> io::Result<Exit> {
        if let Some(exit) = self.exited {
            return Ok(exit);
        }
        let exit = self.exit.recv().unwrap_or(Exit { code: None, signal: Some(SIGKILL) });
        self.exited = Some(exit);
        Ok(exit)
    }

    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        if self.exited.is_none() {
            self.exited = self.exit.try_recv().ok();
        }
        Ok(self.exited)
    }
}

// --- Downloads ---------------------------------------------------------------

/// Downloads answered from a table. Unknown URLs answer HTTP 404.
#[derive(Default)]
pub struct ScriptedDownloads {
    table: Mutex<HashMap<String, Result<Vec<u8>, u16>>>,
    requests: Mutex<Vec<String>>,
}

impl ScriptedDownloads {
    /// Answers `url` with these bytes.
    pub fn serve(&self, url: impl Into<String>, body: impl Into<Vec<u8>>) {
        self.table.lock().unwrap().insert(url.into(), Ok(body.into()));
    }

    /// Answers `url` with an HTTP error status.
    pub fn fail(&self, url: impl Into<String>, status: u16) {
        self.table.lock().unwrap().insert(url.into(), Err(status));
    }

    /// Every URL fetched so far, in order.
    pub fn requests(&self) -> Vec<String> {
        self.requests.lock().unwrap().clone()
    }
}

impl Downloads for ScriptedDownloads {
    fn fetch(&self, url: &str, sink: &mut dyn Write) -> Result<u64, DownloadError> {
        self.requests.lock().unwrap().push(url.to_owned());
        let answer = self.table.lock().unwrap().get(url).cloned().unwrap_or(Err(404));
        match answer {
            Ok(body) => {
                sink.write_all(&body)?;
                Ok(body.len() as u64)
            }
            Err(status) => Err(DownloadError::Status(status)),
        }
    }
}

#[cfg(test)]
mod tests {
    //! Smoke tests of the fakes themselves, so tests built on them can trust them.

    use super::*;
    use std::io::{BufRead, BufReader, Read};

    #[test]
    fn the_manual_clock_fires_due_timers_in_order_only_when_advanced() {
        let host = TestHost::new();
        let fired = Arc::new(Mutex::new(Vec::new()));
        for (ms, name) in [(300, "c"), (100, "a"), (200, "b")] {
            let fired = fired.clone();
            Host::clock(&host).after(Duration::from_millis(ms), Box::new(move || fired.lock().unwrap().push(name)));
        }
        let start = Host::clock(&host).now();

        host.clock().advance(Duration::from_millis(250));
        assert_eq!(*fired.lock().unwrap(), ["a", "b"]);
        assert_eq!(Host::clock(&host).now() - start, Duration::from_millis(250));
        assert_eq!(host.clock().pending_timers(), 1);

        host.clock().advance(Duration::from_millis(50));
        assert_eq!(*fired.lock().unwrap(), ["a", "b", "c"]);
    }

    #[test]
    fn the_manual_clocks_wall_time_starts_at_its_epoch_and_moves_with_it() {
        let host = TestHost::new();
        assert_eq!(Host::clock(&host).system_time(), ManualClock::epoch());

        host.clock().advance(Duration::from_secs(90));
        assert_eq!(Host::clock(&host).system_time(), ManualClock::epoch() + Duration::from_secs(90));
    }

    #[test]
    fn a_scripted_process_talks_over_real_pipes_and_exits_with_its_code() {
        let host = TestHost::new();
        host.processes().script("tsc", |spec, mut io| {
            let mut line = String::new();
            BufReader::new(&mut io.stdin).read_line(&mut line).unwrap();
            write!(io.stdout, "{} got {line}", spec.args[0].to_string_lossy()).unwrap();
            3
        });
        let spec = ProcessSpec::new("/project/node_modules/.bin/tsc").arg("--lsp");

        let mut child = Host::processes(&host).spawn(&spec).unwrap();
        child.stdin.take().unwrap().write_all(b"hello\n").unwrap();
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out).unwrap();

        assert_eq!(out, "--lsp got hello\n");
        assert_eq!(child.control.wait().unwrap(), Exit::code(3));
        assert_eq!(host.processes().spawned(), [spec]);
    }

    #[test]
    fn an_unscripted_process_is_not_found() {
        let host = TestHost::new();
        let error = Host::processes(&host).spawn(&ProcessSpec::new("node")).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn downloads_answer_from_the_table_or_404() {
        let host = TestHost::new();
        host.downloads().serve("https://example.test/a", "body");
        host.downloads().fail("https://example.test/b", 500);

        let mut sink = Vec::new();
        assert_eq!(Host::downloads(&host).fetch("https://example.test/a", &mut sink).unwrap(), 4);
        assert_eq!(sink, b"body");
        let status = |url| match Host::downloads(&host).fetch(url, &mut Vec::new()) {
            Err(DownloadError::Status(code)) => code,
            other => panic!("{other:?}"),
        };
        assert_eq!(status("https://example.test/b"), 500);
        assert_eq!(status("https://example.test/missing"), 404);
        assert_eq!(host.downloads().requests().len(), 3);
    }
}
