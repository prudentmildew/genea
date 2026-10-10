//! Session restore (ticket #59): what a project's window showed comes back
//! after a restart, and the projects that were open when Genea quit reopen.
//!
//! A restart is a second workbench on the same test host (they share the
//! application-support folder). `Workbench::quit` is what the app calls as
//! it quits: it writes the session at once.

use std::time::Duration;

use genea_core::{CaretMove, Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Session state is written in the background a short pause after a
/// change. Advancing past it and settling waits for the write.
const SAVED: Duration = Duration::from_secs(5);

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).unwrap()
}

/// Each side's tab titles, with the active one in brackets.
fn tabs(workbench: &Workbench, project: ProjectId) -> Vec<Vec<String>> {
    view(workbench, project)
        .panes
        .iter()
        .map(|pane| {
            pane.tabs
                .iter()
                .enumerate()
                .map(|(i, tab)| if pane.active == Some(i) { format!("[{}]", tab.title) } else { tab.title.clone() })
                .collect()
        })
        .collect()
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
        workbench.settle().unwrap();
    }
}

fn open(path: &str) -> Command {
    Command::OpenFile(path.into())
}

/// Quits `workbench` and starts Genea again on the same host, restoring
/// the session. Returns the new workbench and the projects it reopened.
fn restart(mut workbench: Workbench, host: &TestHost) -> (Workbench, Vec<ProjectId>) {
    workbench.quit();
    drop(workbench);
    let mut restarted = Workbench::new(host.shared());
    let projects = restarted.restore_session();
    restarted.settle().unwrap();
    (restarted, projects)
}

#[test]
fn quitting_and_relaunching_reopens_the_project_with_its_tabs() {
    let fixture = FixtureProject::new().file("a.ts", "let a = 1;\n").file("b.ts", "let b = 2;\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("a.ts"), open("b.ts")]);

    let (restarted, projects) = restart(workbench, &host);

    let [project] = projects[..] else { panic!("one project reopens, not {projects:?}") };
    assert_eq!(view(&restarted, project).root, fixture.root().canonicalize().unwrap());
    assert_eq!(tabs(&restarted, project), [["a.ts", "[b.ts]"]]);
    assert_eq!(view(&restarted, project).editor.unwrap().lines[0].text, "let b = 2;");
}

/// `count` numbered lines.
fn numbered(count: usize) -> String {
    (1..=count).map(|n| format!("line {n}\n")).collect()
}

#[test]
fn the_split_carets_selections_and_scroll_positions_come_back() {
    let fixture = FixtureProject::new().file("a.ts", &numbered(100)).file("b.ts", &numbered(100)).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(
        &mut workbench,
        project,
        [
            Command::SetViewport { rows: 10.0 },
            open("a.ts"),
            Command::PlaceCaret { line: 40, column: 2 },
            Command::Select(CaretMove::Right),
            Command::AddCaret { line: 42, column: 0 },
            Command::SplitRight,
            open("b.ts"),
            Command::ScrollPane { pane: 1, rows: 20.0 },
            Command::FocusPane(0),
        ],
    );
    let before = view(&workbench, project);
    assert_eq!(tabs(&workbench, project), [vec!["[a.ts]"], vec!["a.ts", "[b.ts]"]]);

    let (mut restarted, projects) = restart(workbench, &host);
    let project = projects[0];
    run(&mut restarted, project, [Command::SetViewport { rows: 10.0 }]);

    let after = view(&restarted, project);
    assert_eq!(tabs(&restarted, project), [vec!["[a.ts]"], vec!["a.ts", "[b.ts]"]]);
    assert_eq!(after.focused_pane, 0);
    let editor = after.editor.clone().unwrap();
    assert_eq!((editor.caret.line, editor.caret.column), (42, 0));
    assert_eq!(editor.carets.len(), 2);
    assert_eq!(after.panes[1].editor.as_ref().unwrap().scroll_top, 20.0);
    assert_eq!(after.panes, before.panes);
}

#[test]
fn the_session_is_saved_in_the_background_so_it_survives_a_crash() {
    let fixture = FixtureProject::new().file("a.ts", "a\n").file("b.ts", "b\n").build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    run(&mut workbench, project, [open("a.ts"), open("b.ts")]);
    host.clock().advance(SAVED);
    workbench.settle().unwrap();
    // No quit: Genea stopped.
    drop(workbench);

    let mut restarted = Workbench::new(host.shared());
    let projects = restarted.restore_session();
    restarted.settle().unwrap();

    assert_eq!(projects.len(), 1);
    assert_eq!(tabs(&restarted, projects[0]), [["a.ts", "[b.ts]"]]);
}

#[test]
fn a_closed_project_isnt_reopened_but_opens_again_as_it_was() {
    let fixture = FixtureProject::new().file("a.ts", "a\n").build();
    let other = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.open_project(other.root()).unwrap();
    run(&mut workbench, project, [open("a.ts")]);
    workbench.close_project(project);
    workbench.settle().unwrap();

    let (mut restarted, projects) = restart(workbench, &host);
    assert_eq!(projects.len(), 1);
    assert_eq!(view(&restarted, projects[0]).root, other.root().canonicalize().unwrap());

    let reopened = restarted.open_project(fixture.root()).unwrap();
    restarted.settle().unwrap();
    assert_eq!(tabs(&restarted, reopened), [["[a.ts]"]]);
}

#[test]
fn quitting_with_no_project_open_starts_with_the_welcome() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.close_project(project);

    let (restarted, projects) = restart(workbench, &host);

    assert_eq!(projects, []);
    assert!(restarted.welcome().is_some());
}

#[test]
fn quitting_saves_the_recent_projects_at_once() {
    let fixture = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.close_project(project);
    // Quit well within the recent list's save delay.

    let (restarted, _) = restart(workbench, &host);

    let welcome = restarted.welcome().unwrap();
    assert_eq!(welcome.recent_projects[0].root, fixture.root().canonicalize().unwrap());
}
