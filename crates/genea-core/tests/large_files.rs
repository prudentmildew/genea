//! Large files (ticket #27): a file over 5 MB opens with no syntax
//! highlighting and no language intelligence, and the status bar says why.

use genea_core::{Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

const MB: usize = 1024 * 1024;

/// A highlightable line of TypeScript, 41 bytes with its line break.
const LINE: &str = "const greeting: string = \"hi\"; // hello\n";

/// TypeScript of exactly `bytes` bytes: whole copies of [`LINE`], padded
/// with a comment line at the end.
fn typescript(bytes: usize) -> String {
    let mut text = LINE.repeat(bytes / LINE.len());
    let rest = bytes - text.len();
    if rest > 0 {
        text.push_str(&"/".repeat(rest - 1));
        text.push('\n');
    }
    assert_eq!(text.len(), bytes);
    text
}

fn open_file(name: &str, text: &str) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(name, text).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::SetViewport { rows: 20.0 });
    workbench.dispatch(project, Command::OpenFile(name.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

#[test]
fn a_file_over_5_mb_opens_without_highlighting_and_the_status_bar_says_why() {
    let text = typescript(5 * MB + 1);
    let (_fixture, workbench, project) = open_file("big.ts", &text);

    let view = workbench.project(project).unwrap();
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, LINE.trim_end());
    assert_eq!(editor.lines.len(), 20);
    assert_eq!(editor.line_count, text.lines().count() + 1);
    assert!(editor.lines.iter().all(|line| line.highlights.is_empty()), "no highlighting");
    assert!(!editor.read_only);
    let reason = view.status.large_file.expect("a status-bar item explains the missing highlighting");
    assert!(reason.contains("5 MB") && reason.contains("highlighting"), "{reason:?}");
}

#[test]
fn a_file_of_exactly_5_mb_is_highlighted_as_usual() {
    let (_fixture, workbench, project) = open_file("big.ts", &typescript(5 * MB));

    let view = workbench.project(project).unwrap();
    assert!(!view.editor.unwrap().lines[0].highlights.is_empty(), "highlighted");
    assert_eq!(view.status.large_file, None);
}

#[test]
fn a_large_file_can_be_edited_and_saved_and_stays_unhighlighted() {
    let text = typescript(5 * MB + 1);
    let (fixture, mut workbench, project) = open_file("big.ts", &text);

    workbench.dispatch(project, Command::SelectAll);
    workbench.dispatch(project, Command::InsertText("let small = 1;".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("big.ts"), "let small = 1;");
    let view = workbench.project(project).unwrap();
    let editor = view.editor.unwrap();
    assert_eq!(editor.lines[0].text, "let small = 1;");
    assert!(editor.lines[0].highlights.is_empty(), "still no highlighting");
    assert!(view.status.large_file.is_some(), "still a large file until reopened");
}

#[test]
fn the_status_bar_item_follows_the_focused_tab() {
    let fixture = FixtureProject::new().file("big.ts", typescript(5 * MB + 1)).file("small.ts", LINE).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("big.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFile("small.ts".into()));
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.status.large_file, None);
    assert!(!view.editor.unwrap().lines[0].highlights.is_empty());

    workbench.dispatch(project, Command::OpenFile("big.ts".into()));
    assert!(workbench.project(project).unwrap().status.large_file.is_some());
}
