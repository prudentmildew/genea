//! Opening a project and reading a file in it (ticket #20).

use genea_core::{Command, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn visible_text(workbench: &Workbench, project: genea_core::ProjectId) -> Vec<String> {
    let view = workbench.project(project).expect("project is open");
    let editor = view.editor.expect("an editor is open");
    editor.lines.into_iter().map(|line| line.text).collect()
}

#[test]
fn an_opened_file_shows_its_lines() {
    let fixture = FixtureProject::new()
        .file("src/main.ts", "const a = 1;\nconst b = 2;\n")
        .build();
    let mut workbench = Workbench::new(TestHost::new().shared());

    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(visible_text(&workbench, project), ["const a = 1;", "const b = 2;", ""]);
}
