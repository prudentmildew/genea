//! Cut, copy and paste through the system clipboard (ticket #21). The test
//! host's clipboard stands in for the system one.

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(text: &str) -> (FixtureProject, TestHost, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file("file.ts", text).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("file.ts".into()));
    workbench.settle().unwrap();
    (fixture, host, workbench, project)
}

fn run(workbench: &mut Workbench, project: ProjectId, commands: impl IntoIterator<Item = Command>) {
    for command in commands {
        workbench.dispatch(project, command);
    }
}

fn lines(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
}

#[test]
fn copy_puts_the_selection_on_the_clipboard() {
    let (_fixture, host, mut workbench, project) = open("let abc = 1;\n");

    run(&mut workbench, project, [Command::MoveCaret(WordRight), Command::MoveCaret(Right)]);
    run(&mut workbench, project, [Command::Select(WordRight), Command::Copy]);

    assert_eq!(host.clipboard().text().as_deref(), Some("abc"));
    assert_eq!(lines(&workbench, project), ["let abc = 1;", ""]);
    assert!(!workbench.project(project).unwrap().editor.unwrap().modified);
}

#[test]
fn copy_with_nothing_selected_leaves_the_clipboard_alone() {
    let (_fixture, host, mut workbench, project) = open("abc\n");
    host.clipboard().set_text("before");

    run(&mut workbench, project, [Command::Copy, Command::Cut]);

    assert_eq!(host.clipboard().text().as_deref(), Some("before"));
    assert_eq!(lines(&workbench, project), ["abc", ""]);
}

#[test]
fn cut_moves_the_selection_to_the_clipboard() {
    let (_fixture, host, mut workbench, project) = open("one\ntwo\nthree\n");

    run(&mut workbench, project, [Command::SelectLine { line: 1 }, Command::Cut]);

    assert_eq!(host.clipboard().text().as_deref(), Some("two\n"));
    assert_eq!(lines(&workbench, project), ["one", "three", ""]);
    assert!(workbench.project(project).unwrap().editor.unwrap().modified);
}

#[test]
fn paste_types_the_clipboard_over_the_selection() {
    let (_fixture, host, mut workbench, project) = open("let abc = 1;\n");
    host.clipboard().set_text("first,\nsecond");

    run(&mut workbench, project, [Command::SelectWord { line: 0, column: 5 }, Command::Paste]);

    assert_eq!(lines(&workbench, project), ["let first,", "second = 1;", ""]);
    assert_eq!(workbench.project(project).unwrap().status.caret.unwrap(), "2:7");
}

#[test]
fn paste_into_a_crlf_file_uses_crlf() {
    let (fixture, host, mut workbench, project) = open("a\r\n");
    host.clipboard().set_text("x\ny\n");

    run(&mut workbench, project, [Command::Paste, Command::Save]);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("file.ts"), "x\r\ny\r\na\r\n");
}

#[test]
fn paste_with_no_text_on_the_clipboard_does_nothing() {
    let (_fixture, _host, mut workbench, project) = open("abc\n");

    run(&mut workbench, project, [Command::Paste]);

    assert_eq!(lines(&workbench, project), ["abc", ""]);
    assert!(!workbench.project(project).unwrap().editor.unwrap().modified);
}
