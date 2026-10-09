//! Saving a buffer to disk and the unsaved state (ticket #21).

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
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

#[test]
fn save_writes_the_buffer_to_disk() {
    let (fixture, mut workbench, project) = open("src/main.ts", "let a = 1;\n");

    run(&mut workbench, project, [Command::MoveCaret(LineEnd), Command::InsertText(" // one".into()), Command::Save]);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("src/main.ts"), "let a = 1; // one\n");
}

fn modified(workbench: &Workbench, project: ProjectId) -> bool {
    workbench.project(project).unwrap().editor.unwrap().modified
}

#[test]
fn an_edited_file_shows_as_unsaved_until_it_is_saved() {
    let (_fixture, mut workbench, project) = open("main.ts", "let a = 1;\n");
    assert!(!modified(&workbench, project), "a freshly opened file is saved");

    run(&mut workbench, project, [Command::InsertText("x".into())]);
    assert!(modified(&workbench, project), "an edit makes it unsaved");

    run(&mut workbench, project, [Command::Save]);
    workbench.settle().unwrap();
    assert!(!modified(&workbench, project), "saving makes it saved again");
}

#[test]
fn an_edit_made_while_saving_stays_unsaved() {
    let (fixture, mut workbench, project) = open("main.ts", "a\n");

    run(&mut workbench, project, [Command::InsertText("1".into()), Command::Save, Command::InsertText("2".into())]);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("main.ts"), "1a\n");
    assert!(modified(&workbench, project));
}
