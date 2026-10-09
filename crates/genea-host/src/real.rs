//! The real host: system clock, real child processes and pseudo-terminals,
//! real HTTP.

use std::{
    collections::BinaryHeap,
    cmp::Reverse,
    ffi::OsString,
    io::{self, Write},
    path::{Path, PathBuf},
    process::Stdio,
    sync::{Arc, Condvar, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use crate::{
    Child, Clipboard, Clock, DownloadError, Downloads, Exit, Host, ProcessControl, ProcessSpec, Processes, Pty,
    PtyControl, PtySize, Ptys, SharedHost, TimerCallback,
};

/// The host the app runs on.
pub struct RealHost {
    clock: SystemClock,
    processes: OsProcesses,
    ptys: OsPtys,
    downloads: HttpDownloads,
    support_dir: PathBuf,
    clipboard: ClipboardSlot,
}

impl Default for RealHost {
    fn default() -> Self {
        // Without a home folder (never on a normal login) fall back to a
        // folder that is at least writable.
        let home = std::env::home_dir().unwrap_or_else(std::env::temp_dir);
        RealHost {
            clock: SystemClock::default(),
            processes: OsProcesses::default(),
            ptys: OsPtys,
            downloads: HttpDownloads::default(),
            support_dir: home.join("Library/Application Support/Genea"),
            clipboard: ClipboardSlot::default(),
        }
    }
}

impl RealHost {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn shared() -> SharedHost {
        Arc::new(Self::new())
    }

    /// Uses `clipboard` as the system clipboard. The pasteboard lives in
    /// AppKit, which no core crate may link (ADR 0004), so the view layer
    /// supplies it. Without one, the clipboard is private to the process.
    pub fn with_clipboard(mut self, clipboard: impl Clipboard + 'static) -> Self {
        self.clipboard = ClipboardSlot(Box::new(clipboard));
        self
    }
}

impl Host for RealHost {
    fn clock(&self) -> &dyn Clock {
        &self.clock
    }

    fn processes(&self) -> &dyn Processes {
        &self.processes
    }

    fn ptys(&self) -> &dyn Ptys {
        &self.ptys
    }

    fn downloads(&self) -> &dyn Downloads {
        &self.downloads
    }

    fn support_dir(&self) -> &Path {
        &self.support_dir
    }

    fn clipboard(&self) -> &dyn Clipboard {
        self.clipboard.0.as_ref()
    }

    fn launch_environment(&self) -> Vec<(OsString, OsString)> {
        let mut vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
        // launchd sets SHELL for apps; without it, assume macOS's default.
        if !vars.iter().any(|(key, _)| key == "SHELL") {
            vars.push(("SHELL".into(), "/bin/zsh".into()));
        }
        vars
    }
}

// --- Clipboard ---------------------------------------------------------------

struct ClipboardSlot(Box<dyn Clipboard>);

impl Default for ClipboardSlot {
    fn default() -> Self {
        ClipboardSlot(Box::new(ProcessClipboard::default()))
    }
}

/// A clipboard private to the process, until the app plugs in the system one.
#[derive(Default)]
struct ProcessClipboard {
    text: Mutex<Option<String>>,
}

impl Clipboard for ProcessClipboard {
    fn read_text(&self) -> Option<String> {
        self.text.lock().unwrap().clone()
    }

    fn write_text(&self, text: &str) {
        *self.text.lock().unwrap() = Some(text.to_owned());
    }
}

// --- Clock -------------------------------------------------------------------

/// The monotonic system clock. Timers share one lazily started thread, which
/// sleeps until the next deadline, so an idle Genea has no timer wake-ups.
#[derive(Default)]
struct SystemClock {
    timers: OnceLock<Arc<TimerThread>>,
}

#[derive(Default)]
struct TimerThread {
    queue: Mutex<TimerQueue>,
    changed: Condvar,
}

#[derive(Default)]
struct TimerQueue {
    heap: BinaryHeap<Reverse<(Instant, u64)>>,
    callbacks: std::collections::HashMap<u64, TimerCallback>,
    next_id: u64,
}

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }

    fn after(&self, delay: Duration, fire: TimerCallback) {
        let timers = self.timers.get_or_init(|| {
            let timers = Arc::new(TimerThread::default());
            let worker = timers.clone();
            std::thread::Builder::new()
                .name("genea-timers".into())
                .spawn(move || worker.run())
                .expect("spawn the timer thread");
            timers
        });
        let mut queue = timers.queue.lock().unwrap();
        let id = queue.next_id;
        queue.next_id += 1;
        queue.heap.push(Reverse((Instant::now() + delay, id)));
        queue.callbacks.insert(id, fire);
        timers.changed.notify_one();
    }

    fn system_time(&self) -> SystemTime {
        SystemTime::now()
    }
}

impl TimerThread {
    fn run(&self) {
        let mut queue = self.queue.lock().unwrap();
        loop {
            let now = Instant::now();
            match queue.heap.peek().copied() {
                None => queue = self.changed.wait(queue).unwrap(),
                Some(Reverse((due, id))) if due <= now => {
                    queue.heap.pop();
                    let fire = queue.callbacks.remove(&id);
                    drop(queue);
                    if let Some(fire) = fire {
                        fire();
                    }
                    queue = self.queue.lock().unwrap();
                }
                Some(Reverse((due, _))) => queue = self.changed.wait_timeout(queue, due - now).unwrap().0,
            }
        }
    }
}

// --- Processes ---------------------------------------------------------------

#[derive(Default)]
struct OsProcesses;

impl Processes for OsProcesses {
    fn spawn(&self, spec: &ProcessSpec) -> io::Result<Child> {
        let mut command = std::process::Command::new(&spec.program);
        command.args(&spec.args).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if spec.clear_env {
            command.env_clear();
        }
        command.envs(spec.env.iter().map(|(k, v)| (k, v)));
        let mut child = command.spawn()?;
        Ok(Child {
            stdin: child.stdin.take().map(|p| Box::new(p) as Box<dyn Write + Send>),
            stdout: child.stdout.take().map(|p| Box::new(p) as _),
            stderr: child.stderr.take().map(|p| Box::new(p) as _),
            control: Box::new(OsChild(child)),
        })
    }
}

struct OsChild(std::process::Child);

fn exit_of(status: std::process::ExitStatus) -> Exit {
    use std::os::unix::process::ExitStatusExt;
    Exit { code: status.code(), signal: status.signal() }
}

impl ProcessControl for OsChild {
    fn id(&self) -> Option<u32> {
        Some(self.0.id())
    }

    fn kill(&mut self) -> io::Result<()> {
        self.0.kill()
    }

    fn wait(&mut self) -> io::Result<Exit> {
        self.0.wait().map(exit_of)
    }

    fn try_wait(&mut self) -> io::Result<Option<Exit>> {
        Ok(self.0.try_wait()?.map(exit_of))
    }
}

// --- Pseudo-terminals --------------------------------------------------------

/// Programs on real pseudo-terminals (`openpty`), each the leader of its
/// own session with the terminal as its controlling terminal.
struct OsPtys;

impl Ptys for OsPtys {
    fn spawn(&self, spec: &ProcessSpec, size: PtySize) -> io::Result<Pty> {
        use std::os::{
            fd::{FromRawFd, OwnedFd},
            unix::process::CommandExt,
        };

        let (mut master, mut slave) = (0, 0);
        let mut window = window_size(size);
        // SAFETY: openpty only writes the two descriptors and reads the
        // window size; the name and termios may be null.
        let opened = unsafe {
            libc::openpty(&mut master, &mut slave, std::ptr::null_mut(), std::ptr::null_mut(), &mut window)
        };
        if opened != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty succeeded, so both are open descriptors we own.
        let (master, slave) = unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
        // Neither may leak into the program (std dups the slave onto 0–2).
        for fd in [&master, &slave] {
            set_cloexec(fd)?;
        }

        let mut command = std::process::Command::new(&spec.program);
        command
            .args(&spec.args)
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if spec.clear_env {
            command.env_clear();
        }
        command.envs(spec.env.iter().map(|(k, v)| (k, v)));
        // SAFETY: only async-signal-safe calls between fork and exec.
        unsafe {
            command.pre_exec(|| {
                // A new session whose controlling terminal is the pty.
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                // Default dispositions and no blocked signals, whatever
                // Genea's own are.
                for signal in [libc::SIGCHLD, libc::SIGHUP, libc::SIGINT, libc::SIGQUIT, libc::SIGTERM, libc::SIGALRM] {
                    libc::signal(signal, libc::SIG_DFL);
                }
                let mut none: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut none);
                libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
                Ok(())
            });
        }
        let child = command.spawn()?;
        // Closes Genea's copies of the slave, so the output ends when the
        // program's side closes.
        drop(command);
        // `OsPty::wait` reaps it with waitpid; std's handle never does on drop.
        let pid = child.id() as libc::pid_t;
        drop(child);

        let output = std::fs::File::from(master.try_clone()?);
        let input = std::fs::File::from(master.try_clone()?);
        Ok(Pty {
            output: Box::new(output),
            input: Box::new(input),
            control: Box::new(OsPty { pid, master, exit: Mutex::new(None) }),
        })
    }
}

fn window_size(size: PtySize) -> libc::winsize {
    libc::winsize { ws_row: size.rows, ws_col: size.columns, ws_xpixel: 0, ws_ypixel: 0 }
}

fn set_cloexec(fd: &std::os::fd::OwnedFd) -> io::Result<()> {
    use std::os::fd::AsRawFd;
    // SAFETY: fcntl on a descriptor we own.
    if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

struct OsPty {
    pid: libc::pid_t,
    master: std::os::fd::OwnedFd,
    /// Set once reaped: the pid may belong to another process after that.
    exit: Mutex<Option<Exit>>,
}

impl PtyControl for OsPty {
    fn id(&self) -> Option<u32> {
        Some(self.pid as u32)
    }

    fn resize(&self, size: PtySize) -> io::Result<()> {
        use std::os::fd::AsRawFd;
        let window = window_size(size);
        // SAFETY: TIOCSWINSZ reads a winsize; the descriptor is ours.
        if unsafe { libc::ioctl(self.master.as_raw_fd(), libc::TIOCSWINSZ, &window) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn hang_up(&self) -> io::Result<()> {
        let exit = self.exit.lock().unwrap();
        if exit.is_some() {
            return Ok(());
        }
        // The program leads its own process group (setsid).
        // SAFETY: plain syscall; the process isn't reaped (checked above,
        // under the lock `wait` takes to record its reaping).
        if unsafe { libc::killpg(self.pid, libc::SIGHUP) } == -1 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    fn wait(&self) -> io::Result<Exit> {
        if let Some(exit) = *self.exit.lock().unwrap() {
            return Ok(exit);
        }
        let mut status = 0;
        loop {
            // WNOWAIT leaves it a zombie, so its pid can't be reused until
            // the exit is recorded below.
            let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
            // SAFETY: waitid writes into `info`.
            let waited = unsafe { libc::waitid(libc::P_PID, self.pid as libc::id_t, &mut info, libc::WEXITED | libc::WNOWAIT) };
            if waited == 0 {
                break;
            }
            let error = io::Error::last_os_error();
            if error.kind() != io::ErrorKind::Interrupted {
                return Err(error);
            }
        }
        let mut exit = self.exit.lock().unwrap();
        if let Some(exit) = *exit {
            return Ok(exit);
        }
        // SAFETY: reaps the zombie waited for above.
        if unsafe { libc::waitpid(self.pid, &mut status, 0) } == -1 {
            return Err(io::Error::last_os_error());
        }
        let reaped = if libc::WIFEXITED(status) {
            Exit::code(libc::WEXITSTATUS(status))
        } else {
            Exit { code: None, signal: libc::WIFSIGNALED(status).then(|| libc::WTERMSIG(status)) }
        };
        *exit = Some(reaped);
        Ok(reaped)
    }
}

// --- Downloads ---------------------------------------------------------------

/// HTTP over the system TLS stack (Security.framework) and system trust
/// store, so no C crypto library is built.
#[derive(Default)]
struct HttpDownloads {
    agent: OnceLock<ureq::Agent>,
}

impl HttpDownloads {
    fn agent(&self) -> &ureq::Agent {
        self.agent.get_or_init(|| {
            use ureq::tls::{RootCerts, TlsConfig, TlsProvider};
            let tls = TlsConfig::builder()
                .provider(TlsProvider::NativeTls)
                .root_certs(RootCerts::PlatformVerifier)
                .build();
            ureq::Agent::config_builder()
                .tls_config(tls)
                .http_status_as_error(false)
                .user_agent(concat!("Genea/", env!("CARGO_PKG_VERSION")))
                .build()
                .into()
        })
    }
}

impl Downloads for HttpDownloads {
    fn fetch(&self, url: &str, sink: &mut dyn Write) -> Result<u64, DownloadError> {
        self.fetch_with_length(url, sink, &mut |_| {})
    }

    fn fetch_with_length(
        &self,
        url: &str,
        sink: &mut dyn Write,
        length: &mut dyn FnMut(u64),
    ) -> Result<u64, DownloadError> {
        let response = self.agent().get(url).call().map_err(|e| DownloadError::Transport(e.to_string()))?;
        let status = response.status().as_u16();
        if !(200..300).contains(&status) {
            return Err(DownloadError::Status(status));
        }
        if let Some(len) = response.body().content_length() {
            length(len);
        }
        let mut body = response.into_body().into_reader();
        Ok(io::copy(&mut body, sink)?)
    }
}

#[cfg(test)]
mod tests {
    //! Smoke tests of the adapters themselves. Behaviour is tested through
    //! the core API with the test host; these only check the real wiring.

    use super::*;
    use std::{io::Read, sync::mpsc};

    #[test]
    fn spawns_a_real_process_and_reads_its_output() {
        let host = RealHost::new();
        let mut child = host.processes().spawn(&ProcessSpec::new("/bin/echo").arg("hello")).unwrap();
        let mut out = String::new();
        child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
        assert_eq!(out, "hello\n");
        assert!(child.control.wait().unwrap().success());
    }

    #[test]
    fn a_program_on_a_pty_has_it_as_its_terminal_at_the_given_size() {
        let host = RealHost::new();
        let spec = ProcessSpec::new("/bin/sh").args(["-c", "test -t 0 && test -t 1 && stty size; printf done"]);
        let mut pty = host.ptys().spawn(&spec, PtySize { rows: 30, columns: 100 }).unwrap();
        let mut out = Vec::new();
        // The output ends with EIO on macOS once the program's side closes.
        let _ = pty.output.read_to_end(&mut out);
        assert_eq!(String::from_utf8_lossy(&out), "30 100\r\ndone");
        assert!(pty.control.wait().unwrap().success());
    }

    #[test]
    fn a_pty_takes_input_resizes_and_hangs_up() {
        let host = RealHost::new();
        let spec = ProcessSpec::new("/bin/sh").args(["-c", "read line; stty size; printf '<%s>' \"$line\"; exec cat"]);
        let pty = host.ptys().spawn(&spec, PtySize { rows: 24, columns: 80 }).unwrap();
        pty.control.resize(PtySize { rows: 40, columns: 120 }).unwrap();
        let (mut input, mut output) = (pty.input, pty.output);
        input.write_all(b"hello\r").unwrap();
        let mut seen = Vec::new();
        let mut buffer = [0; 256];
        while !String::from_utf8_lossy(&seen).contains("<hello>") {
            let n = output.read(&mut buffer).unwrap();
            assert!(n > 0, "{}", String::from_utf8_lossy(&seen));
            seen.extend_from_slice(&buffer[..n]);
        }
        assert!(String::from_utf8_lossy(&seen).contains("40 120"), "{}", String::from_utf8_lossy(&seen));

        pty.control.hang_up().unwrap();
        assert_eq!(pty.control.wait().unwrap(), Exit { code: None, signal: Some(libc::SIGHUP) });
        // Hanging up a reaped program does nothing.
        pty.control.hang_up().unwrap();
    }

    #[test]
    fn the_launch_environment_is_the_processs_own_and_names_a_shell() {
        let vars = RealHost::new().launch_environment();
        let path = vars.iter().find(|(key, _)| key == "PATH").map(|(_, value)| value.clone());
        assert_eq!(path, std::env::var_os("PATH"));
        assert!(vars.iter().any(|(key, value)| key == "SHELL" && !value.is_empty()));
    }

    #[test]
    fn fires_timers_in_deadline_order() {
        let host = RealHost::new();
        let (tx, rx) = mpsc::channel();
        for (delay, name) in [(30, "late"), (5, "early")] {
            let tx = tx.clone();
            host.clock().after(Duration::from_millis(delay), Box::new(move || tx.send(name).unwrap()));
        }
        let fired: Vec<_> = rx.iter().take(2).collect();
        assert_eq!(fired, ["early", "late"]);
    }
}
