//! Session restore (ticket #59): what a project's window showed comes back
//! after a restart, and the projects that were open when Genea quit reopen.
//!
//! A restart is a second workbench on the same test host (they share the
//! application-support folder). `Workbench::quit` is what the app calls as
//! it quits: it writes the session at once.

use genea_core::{Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

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
