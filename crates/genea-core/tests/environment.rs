//! The project environment (ticket #36, ADR 0005): every process Genea
//! starts for a project gets the variables of the user's login shell,
//! captured once per open in the project root, with the pinned runtime and
//! package manager first on PATH.
//!
//! The login shell is scripted through the test host: `script_shell` plays
//! it with a real `/bin/sh` whose environment is what the user's rc files
//! would export. Processes are started through `Workbench::spawn`, the way
//! the terminal, scripts and language servers start theirs.

use std::io::Read;

use genea_core::{ProcessSpec, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// A host whose launch environment names `/bin/zsh` as the login shell,
/// with a `printenv` that prints the environment it was given.
fn host() -> TestHost {
    let host = TestHost::new();
    host.set_launch_environment([
        ("SHELL", "/bin/zsh"),
        ("PATH", "/usr/bin:/bin:/usr/sbin:/sbin"),
        ("FROM_LAUNCH", "launch"),
    ]);
    host.processes().script("printenv", |spec, mut io| {
        use std::io::Write;
        for (key, value) in &spec.env {
            writeln!(io.stdout, "{}={}", key.to_string_lossy(), value.to_string_lossy()).unwrap();
        }
        0
    });
    host
}

/// What a process started for the project sees: its environment, as
/// `KEY=value` lines (a later line wins, as in the real environment).
fn printenv(workbench: &Workbench, project: ProjectId) -> Vec<(String, String)> {
    let mut child = workbench.spawn(project, ProcessSpec::new("printenv")).unwrap();
    let mut out = String::new();
    child.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    child.control.wait().unwrap();
    let mut vars: Vec<(String, String)> = Vec::new();
    for line in out.lines() {
        let (key, value) = line.split_once('=').unwrap();
        vars.retain(|(k, _)| k != key);
        vars.push((key.to_owned(), value.to_owned()));
    }
    vars
}

fn var(vars: &[(String, String)], key: &str) -> Option<String> {
    vars.iter().find(|(k, _)| k == key).map(|(_, v)| v.clone())
}

#[test]
fn a_process_sees_the_variables_of_the_login_shell() {
    let host = host();
    host.processes().script_shell("zsh", [("PATH", "/opt/mine/bin:/usr/bin:/bin"), ("GREETING", "hello")]);
    let fixture = FixtureProject::new().file("src/main.ts", "").build();
    let mut workbench = Workbench::new(host.shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    let vars = printenv(&workbench, project);
    assert_eq!(var(&vars, "GREETING").as_deref(), Some("hello"));
    assert_eq!(var(&vars, "PATH").as_deref(), Some("/opt/mine/bin:/usr/bin:/bin"));
    // The shell's environment replaces the one Genea was started with.
    assert_eq!(var(&vars, "FROM_LAUNCH"), None);
}
