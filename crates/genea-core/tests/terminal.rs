//! The terminal (ticket #38): one shell per project, in a pane next to the
//! editor, running on a PTY through the host.
//!
//! The test host's fake PTY plays the shell: `FakePty::output` replays
//! what the shell prints and returns once Genea has read it, so a
//! `settle()` after it shows it on the grid; `wait_for_input` sees what the
//! user typed.

use std::{path::Path, sync::mpsc, time::Duration};

use genea_core::{
    Command, Modifiers, ProjectId, TerminalKey, TerminalStatus, TerminalView, ToolState, ToolView, Workbench,
};
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

/// Plays the shell for one command line: runs it with `/bin/sh` in the
/// terminal's environment and folder, and prints its output on the PTY.
fn shell_runs(pty: &FakePty, command: &str) {
    let spec = pty.spec();
    let mut sh = std::process::Command::new("/bin/sh");
    sh.arg("-c").arg(command).env_clear().envs(spec.env.iter().map(|(k, v)| (k, v)));
    if let Some(cwd) = &spec.cwd {
        sh.current_dir(cwd);
    }
    let out = sh.output().unwrap();
    pty.output(String::from_utf8_lossy(&out.stdout).replace('\n', "\r\n"));
}

/// Types a command line into the terminal, as the user would.
fn type_line(workbench: &mut Workbench, project: ProjectId, line: &str) {
    workbench.dispatch(project, Command::TerminalText(line.into()));
    workbench.dispatch(project, Command::TerminalKey(TerminalKey::Enter, Modifiers::default()));
}

#[test]
fn node_and_pnpm_in_the_terminal_are_the_pinned_versions_once_downloaded() {
    let host = TestHost::new();
    let node = host.tools().node("24.18.0");
    host.tools().pnpm("11.13.0");
    host.download_server().hold(&node);
    let package_json = r#"{
  "name": "app",
  "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
  "packageManager": "pnpm@11.13.0"
}
"#;
    let fixture = FixtureProject::new().file("package.json", package_json).build();
    let mut workbench = Workbench::new(host.shared());
    let (notify, notified) = mpsc::channel();
    workbench.set_notifier(move || {
        let _ = notify.send(());
    });
    let project = workbench.open_project(fixture.root()).unwrap();

    // The shell waits for the pinned tools, so they are on its PATH.
    loop {
        workbench.pump();
        let runtime = workbench.project(project).unwrap().toolchain.runtime;
        if matches!(runtime, Some(ToolView { state: ToolState::Downloading { .. }, .. })) {
            break;
        }
        notified.recv_timeout(Duration::from_secs(10)).expect("Node never started downloading");
    }
    host.download_server().wait_held(&node);
    workbench.pump();
    assert!(host.ptys().spawned().is_empty());
    assert_eq!(terminal(&workbench, project).status, TerminalStatus::Starting);

    host.download_server().release(&node);
    workbench.settle().unwrap();
    let pty = host.ptys().last();
    assert_eq!(terminal(&workbench, project).status, TerminalStatus::Running);

    type_line(&mut workbench, project, "node -v");
    pty.wait_for_input("node -v\r");
    shell_runs(&pty, "node -v");
    type_line(&mut workbench, project, "pnpm -v");
    pty.wait_for_input("pnpm -v\r");
    shell_runs(&pty, "pnpm -v");
    workbench.settle().unwrap();

    assert_eq!(screen(&workbench, project), ["v24.18.0", "11.13.0"]);
}
