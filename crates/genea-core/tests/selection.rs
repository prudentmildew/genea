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
fn option_right_moves_to_the_end_of_each_word_and_punctuation_run() {
    let (_fixture, mut workbench, project) = open("let fooBar = baz(1);\nnext\n");
    let mut stops = Vec::new();
    for _ in 0..8 {
        run(&mut workbench, project, [Command::MoveCaret(WordRight)]);
        stops.push(status_caret(&workbench, project));
    }

    assert_eq!(stops, ["1:4", "1:11", "1:13", "1:17", "1:18", "1:19", "1:21", "2:1"]);
}

#[test]
fn option_left_moves_to_the_start_of_each_word_and_punctuation_run() {
    let (_fixture, mut workbench, project) = open("first\nlet fooBar = baz(1);\n");
    run(&mut workbench, project, [Command::MoveCaret(Down), Command::MoveCaret(LineEnd)]);
    let mut stops = Vec::new();
    for _ in 0..8 {
        run(&mut workbench, project, [Command::MoveCaret(WordLeft)]);
        stops.push(status_caret(&workbench, project));
    }

    assert_eq!(stops, ["2:19", "2:18", "2:17", "2:14", "2:12", "2:5", "2:1", "1:6"]);
}

#[test]
fn option_shift_arrows_select_words_and_option_backspace_deletes_one() {
    let (_fixture, mut workbench, project) = open("let fooBar = 1;\n");

    run(&mut workbench, project, [Command::MoveCaret(WordRight), Command::Select(WordRight)]);
    assert_eq!(highlighted(&workbench, project), [vec![3..10], vec![]]);

    run(&mut workbench, project, [Command::MoveCaret(Right), Command::Delete(WordLeft)]);
    assert_eq!(lines(&workbench, project), ["let  = 1;", ""]);

    run(&mut workbench, project, [Command::Delete(WordRight)]);
    assert_eq!(lines(&workbench, project), ["let  1;", ""]);
}

#[test]
fn dragging_or_shift_clicking_extends_the_selection_from_the_click() {
    let (_fixture, mut workbench, project) = open("one two\nthree four\n");

    run(&mut workbench, project, [Command::PlaceCaret { line: 0, column: 4 }]);
    run(&mut workbench, project, [Command::ExtendSelection { line: 1, column: 5 }]);
    assert_eq!(highlighted(&workbench, project), [vec![4..8], vec![0..5], vec![]]);
    assert_eq!(status_caret(&workbench, project), "2:6");

    // Dragging back above the press point flips the selection.
    run(&mut workbench, project, [Command::ExtendSelection { line: 0, column: 1 }]);
    assert_eq!(highlighted(&workbench, project), [vec![1..4], vec![], vec![]]);

    // A plain click drops it.
    run(&mut workbench, project, [Command::PlaceCaret { line: 1, column: 0 }]);
    assert_eq!(highlighted(&workbench, project), [vec![], vec![], vec![]]);
}

#[test]
fn double_click_selects_a_word() {
    let (_fixture, mut workbench, project) = open("let fooBar = 1;\n");

    run(&mut workbench, project, [Command::SelectWord { line: 0, column: 6 }]);

    assert_eq!(highlighted(&workbench, project), [vec![4..10], vec![]]);
    assert_eq!(status_caret(&workbench, project), "1:11");
}

#[test]
fn triple_click_selects_a_line_with_its_line_break() {
    let (_fixture, mut workbench, project) = open("one\ntwo\nthree\n");

    run(&mut workbench, project, [Command::SelectLine { line: 1 }]);
    assert_eq!(highlighted(&workbench, project), [vec![], vec![0..4], vec![], vec![]]);
    assert_eq!(status_caret(&workbench, project), "3:1");

    run(&mut workbench, project, [Command::Delete(Left)]);
    assert_eq!(lines(&workbench, project), ["one", "three", ""]);
}

#[test]
fn select_all_selects_the_whole_file() {
    let (_fixture, mut workbench, project) = open("one\ntwo");

    run(&mut workbench, project, [Command::MoveCaret(Down), Command::SelectAll]);

    assert_eq!(highlighted(&workbench, project), [vec![0..4], vec![0..3]]);
    assert_eq!(status_caret(&workbench, project), "2:4");
}
