//! Bracket matching (ticket #25): the bracket at the caret and the one that
//! matches it are highlighted, as found in the syntax tree.

use genea_core::{Caret, Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open_file(name: &str, text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(name, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile(name.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn brackets_at(workbench: &mut Workbench, project: ProjectId, line: usize, column: usize) -> Vec<Caret> {
    workbench.dispatch(project, Command::PlaceCaret { line, column });
    workbench.project(project).unwrap().editor.unwrap().brackets
}

fn cell(line: usize, column: usize) -> Caret {
    Caret { line, column }
}

#[test]
fn a_bracket_after_the_caret_and_its_match_are_highlighted() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "f(a[0], { b });\n");

    assert_eq!(brackets_at(&mut workbench, project, 0, 1), [cell(0, 1), cell(0, 13)]);
    assert_eq!(brackets_at(&mut workbench, project, 0, 8), [cell(0, 8), cell(0, 12)]);
}

#[test]
fn a_bracket_before_the_caret_counts_when_none_is_after_it() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "f(a[0], { b });\n");

    assert_eq!(brackets_at(&mut workbench, project, 0, 6), [cell(0, 3), cell(0, 5)]);
    assert_eq!(brackets_at(&mut workbench, project, 0, 14), [cell(0, 1), cell(0, 13)]);
}

#[test]
fn matching_brackets_can_be_lines_apart() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "if (a) {\n  b();\n}\n");

    assert_eq!(brackets_at(&mut workbench, project, 2, 0), [cell(0, 7), cell(2, 0)]);
}

#[test]
fn nothing_is_highlighted_away_from_brackets() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let abc = 1;\n");

    assert!(brackets_at(&mut workbench, project, 0, 5).is_empty());
}

#[test]
fn brackets_inside_strings_and_comments_are_not_matched() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "let s = \"(\"; // )\n");

    assert!(brackets_at(&mut workbench, project, 0, 9).is_empty());
    assert!(brackets_at(&mut workbench, project, 0, 16).is_empty());
}

#[test]
fn json_and_css_brackets_match_too() {
    let (_fixture, mut workbench, project) = open_file("data.json", "{\"a\": [1]}\n");
    assert_eq!(brackets_at(&mut workbench, project, 0, 6), [cell(0, 6), cell(0, 8)]);

    let (_fixture, mut workbench, project) = open_file("style.css", "a { color: red; }\n");
    assert_eq!(brackets_at(&mut workbench, project, 0, 2), [cell(0, 2), cell(0, 16)]);
}

#[test]
fn the_match_follows_edits_once_the_reparse_lands() {
    let (_fixture, mut workbench, project) = open_file("main.ts", "f();\n");
    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 2 });
    workbench.dispatch(project, Command::InsertText("1, 2".into()));
    workbench.settle().unwrap();

    assert_eq!(brackets_at(&mut workbench, project, 0, 1), [cell(0, 1), cell(0, 6)]);
}
