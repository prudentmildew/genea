//! Terminal tabs and links (ticket #39): several shell tabs in the terminal
//! pane, and `path:line:col` references in their output that open the file.
//!
//! The test host's fake PTY plays each tab's shell (`host.ptys()` records
//! one per tab, in the order they started).

use genea_core::{Command, ProjectId, TerminalStatus, TerminalView, Workbench};
use genea_testkit::{FakePty, FixtureProject, TestHost};

/// A project open on `host`, settled, with its first tab's fake PTY.
fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId, FakePty) {
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    let pty = host.ptys().last();
    (workbench, project, pty)
}

fn fixture() -> FixtureProject {
    FixtureProject::new().file("src/app.ts", "").build()
}

fn terminal(workbench: &Workbench, project: ProjectId) -> TerminalView {
    workbench.project(project).unwrap().terminal
}

/// The active tab's rows as text, without the empty rows at the bottom.
fn screen(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let mut lines: Vec<String> = terminal(workbench, project).lines.into_iter().map(|line| line.text).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Each tab's title and status, and which one is active.
fn tabs(workbench: &Workbench, project: ProjectId) -> (Vec<(String, TerminalStatus)>, usize) {
    let view = terminal(workbench, project);
    (view.tabs.into_iter().map(|tab| (tab.title, tab.status)).collect(), view.active)
}

fn running(title: &str) -> (String, TerminalStatus) {
    (title.into(), TerminalStatus::Running)
}

#[test]
fn a_new_tab_runs_its_own_shell_and_shows_its_output() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, first) = open(&host, &fixture);
    first.output("first");
    workbench.settle().unwrap();
    assert_eq!(tabs(&workbench, project), (vec![running("zsh")], 0));

    workbench.dispatch(project, Command::NewTerminalTab);
    workbench.settle().unwrap();

    let second = host.ptys().wait_for_spawn(2);
    assert_eq!(second.spec().cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));
    second.output("second");
    workbench.settle().unwrap();
    assert_eq!(tabs(&workbench, project), (vec![running("zsh"), running("zsh")], 1));
    assert_eq!(screen(&workbench, project), ["second"]);
    let view = terminal(&workbench, project);
    assert!(view.visible && view.focused);
    assert!(!first.hung_up());
}
