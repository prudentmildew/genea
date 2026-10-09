//! The caret on a read-only file and its position in the status bar
//! (ticket #20).

use genea_core::{CaretMove::*, Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(text: &str, rows: f64) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file("file.ts", text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows });
    workbench.dispatch(project, Command::OpenFile("file.ts".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn moves(workbench: &mut Workbench, project: ProjectId, moves: &[genea_core::CaretMove]) {
    for &m in moves {
        workbench.dispatch(project, Command::MoveCaret(m));
    }
}

fn status_caret(workbench: &Workbench, project: ProjectId) -> String {
    workbench.project(project).unwrap().status.caret.expect("the status bar shows the caret")
}

fn first_visible_line(workbench: &Workbench, project: ProjectId) -> usize {
    workbench.project(project).unwrap().editor.unwrap().lines[0].index
}

#[test]
fn the_status_bar_has_no_caret_without_an_open_file() {
    let fixture = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    assert_eq!(workbench.project(project).unwrap().status.caret, None);
}

#[test]
fn a_newly_opened_file_has_the_caret_at_the_start() {
    let (_fixture, workbench, project) = open("let a = 1;\n", 10.0);

    assert_eq!(status_caret(&workbench, project), "1:1");
}

#[test]
fn arrow_keys_move_the_caret() {
    let (_fixture, mut workbench, project) = open("let a = 1;\nlet b = 2;\n", 10.0);

    moves(&mut workbench, project, &[Down, Right, Right]);

    assert_eq!(status_caret(&workbench, project), "2:3");
}

#[test]
fn left_at_the_start_of_a_line_goes_to_the_end_of_the_previous_line() {
    let (_fixture, mut workbench, project) = open("abc\r\ndef\r\n", 10.0);

    moves(&mut workbench, project, &[Down, Left]);

    assert_eq!(status_caret(&workbench, project), "1:4");
}

#[test]
fn right_at_the_end_of_a_line_goes_to_the_start_of_the_next_line() {
    let (_fixture, mut workbench, project) = open("abc\r\ndef\r\n", 10.0);

    moves(&mut workbench, project, &[LineEnd, Right]);

    assert_eq!(status_caret(&workbench, project), "2:1");
}

#[test]
fn the_caret_stays_inside_the_file() {
    let (_fixture, mut workbench, project) = open("ab\ncd", 10.0);

    moves(&mut workbench, project, &[Left, Up]);
    assert_eq!(status_caret(&workbench, project), "1:1");

    moves(&mut workbench, project, &[DocumentEnd, Right, Down]);
    assert_eq!(status_caret(&workbench, project), "2:3");
}

#[test]
fn moving_up_and_down_keeps_the_column_across_a_shorter_line() {
    let (_fixture, mut workbench, project) = open("long line\nab\nanother long line\n", 10.0);

    moves(&mut workbench, project, &[LineEnd, Down]);
    assert_eq!(status_caret(&workbench, project), "2:3");

    moves(&mut workbench, project, &[Down]);
    assert_eq!(status_caret(&workbench, project), "3:10");
}

#[test]
fn columns_count_grid_cells_so_a_tab_is_several() {
    let (_fixture, mut workbench, project) = open("\tx = 1\n", 10.0);

    moves(&mut workbench, project, &[Right]);

    assert_eq!(status_caret(&workbench, project), "1:5");
    let editor = workbench.project(project).unwrap().editor.unwrap();
    assert_eq!(editor.lines[0].text, "    x = 1");
}

#[test]
fn line_start_and_document_moves() {
    let (_fixture, mut workbench, project) = open("one\ntwo\nthree", 10.0);

    moves(&mut workbench, project, &[DocumentEnd]);
    assert_eq!(status_caret(&workbench, project), "3:6");

    moves(&mut workbench, project, &[LineStart]);
    assert_eq!(status_caret(&workbench, project), "3:1");

    moves(&mut workbench, project, &[DocumentStart]);
    assert_eq!(status_caret(&workbench, project), "1:1");
}

#[test]
fn moving_the_caret_below_the_viewport_scrolls_it_into_view() {
    let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let (_fixture, mut workbench, project) = open(&text, 5.0);

    moves(&mut workbench, project, &[Down; 7]);

    assert_eq!(status_caret(&workbench, project), "8:1");
    // Line 8 (index 7) is the last fully visible row.
    assert_eq!(first_visible_line(&workbench, project), 3);
}

#[test]
fn moving_the_caret_above_the_viewport_scrolls_it_into_view() {
    let text: String = (1..=20).map(|n| format!("line {n}\n")).collect();
    let (_fixture, mut workbench, project) = open(&text, 5.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 10.0 });
    workbench.dispatch(project, Command::MoveCaret(Down));

    assert_eq!(first_visible_line(&workbench, project), 1);
}

#[test]
fn page_down_and_up_move_the_caret_by_a_viewport() {
    let text: String = (1..=50).map(|n| format!("line {n}\n")).collect();
    let (_fixture, mut workbench, project) = open(&text, 10.0);

    moves(&mut workbench, project, &[PageDown]);
    assert_eq!(status_caret(&workbench, project), "11:1");
    assert_eq!(first_visible_line(&workbench, project), 10);

    moves(&mut workbench, project, &[PageUp]);
    assert_eq!(status_caret(&workbench, project), "1:1");
    assert_eq!(first_visible_line(&workbench, project), 0);
}

#[test]
fn clicking_places_the_caret_on_the_nearest_text_position() {
    let (_fixture, mut workbench, project) = open("short\n\tindented\n", 10.0);

    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 3 });
    assert_eq!(status_caret(&workbench, project), "1:4");

    // Past the end of the line.
    workbench.dispatch(project, Command::PlaceCaret { line: 0, column: 40 });
    assert_eq!(status_caret(&workbench, project), "1:6");

    // Inside a tab's cells: the caret goes before the tab.
    workbench.dispatch(project, Command::PlaceCaret { line: 1, column: 2 });
    assert_eq!(status_caret(&workbench, project), "2:1");

    // Below the last line.
    workbench.dispatch(project, Command::PlaceCaret { line: 99, column: 0 });
    assert_eq!(status_caret(&workbench, project), "3:1");
}
