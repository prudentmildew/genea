//! Selecting text by character, word, line, document and mouse, and
//! editing a selection (ticket #21).

use std::ops::Range;

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

fn lines(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.text).collect()
}

/// The highlighted columns of each visible line.
fn highlighted(workbench: &Workbench, project: ProjectId) -> Vec<Vec<Range<usize>>> {
    workbench.project(project).unwrap().editor.unwrap().lines.into_iter().map(|l| l.selections).collect()
}

fn status_caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.unwrap()
}

#[test]
fn shift_arrows_select_characters_and_typing_replaces_them() {
    let (_fixture, mut workbench, project) = open("let abc = 1;\n");

    run(&mut workbench, project, [Command::MoveCaret(LineStart)]);
    run(&mut workbench, project, std::iter::repeat_n(Command::MoveCaret(Right), 4));
    run(&mut workbench, project, std::iter::repeat_n(Command::Select(Right), 3));
    assert_eq!(highlighted(&workbench, project), [vec![4..7], vec![]]);
    assert_eq!(status_caret(&workbench, project), "1:8");

    run(&mut workbench, project, [Command::InsertText("x".into())]);
    assert_eq!(lines(&workbench, project), ["let x = 1;", ""]);
    assert_eq!(highlighted(&workbench, project), [vec![], vec![]]);
}

#[test]
fn left_and_right_collapse_a_selection_to_its_start_and_end() {
    let (_fixture, mut workbench, project) = open("abcdef\n");

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Select(Right), Command::Select(Right)]);
    run(&mut workbench, project, [Command::MoveCaret(Left)]);
    assert_eq!(status_caret(&workbench, project), "1:2");
    assert_eq!(highlighted(&workbench, project), [vec![], vec![]]);

    run(&mut workbench, project, [Command::Select(Right), Command::Select(Right), Command::MoveCaret(Right)]);
    assert_eq!(status_caret(&workbench, project), "1:4");
    assert_eq!(highlighted(&workbench, project), [vec![], vec![]]);
}

#[test]
fn a_selection_over_a_line_break_highlights_one_column_past_the_text() {
    let (_fixture, mut workbench, project) = open("ab\r\ncd\r\nef\r\n");

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Select(Down), Command::Select(Down)]);

    assert_eq!(highlighted(&workbench, project), [vec![1..3], vec![0..3], vec![0..1], vec![]]);
}

#[test]
fn backspace_deletes_a_selection_across_lines() {
    let (_fixture, mut workbench, project) = open("ab\r\ncd\r\nef\r\n");

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Select(Down), Command::Select(Down)]);
    run(&mut workbench, project, [Command::Delete(Left)]);

    assert_eq!(lines(&workbench, project), ["af", ""]);
    assert_eq!(status_caret(&workbench, project), "1:2");
}

#[test]
fn selecting_to_line_and_document_ends() {
    let (_fixture, mut workbench, project) = open("one\ntwo\n");

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Select(LineEnd)]);
    assert_eq!(highlighted(&workbench, project), [vec![1..3], vec![], vec![]]);

    run(&mut workbench, project, [Command::Select(LineStart)]);
    assert_eq!(highlighted(&workbench, project), [vec![0..1], vec![], vec![]]);

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Select(DocumentEnd)]);
    assert_eq!(highlighted(&workbench, project), [vec![1..4], vec![0..4], vec![]]);

    run(&mut workbench, project, [Command::Select(DocumentStart)]);
    assert_eq!(highlighted(&workbench, project), [vec![0..1], vec![], vec![]]);
}

#[test]
fn select_all_selects_the_whole_file() {
    let (_fixture, mut workbench, project) = open("one\ntwo");

    run(&mut workbench, project, [Command::MoveCaret(Down), Command::SelectAll]);

    assert_eq!(highlighted(&workbench, project), [vec![0..4], vec![0..3]]);
    assert_eq!(status_caret(&workbench, project), "2:4");
}
