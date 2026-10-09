//! Scrolling the editor surface (ticket #20).

use genea_core::{Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// A ten-line file, "line 1" to "line 10", open in a viewport of `rows`.
fn ten_lines_in_viewport(rows: f64) -> (FixtureProject, Workbench, ProjectId) {
    let text: String = (1..=10).map(|n| format!("line {n}\n")).collect();
    let fixture = FixtureProject::new().file("ten.txt", text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows });
    workbench.dispatch(project, Command::OpenFile("ten.txt".into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn editor(workbench: &Workbench, project: ProjectId) -> EditorView {
    workbench.project(project).unwrap().editor.unwrap()
}

fn visible(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    editor(workbench, project).lines.into_iter().map(|l| l.text).collect()
}

#[test]
fn only_the_lines_in_the_viewport_are_visible() {
    let (_fixture, workbench, project) = ten_lines_in_viewport(3.0);

    assert_eq!(visible(&workbench, project), ["line 1", "line 2", "line 3"]);
}

#[test]
fn scrolling_down_shows_later_lines() {
    let (_fixture, mut workbench, project) = ten_lines_in_viewport(3.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 4.0 });

    assert_eq!(visible(&workbench, project), ["line 5", "line 6", "line 7"]);
}

#[test]
fn a_partly_scrolled_viewport_includes_the_partly_visible_line() {
    let (_fixture, mut workbench, project) = ten_lines_in_viewport(3.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 1.5 });

    let editor = editor(&workbench, project);
    assert_eq!(editor.scroll_top, 1.5);
    let lines: Vec<_> = editor.lines.iter().map(|l| (l.index, l.text.as_str())).collect();
    assert_eq!(lines, [(1, "line 2"), (2, "line 3"), (3, "line 4"), (4, "line 5")]);
}

#[test]
fn scrolling_stops_at_the_last_line() {
    let (_fixture, mut workbench, project) = ten_lines_in_viewport(3.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 100.0 });

    // Ten lines plus the empty line after the final newline.
    assert_eq!(visible(&workbench, project), ["line 9", "line 10", ""]);
}

#[test]
fn scrolling_up_stops_at_the_first_line() {
    let (_fixture, mut workbench, project) = ten_lines_in_viewport(3.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 2.0 });
    workbench.dispatch(project, Command::ScrollBy { rows: -5.0 });

    assert_eq!(visible(&workbench, project), ["line 1", "line 2", "line 3"]);
}

#[test]
fn a_file_shorter_than_the_viewport_doesnt_scroll() {
    let (_fixture, mut workbench, project) = ten_lines_in_viewport(20.0);

    workbench.dispatch(project, Command::ScrollBy { rows: 3.0 });

    assert_eq!(editor(&workbench, project).scroll_top, 0.0);
    assert_eq!(visible(&workbench, project).len(), 11);
}
