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
