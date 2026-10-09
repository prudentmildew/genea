//! Tabs and one split (ticket #31).

use genea_core::{CaretMove::*, Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn project(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
    workbench.settle().unwrap();
}

fn open(path: &str) -> Command {
    Command::OpenFile(path.into())
}

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

#[test]
fn opened_files_show_as_tabs_with_the_last_one_active() {
    let (_fixture, mut workbench, project) = project(&[("a.ts", "a\n"), ("b.ts", "b\n")]);

    run(&mut workbench, project, [open("a.ts")]);
    run(&mut workbench, project, [open("b.ts")]);

    assert_eq!(tabs(&workbench, project), [["a.ts", "[b.ts]"]]);
    assert_eq!(view(&workbench, project).editor.unwrap().lines[0].text, "b");
}
