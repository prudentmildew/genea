//! The project environment (ticket #36, ADR 0005): what every process
//! Genea starts for a project gets.
//!
//! On open, the user's login shell (`SHELL` in the host's launch
//! environment) runs once, in the project root, only to print its
//! environment variables. They replace the launch environment for the
//! project's processes. The pinned runtime and package manager go first on
//! PATH when a process starts, so a download that finishes later still
//! counts.
//!
//! **Starting a process**: take [`Project::process_env`] (a `Send` snapshot,
//! fine to move into a background job) and pass every spec through
//! [`ProcessEnv::apply`] before spawning it, through the host's processes
//! or a PTY. The terminal, script runner and language servers all do this.
//!
//! [`Project::process_env`]: crate::project::Project::process_env

use std::{
    ffi::{OsStr, OsString},
    io::Read,
    path::{Path, PathBuf},
};

use genea_host::{ProcessSpec, SharedHost};

use crate::{
    jobs::Jobs,
    workbench::{Core, ProjectId},
};

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
    /// Bumped by every capture, so a stale one's result is dropped.
    generation: u64,
}

impl Environment {
    pub(crate) fn new(project: ProjectId, root: PathBuf, host: SharedHost) -> Self {
        let vars = host.launch_environment();
        Environment { project, root, host, vars, generation: 0 }
    }

    /// Runs the login shell in the background and takes its variables.
    pub(crate) fn capture(&mut self, jobs: &Jobs) {
        self.generation += 1;
        let generation = self.generation;
        let launch = self.host.launch_environment();
        let Some(shell) = value(&launch, "SHELL").map(PathBuf::from) else {
            self.vars = launch;
            return;
        };
        let (id, root, host) = (self.project, self.root.clone(), self.host.clone());
        jobs.spawn("capture the login-shell environment", move || {
            let spec = ProcessSpec {
                program: shell,
                args: ["-l", "-i", "-c", &script()].map(OsString::from).to_vec(),
                cwd: Some(root),
                env: launch,
                clear_env: true,
            };
            let captured = host.processes().spawn(&spec).ok().and_then(|mut child| {
                drop(child.stdin.take());
                let mut out = Vec::new();
                child.stdout.take()?.read_to_end(&mut out).ok()?;
                let _ = child.control.wait();
                parse(&out)
            });
            Box::new(move |core| {
                let Some(environment) = environment_mut(core, id) else { return };
                if environment.generation != generation {
                    return;
                }
                if let Some(vars) = captured {
                    environment.vars = vars;
                }
            })
        });
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

fn environment_mut(core: &mut Core, id: ProjectId) -> Option<&mut Environment> {
    core.project_mut(id)?.environment.as_mut()
}
