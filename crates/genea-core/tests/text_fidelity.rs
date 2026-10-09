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

#[test]
fn a_shell_script_and_a_dockerfile_open_as_editable_plain_text() {
    for (path, text, first_line) in [
        ("scripts/build.sh", "#!/bin/sh\nset -eu\n", "#!/bin/sh"),
        ("Dockerfile", "FROM node:24\nRUN pnpm install\n", "FROM node:24"),
    ] {
        let (fixture, mut workbench, project) = open(path, text);

        let editor = view(&workbench, project).editor.unwrap_or_else(|| panic!("{path} opens"));
        assert!(!editor.read_only, "{path}");
        assert_eq!(editor.lines[0].text, first_line);

        workbench.dispatch(project, Command::InsertText("# ".into()));
        workbench.dispatch(project, Command::Save);
        workbench.settle().unwrap();
        assert_eq!(fixture.read(path), format!("# {text}"));
    }
}

/// The start of a PNG: binary, with NUL bytes early on.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\x00\x00\x00\rIHDR\x00\x00\x00\x10\x00\x00\x00\x10\x08\x06\x00\x00\x00";

#[test]
fn a_binary_file_shows_a_notice_instead_of_an_editor() {
    let (_fixture, workbench, project) = open("logo.png", PNG);

    let view = view(&workbench, project);
    assert_eq!(view.editor, None);
    let notice = &view.notices.last().expect("a notice explains why").message;
    assert!(notice.contains("logo.png") && notice.contains("binary"), "{notice}");
}

#[test]
fn opening_a_binary_file_leaves_the_open_file_in_the_editor() {
    let fixture = FixtureProject::new().file("main.ts", "let a = 1;\n").file("logo.png", PNG).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("x".into()));

    workbench.dispatch(project, Command::OpenFile("logo.png".into()));
    workbench.settle().unwrap();

    let editor = view(&workbench, project).editor.expect("the text file stays open");
    assert_eq!(editor.title, "main.ts");
    assert_eq!(editor.lines[0].text, "xlet a = 1;", "its unsaved edit is kept");
}
