//! Awkward text and files (ticket #28): the encoding and line ending in the
//! status bar, invalid UTF-8, binary files and files of other types.

use genea_core::{Command, ProjectId, ProjectView, Workbench};
use genea_testkit::{FixtureProject, TestHost};

fn open(path: &str, contents: impl AsRef<[u8]>) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new().file(path, contents).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile(path.into()));
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn view(workbench: &Workbench, project: ProjectId) -> ProjectView {
    workbench.project(project).expect("project is open")
}

#[test]
fn the_status_bar_shows_utf8_and_lf() {
    let (_fixture, workbench, project) = open("main.ts", "let a = 1;\nlet b = 2;\n");

    let status = view(&workbench, project).status;
    assert_eq!(status.encoding.as_deref(), Some("UTF-8"));
    assert_eq!(status.line_ending.as_deref(), Some("LF"));
}

#[test]
fn the_status_bar_shows_crlf_for_a_crlf_file() {
    let (_fixture, workbench, project) = open("main.ts", "let a = 1;\r\nlet b = 2;\r\n");

    assert_eq!(view(&workbench, project).status.line_ending.as_deref(), Some("CRLF"));
}

#[test]
fn the_status_bar_has_no_encoding_or_line_ending_without_an_open_file() {
    let fixture = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    let status = view(&workbench, project).status;
    assert_eq!(status.encoding, None);
    assert_eq!(status.line_ending, None);
}

/// "café" in Latin-1: the é is a lone 0xE9 byte, which isn't valid UTF-8.
const LATIN1: &[u8] = b"let caf\xe9 = 1;\nlet b = 2;\n";

#[test]
fn a_file_with_invalid_utf8_opens_read_only_with_a_notice() {
    let (_fixture, workbench, project) = open("legacy.ts", LATIN1);

    let view = view(&workbench, project);
    let editor = view.editor.expect("the file opens");
    assert!(editor.read_only);
    assert_eq!(editor.lines[0].text, "let caf\u{FFFD} = 1;", "the invalid byte shows as a replacement character");
    assert_eq!(editor.lines[1].text, "let b = 2;");
    let notice = &view.notices.last().expect("a notice explains why").message;
    assert!(notice.contains("legacy.ts") && notice.contains("UTF-8") && notice.contains("read-only"), "{notice}");
}

#[test]
fn editing_a_file_with_invalid_utf8_is_refused() {
    let (fixture, mut workbench, project) = open("legacy.ts", LATIN1);

    for command in [
        Command::InsertText("x".into()),
        Command::NewLine,
        Command::Delete(genea_core::CaretMove::Right),
        Command::SetPreedit("´".into()),
        Command::SelectAll,
        Command::Cut,
        Command::Paste,
        Command::Save,
    ] {
        workbench.dispatch(project, command);
    }
    workbench.settle().unwrap();

    let editor = view(&workbench, project).editor.unwrap();
    assert_eq!(editor.lines[0].text, "let caf\u{FFFD} = 1;");
    assert_eq!(editor.preedit, None);
    assert!(!editor.modified);
    assert_eq!(std::fs::read(fixture.path("legacy.ts")).unwrap(), LATIN1, "the file on disk is untouched");
}

#[test]
fn a_file_with_invalid_utf8_can_still_be_read_and_copied() {
    let host = TestHost::new();
    let fixture = FixtureProject::new().file("legacy.ts", LATIN1).build();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("legacy.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::SelectLine { line: 1 });
    workbench.dispatch(project, Command::Copy);

    assert_eq!(view(&workbench, project).editor.unwrap().caret.line, 2);
    assert_eq!(host.clipboard().text().as_deref(), Some("let b = 2;\n"));
}

#[test]
fn a_valid_utf8_file_is_editable() {
    let (_fixture, workbench, project) = open("main.ts", "let café = 1;\n");

    let editor = view(&workbench, project).editor.unwrap();
    assert!(!editor.read_only);
    assert_eq!(editor.lines[0].text, "let café = 1;");
}
