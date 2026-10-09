//! Typing into a buffer: text, Backspace, Delete and Return (ticket #21).

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file("file.ts", text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 10.0 });
    workbench.dispatch(project, Command::OpenFile("file.ts".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

fn type_text(text: &str) -> Command {
    Command::InsertText(text.into())
}

/// The visible lines' grid text.
fn lines(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
}

fn status_caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.unwrap()
}

#[test]
fn typing_inserts_text_at_the_caret() {
    let (_fixture, mut workbench, project) = open("let = 1;\n");

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::MoveCaret(Right), Command::MoveCaret(Right)]);
    run(&mut workbench, project, [type_text(" "), type_text("a")]);

    assert_eq!(lines(&workbench, project), ["let a = 1;", ""]);
    assert_eq!(status_caret(&workbench, project), "1:6");
}

#[test]
fn backspace_deletes_the_character_before_the_caret() {
    let (_fixture, mut workbench, project) = open("let ab = 1;\n");

    run(&mut workbench, project, std::iter::repeat_n(Command::MoveCaret(Right), 6));
    run(&mut workbench, project, [Command::Delete(Left)]);

    assert_eq!(lines(&workbench, project), ["let a = 1;", ""]);
    assert_eq!(status_caret(&workbench, project), "1:6");
}
