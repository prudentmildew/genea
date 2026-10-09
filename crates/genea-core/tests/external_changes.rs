//! Open editors follow changes made on disk by something other than Genea
//! (ticket #32): a clean buffer reloads, undoably; a dirty one shows a
//! conflict bar; Genea's own saves are neither.

use genea_core::{Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(path: &str, text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(path, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile(path.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

fn editor(workbench: &Workbench, project: ProjectId) -> EditorView {
    workbench.project(project).unwrap().editor.expect("an open editor")
}

fn text(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    editor(workbench, project).lines.into_iter().map(|l| l.text).collect()
}

#[test]
fn a_clean_buffer_reloads_when_its_file_changes_on_disk() {
    let (fixture, mut workbench, project) = open("src/main.ts", "let a = 1;\n");

    fixture.write("src/main.ts", "let a = 2;\nlet b = 3;\n");
    workbench.settle().unwrap();

    assert_eq!(text(&workbench, project), ["let a = 2;", "let b = 3;", ""]);
    assert!(!editor(&workbench, project).modified, "the reloaded buffer matches the disk");
}
