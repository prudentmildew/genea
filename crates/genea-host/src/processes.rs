use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

/// Starting child processes.
pub trait Processes: Send + Sync {
    /// Starts `spec` with piped stdin, stdout and stderr.
    fn spawn(&self, spec: &ProcessSpec) -> io::Result<Child>;
}

/// What to run. A plain description, so the test host can match on it and
/// tests can assert on what Genea tried to start.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProcessSpec {
    pub program: PathBuf,
    pub args: Vec<OsString>,
    pub cwd: Option<PathBuf>,
    /// Variables set on top of the inherited environment, or on top of an
    /// empty one when `clear_env` is set.
    pub env: Vec<(OsString, OsString)>,
    pub clear_env: bool,
}

impl ProcessSpec {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        ProcessSpec { program: program.into(), args: Vec::new(), cwd: None, env: Vec::new(), clear_env: false }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_owned());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args.extend(args.into_iter().map(|a| a.as_ref().to_owned()));
        self
    }

    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_owned());
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.env.push((key.as_ref().to_owned(), value.as_ref().to_owned()));
        self
    }

    pub fn clear_env(mut self) -> Self {
        self.clear_env = true;
        self
    }

    /// The program's file name, which is what the test host matches on.
    pub fn program_name(&self) -> &OsStr {
        self.program.file_name().unwrap_or(self.program.as_os_str())
    }
}

/// A started child process. Take the pipes out of it and keep the control.
pub struct Child {
    pub stdin: Option<Box<dyn Write + Send>>,
    pub stdout: Option<Box<dyn Read + Send>>,
    pub stderr: Option<Box<dyn Read + Send>>,
    pub control: Box<dyn ProcessControl>,
}

/// Waiting for and stopping a child process.
pub trait ProcessControl: Send {
    /// The OS process id, if there is a real process.
    fn id(&self) -> Option<u32>;
    /// Sends SIGKILL (or makes a scripted process stop).
    fn kill(&mut self) -> io::Result<()>;
    /// Blocks until the process exits.
    fn wait(&mut self) -> io::Result<Exit>;
    /// Returns the exit if the process has already exited.
    fn try_wait(&mut self) -> io::Result<Option<Exit>>;
}

/// How a process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    /// The exit code, if it exited normally.
    pub code: Option<i32>,
    /// The signal that ended it, if any.
    pub signal: Option<i32>,
}

impl Exit {
    pub fn code(code: i32) -> Self {
        Exit { code: Some(code), signal: None }
    }

    pub fn success(&self) -> bool {
        self.code == Some(0)
    }
}
