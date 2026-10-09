//! The terminal (ticket #38): one shell per project, in a pane next to the
//! editor, running on a PTY through the host.
//!
//! The test host's fake PTY plays the shell: `FakePty::output` replays
//! what the shell prints and returns once Genea has read it, so a
//! `settle()` after it shows it on the grid; `wait_for_input` sees what the
//! user typed.

use std::path::Path;

use genea_core::{ProjectId, TerminalView, Workbench};
use genea_testkit::{FakePty, FixtureProject, TestHost};

/// A project open on `host`, settled, with its terminal's fake PTY.
fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId, FakePty) {
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let pty = host.ptys().last();
    (workbench, project, pty)
}

fn fixture() -> FixtureProject {
    FixtureProject::new().file("src/main.ts", "").build()
}

fn terminal(workbench: &Workbench, project: ProjectId) -> TerminalView {
    workbench.project(project).unwrap().terminal
}

/// The grid's rows as text, without the empty rows at the bottom.
fn screen(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let mut lines: Vec<String> = terminal(workbench, project).lines.into_iter().map(|line| line.text).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

fn var(pty: &FakePty, key: &str) -> Option<String> {
    let env = pty.spec().env;
    env.iter().rev().find(|(k, _)| k == key).map(|(_, v)| v.to_string_lossy().into_owned())
}

#[test]
fn the_shell_is_the_users_login_shell_in_the_project_root() {
    let host = TestHost::new();
    host.set_launch_environment([("SHELL", "/bin/bash"), ("PATH", "/usr/bin:/bin")]);
    host.processes().script_shell("bash", [("PATH", "/opt/mine/bin:/usr/bin:/bin"), ("GREETING", "hello")]);
    let fixture = fixture();

    let (_workbench, _project, pty) = open(&host, &fixture);

    let spec = pty.spec();
    assert_eq!(spec.program, Path::new("/bin/bash"));
    assert_eq!(spec.args, ["-l"]);
    assert_eq!(spec.cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));
    assert!(spec.clear_env);
    // The project environment: the login shell's variables.
    assert_eq!(var(&pty, "GREETING").as_deref(), Some("hello"));
    assert_eq!(var(&pty, "PATH").as_deref(), Some("/opt/mine/bin:/usr/bin:/bin"));
    assert_eq!(var(&pty, "TERM").as_deref(), Some("xterm-256color"));
    assert_eq!(var(&pty, "COLORTERM").as_deref(), Some("truecolor"));
}

#[test]
fn what_the_shell_prints_shows_on_the_grid() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, pty) = open(&host, &fixture);

    pty.output("hello\r\nworld");
    workbench.settle().unwrap();

    assert_eq!(screen(&workbench, project), ["hello", "world"]);
    let cursor = terminal(&workbench, project).cursor.unwrap();
    assert_eq!((cursor.line, cursor.column), (1, 5));
}
