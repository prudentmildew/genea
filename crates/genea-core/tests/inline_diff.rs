//! The inline diff (ticket #55): a file opened from Changes shows its
//! removed and added lines interleaved in the editor, against its review
//! baseline, with Keep and Revert.

use genea_core::{Command, EditorView, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Opens a project with these files and waits for the first snapshot.
fn open(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

fn editor(workbench: &Workbench, project: ProjectId) -> EditorView {
    workbench.project(project).unwrap().editor.expect("an open editor")
}

/// The editor's rows as the user reads them, top to bottom: `-` for a
/// removed line, `+` for an added one, a space for an unchanged one.
fn rows(view: &EditorView) -> Vec<String> {
    let diff = view.inline_diff.as_ref().expect("an inline diff");
    let mut rows: Vec<(usize, String)> = view
        .lines
        .iter()
        .map(|line| {
            let mark = if diff.added.contains(&line.index) { '+' } else { ' ' };
            (line.row, format!("{mark}{}", line.text))
        })
        .chain(diff.removed.iter().map(|removed| (removed.row, format!("-{}", removed.text))))
        .collect();
    rows.sort();
    rows.into_iter().map(|(_, text)| text).collect()
}

#[test]
fn opening_a_changed_file_from_changes_shows_removed_and_added_lines_interleaved() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\ntwo\nthree\nfour\n")]);
    fixture.write("main.ts", "one\n2\nthree\nfour\nfive\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("main.ts".into()));
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.path, std::path::Path::new("main.ts"));
    assert_eq!(rows(&view), [" one", "-two", "+2", " three", " four", "+five", " "]);
}

#[test]
fn a_created_file_shows_as_all_added() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n")]);
    fixture.write("src/new.ts", "alpha\nbeta\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("src/new.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(rows(&editor(&workbench, project)), ["+alpha", "+beta", " "]);
}

#[test]
fn a_deleted_file_opens_read_only_and_shows_as_all_removed() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n"), ("old.ts", "first\nsecond\n")]);
    fixture.remove("old.ts");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenChange("old.ts".into()));
    workbench.settle().unwrap();

    let view = editor(&workbench, project);
    assert_eq!(view.path, std::path::Path::new("old.ts"));
    assert!(view.read_only);
    assert_eq!(rows(&view), ["-first", "-second", " "]);
}
