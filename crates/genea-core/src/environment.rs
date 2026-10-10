//! The project environment (ticket #36, ADR 0005): what every process
//! Genea starts for a project gets.
//!
//! On open, the user's login shell (`SHELL` in the host's launch
//! environment) runs once, in the project root, only to print its
//! environment variables. They replace the launch environment for the
//! project's processes. If the shell fails, or doesn't finish within
//! [`LOGIN_SHELL_TIMEOUT`] on the host clock, processes get the launch
//! environment and a notice says why. The pinned runtime and package
//! manager go first on PATH when a process starts, so a download that
//! finishes later still counts.
//!
//! **Starting a process**: take [`Project::process_env`] (a `Send` snapshot,
//! fine to move into a background job) and pass every spec through
//! [`ProcessEnv::apply`] before spawning it, through the host's processes
//! or a PTY. The terminal, script runner and language servers all do this.
//! A process started before the capture lands gets the launch environment.
//!
//! [`Project::process_env`]: crate::project::Project::process_env

use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::Duration,
};

use genea_host::{Host, ProcessControl, ProcessSpec, SharedHost};

use crate::{
    command::Command,
    jobs::Jobs,
    view::{Notice, NoticeAction},
    workbench::{Core, ProjectId},
};

/// How long the login shell may take before Genea gives up on it and falls
/// back to the launch environment, on the host clock.
pub const LOGIN_SHELL_TIMEOUT: Duration = Duration::from_secs(10);

/// Environment variables, in order.
pub(crate) type Vars = Vec<(OsString, OsString)>;

/// Marks the start and end of the variables in the shell's output, so
/// whatever its rc files print around them is ignored.
const START: &str = "__GENEA_ENVIRONMENT_START__";
const END: &str = "__GENEA_ENVIRONMENT_END__";

/// Variables that describe the capturing shell itself, not the user's
/// environment.
const SHELL_OWN: [&str; 4] = ["PWD", "OLDPWD", "SHLVL", "_"];

pub(crate) struct Environment {
    project: ProjectId,
    root: PathBuf,
    host: SharedHost,
    /// The variables processes get: the login shell's once captured, else
    /// the launch environment.
    vars: Vars,
    /// Why the last capture fell back to the launch environment.
    problem: Option<String>,
    /// Bumped by every capture, so a stale one's result is dropped.
    generation: u64,
    /// A capture is running: see [`is_ready`](Self::is_ready).
    capturing: bool,
}

/// How a capture ended.
enum Outcome {
    Captured(Vars),
    /// The shell failed or timed out: the notice's message.
    Fallback(String),
}

/// One run of the login shell, shared by the job that reads it and the
/// timer that gives up on it. Whichever finishes it first decides.
#[derive(Default)]
struct Attempt {
    finished: bool,
    /// The running shell, so the timer can kill it.
    control: Option<Box<dyn ProcessControl>>,
}

impl Environment {
    pub(crate) fn new(project: ProjectId, root: PathBuf, host: SharedHost) -> Self {
        let vars = host.launch_environment();
        Environment { project, root, host, vars, problem: None, generation: 0, capturing: false }
    }

    /// Runs the login shell in the background and takes its variables.
    pub(crate) fn capture(&mut self, jobs: &Jobs) {
        self.generation += 1;
        let generation = self.generation;
        let launch = self.host.launch_environment();
        let Some(shell) = value(&launch, "SHELL").map(PathBuf::from) else {
            (self.vars, self.problem, self.capturing) = (launch, None, false);
            return;
        };
        self.capturing = true;
        let attempt = Arc::new(Mutex::new(Attempt::default()));
        let id = self.project;

        // The timer starts here, on the main thread, so the timeout counts
        // from the open however late the job gets to run.
        let timeout = format!(
            "Your login shell ({}) didn't finish within {} s, so processes get the environment Genea was started with.",
            shell.display(),
            LOGIN_SHELL_TIMEOUT.as_secs()
        );
        let (timer_jobs, timer_attempt, timer_launch) = (jobs.clone(), attempt.clone(), launch.clone());
        self.host.clock().after(
            LOGIN_SHELL_TIMEOUT,
            Box::new(move || {
                let mut attempt = timer_attempt.lock().unwrap();
                if std::mem::replace(&mut attempt.finished, true) {
                    return;
                }
                if let Some(control) = &mut attempt.control {
                    let _ = control.kill();
                }
                drop(attempt);
                let outcome = Outcome::Fallback(timeout);
                timer_jobs.busy().finish(Box::new(move |core| finished(core, id, generation, timer_launch, outcome)));
            }),
        );

        let (root, host) = (self.root.clone(), self.host.clone());
        jobs.spawn("capture the login-shell environment", move || {
            let spec = ProcessSpec {
                program: shell.clone(),
                args: ["-l", "-i", "-c", &script()].map(OsString::from).to_vec(),
                cwd: Some(root),
                env: launch.clone(),
                clear_env: true,
            };
            let outcome = run(host.as_ref(), &spec, &attempt).map(|result| match result {
                Ok(vars) => Outcome::Captured(vars),
                Err(reason) => Outcome::Fallback(format!(
                    "Couldn't read the environment of your login shell ({}): {reason}. Processes get the environment Genea was started with.",
                    shell.display()
                )),
            });
            Box::new(move |core| {
                if let Some(outcome) = outcome {
                    finished(core, id, generation, launch, outcome);
                }
            })
        });
    }

    /// Whether the variables are final for now: no capture is running.
    /// Processes that should see the login shell's variables, like the
    /// terminal's shell, wait for this.
    pub(crate) fn is_ready(&self) -> bool {
        !self.capturing
    }

    pub(crate) fn notices(&self) -> Vec<Notice> {
        let reload = NoticeAction { label: "Reload environment".into(), command: Command::ReloadEnvironment };
        self.problem.iter().map(|message| Notice { message: message.clone(), action: Some(reload.clone()) }).collect()
    }

    /// The environment with `first` (the pinned tools' folders) put first
    /// on PATH.
    pub(crate) fn process_env<'a>(&self, first: impl IntoIterator<Item = &'a Path>) -> ProcessEnv {
        let mut path: Vec<PathBuf> = Vec::new();
        for dir in first {
            if !path.iter().any(|seen| seen == dir) {
                path.push(dir.to_owned());
            }
        }
        let mut vars = self.vars.clone();
        if !path.is_empty() {
            path.extend(value(&vars, "PATH").into_iter().flat_map(std::env::split_paths));
            // A PATH entry can't contain ':' (the store's folders never do).
            if let Ok(joined) = std::env::join_paths(path) {
                vars.retain(|(key, _)| key != "PATH");
                vars.push(("PATH".into(), joined));
            }
        }
        ProcessEnv { root: self.root.clone(), vars }
    }
}

/// What a project's processes get: a snapshot of its environment. Cheap to
/// clone and `Send`.
#[derive(Clone, Debug)]
pub(crate) struct ProcessEnv {
    root: PathBuf,
    vars: Vars,
}

impl ProcessEnv {
    /// Gives `spec` the project's environment and nothing else. Variables
    /// the spec sets itself win; without a working folder it runs in the
    /// project root.
    pub(crate) fn apply(&self, mut spec: ProcessSpec) -> ProcessSpec {
        let own = std::mem::take(&mut spec.env);
        spec.env = self.vars.iter().cloned().chain(own).collect();
        spec.clear_env = true;
        spec.cwd.get_or_insert_with(|| self.root.clone());
        spec
    }

    /// A variable's value, if set.
    pub(crate) fn var(&self, key: &str) -> Option<&OsStr> {
        value(&self.vars, key).map(OsString::as_os_str)
    }
}

/// Applies a capture's outcome, unless a newer capture has started.
fn finished(core: &mut Core, id: ProjectId, generation: u64, launch: Vars, outcome: Outcome) {
    let Some(environment) = core.project_mut(id).and_then(|project| project.environment.as_mut()) else { return };
    if environment.generation != generation {
        return;
    }
    environment.capturing = false;
    (environment.vars, environment.problem) = match outcome {
        Outcome::Captured(vars) => (vars, None),
        Outcome::Fallback(message) => (launch, Some(message)),
    };
}

/// Runs the login shell and reads its variables. `None` if the timer gave
/// up on it first.
fn run(host: &dyn Host, spec: &ProcessSpec, attempt: &Mutex<Attempt>) -> Option<Result<Vars, String>> {
    let finish = |result| (!std::mem::replace(&mut attempt.lock().unwrap().finished, true)).then_some(result);
    let mut child = match host.processes().spawn(spec) {
        Ok(child) => child,
        Err(error) => return finish(Err(format!("it didn't start ({error})"))),
    };
    // No input: an interactive shell that reads gets end-of-file.
    drop(child.stdin.take());
    {
        let mut attempt = attempt.lock().unwrap();
        if attempt.finished {
            let _ = child.control.kill();
        }
        attempt.control = Some(child.control);
    }
    // Drain stderr alongside stdout, so a chatty rc file can't fill its pipe.
    let errors = child.stderr.take().map(|mut stderr| {
        std::thread::spawn(move || {
            let mut text = Vec::new();
            let _ = stderr.read_to_end(&mut text);
            text
        })
    });
    let mut out = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_end(&mut out);
    }
    let errors = errors.and_then(|thread| thread.join().ok()).unwrap_or_default();
    let control = attempt.lock().unwrap().control.take();
    let exit = control.map(|mut control| control.wait());
    finish(parse(&out).ok_or_else(|| {
        let status = match exit {
            Some(Ok(exit)) => match (exit.code, exit.signal) {
                (Some(code), _) => format!("it exited with code {code}"),
                (None, Some(signal)) => format!("it was stopped by signal {signal}"),
                (None, None) => "it printed no environment".to_owned(),
            },
            _ => "it printed no environment".to_owned(),
        };
        let errors = String::from_utf8_lossy(&errors);
        match errors.lines().rev().map(str::trim).find(|line| !line.is_empty()) {
            Some(line) => format!("{status}: {line}"),
            None => status,
        }
    }))
}

/// The command the login shell runs: print the environment between markers.
/// Valid in sh, bash, zsh and fish.
fn script() -> String {
    format!("printf '%s' '{START}'; /usr/bin/env -0; printf '%s' '{END}'")
}

/// The variables between the markers, `NUL`-separated `KEY=value` entries.
fn parse(out: &[u8]) -> Option<Vars> {
    use std::os::unix::ffi::OsStrExt;
    let start = find(out, START.as_bytes())? + START.len();
    let end = start + find(&out[start..], END.as_bytes())?;
    let vars = out[start..end]
        .split(|byte| *byte == 0)
        .filter_map(|entry| {
            let eq = entry.iter().position(|byte| *byte == b'=')?;
            let (key, value) = (&entry[..eq], &entry[eq + 1..]);
            (!key.is_empty() && !SHELL_OWN.iter().any(|own| own.as_bytes() == key))
                .then(|| (OsStr::from_bytes(key).to_owned(), OsStr::from_bytes(value).to_owned()))
        })
        .collect();
    Some(vars)
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
}

fn value<'a>(vars: &'a Vars, key: &str) -> Option<&'a OsString> {
    vars.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v)
}
