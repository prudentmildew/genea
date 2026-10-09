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

/// A project with two tabs, each having printed its name; the second is
/// active.
fn two_tabs(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId, FakePty, FakePty) {
    let (mut workbench, project, first) = open(host, fixture);
    first.output("first");
    workbench.dispatch(project, Command::NewTerminalTab);
    workbench.settle().unwrap();
    let second = host.ptys().wait_for_spawn(2);
    second.output("second");
    workbench.settle().unwrap();
    (workbench, project, first, second)
}

#[test]
fn switching_tabs_shows_each_ones_output_and_types_into_it() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, first, second) = two_tabs(&host, &fixture);

    workbench.dispatch(project, Command::SelectTerminalTab(0));
    assert_eq!(terminal(&workbench, project).active, 0);
    assert_eq!(screen(&workbench, project), ["first"]);
    workbench.dispatch(project, Command::TerminalText("ls".into()));
    assert_eq!(first.wait_for_input("ls"), "ls");

    // Output to a tab in the background lands in it.
    second.output(" and more");
    workbench.settle().unwrap();
    assert_eq!(screen(&workbench, project), ["first"]);
    workbench.dispatch(project, Command::SelectTerminalTab(1));
    assert_eq!(screen(&workbench, project), ["second and more"]);
    assert_eq!(second.input(), "");
}

#[test]
fn closing_a_tab_ends_its_shell_and_shows_a_neighbour() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, first, second) = two_tabs(&host, &fixture);

    workbench.dispatch(project, Command::CloseTerminalTab(1));

    second.wait_for_hang_up();
    assert!(!first.hung_up());
    assert_eq!(tabs(&workbench, project), (vec![running("zsh")], 0));
    assert_eq!(screen(&workbench, project), ["first"]);
}

#[test]
fn closing_the_last_tab_collapses_the_pane_and_showing_it_again_starts_a_new_shell() {
    let host = TestHost::new();
    let fixture = fixture();
    let (mut workbench, project, first) = open(&host, &fixture);
    workbench.dispatch(project, Command::FocusTerminal);

    workbench.dispatch(project, Command::CloseTerminalTab(0));

    first.wait_for_hang_up();
    let view = terminal(&workbench, project);
    assert!(view.tabs.is_empty());
    assert!(!view.visible && !view.focused);

    workbench.dispatch(project, Command::ToggleTerminal);
    workbench.settle().unwrap();
    let second = host.ptys().wait_for_spawn(2);
    second.output("fresh");
    workbench.settle().unwrap();
    assert_eq!(tabs(&workbench, project), (vec![running("zsh")], 0));
    assert_eq!(screen(&workbench, project), ["fresh"]);
    assert!(terminal(&workbench, project).focused);
}

/// Twenty lines of `let lineN = N;`.
fn app_ts() -> String {
    (1..=20).map(|n| format!("let line{n} = {n};\n")).collect()
}

/// The open file's title and the caret as the status bar shows it.
fn opened(workbench: &Workbench, project: ProjectId) -> Option<(String, String)> {
    let view = workbench.project(project).unwrap();
    Some((view.editor?.title, view.status.caret?))
}

#[test]
fn clicking_a_path_line_column_in_the_output_opens_the_file_there() {
    let host = TestHost::new();
    let fixture = FixtureProject::new().file("src/app.ts", &app_ts()).build();
    let (mut workbench, project, pty) = open(&host, &fixture);
    workbench.dispatch(project, Command::FocusTerminal);
    pty.output("$ tsc\r\nerror at src/app.ts:12:5 - Cannot find name 'x'.");
    workbench.settle().unwrap();

    // A click on the "a" of "app.ts".
    workbench.dispatch(project, Command::OpenTerminalLink { line: 1, column: 13 });
    workbench.settle().unwrap();

    assert_eq!(opened(&workbench, project), Some(("app.ts".into(), "12:5".into())));
    assert!(!terminal(&workbench, project).focused);
}
