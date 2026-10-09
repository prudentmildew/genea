//! Wide characters on the grid (ticket #28): CJK and emoji take two grid
//! columns (their Unicode width), and the caret, clicks and selections
//! count them that way.

use genea_core::{CaretMove::*, Command, GridPiece, ProjectId, Workbench, grid_pieces};
use genea_testkit::{FixtureProject, TestHost};

fn open(text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file("i18n.ts", text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("i18n.ts".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn status_caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.unwrap()
}

#[test]
fn cjk_characters_and_emoji_take_two_columns_each() {
    let (_fixture, mut workbench, project) = open("a中文b😀c\n");

    let mut columns = Vec::new();
    for _ in 0..6 {
        workbench.dispatch(project, Command::MoveCaret(Right));
        columns.push(workbench.project(project).unwrap().editor.unwrap().caret.column);
    }

    assert_eq!(columns, [1, 3, 5, 6, 8, 9]);
    assert_eq!(status_caret(&workbench, project), "1:10");
}

#[test]
fn a_click_on_either_half_of_a_wide_character_lands_before_it() {
    let (_fixture, mut workbench, project) = open("a中b\n");

    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 2 });
    assert_eq!(status_caret(&workbench, project), "1:2");

    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 3 });
    assert_eq!(status_caret(&workbench, project), "1:4");
}

#[test]
fn a_selected_wide_character_covers_two_columns() {
    let (_fixture, mut workbench, project) = open("a中b\n");

    workbench.dispatch(project, Command::MoveCaret(Right));
    workbench.dispatch(project, Command::Select(Right));

    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.lines[0].selections, [1..3]);
}

#[test]
fn up_and_down_keep_the_column_across_wide_characters() {
    let (_fixture, mut workbench, project) = open("abcdef\n中文字\n");

    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 4 });
    workbench.dispatch(project, Command::MoveCaret(Down));
    assert_eq!(status_caret(&workbench, project), "2:5", "after 中文, at column 4");

    workbench.dispatch(project, Command::MoveCaret(Up));
    assert_eq!(status_caret(&workbench, project), "1:5");
}

fn pieces(text: &str) -> Vec<(usize, &str)> {
    grid_pieces(text).into_iter().map(|GridPiece { column, text }| (column, text)).collect()
}

#[test]
fn grid_text_splits_so_that_every_wide_character_starts_at_its_own_column() {
    assert_eq!(pieces("a中文b😀c"), [(0, "a"), (1, "中"), (3, "文"), (5, "b"), (6, "😀"), (8, "c")]);
}

#[test]
fn narrow_text_stays_in_one_piece() {
    assert_eq!(pieces("let café = 1; // ok"), [(0, "let café = 1; // ok")]);
    assert_eq!(pieces(""), []);
}

#[test]
fn zero_width_characters_stay_with_the_character_before_them() {
    // e + a combining acute accent; a variation selector after a wide character.
    assert_eq!(pieces("e\u{301}x中\u{FE0F}y"), [(0, "e\u{301}x"), (2, "中\u{FE0F}"), (4, "y")]);
}
