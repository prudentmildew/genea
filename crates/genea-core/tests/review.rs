//! Review (ticket #53): changes made on disk by anything but Genea are
//! listed in Changes against the review baseline, and Keep or Revert settles
//! them, per file or for all files.

use std::path::PathBuf;

use genea_core::{ChangeItem, ChangeKind, Command, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// Opens a project with these files and waits for the first snapshot.
fn open(files: &[(&str, &str)]) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The Changes list as (path, kind).
fn changes(workbench: &Workbench, project: ProjectId) -> Vec<(PathBuf, ChangeKind)> {
    items(workbench, project).into_iter().map(|c| (c.path, c.kind)).collect()
}

fn items(workbench: &Workbench, project: ProjectId) -> Vec<ChangeItem> {
    workbench.project(project).unwrap().changes.to_vec()
}

fn banner(workbench: &Workbench, project: ProjectId) -> Option<String> {
    workbench.project(project).unwrap().review_banner
}

#[test]
fn an_external_edit_is_listed_in_changes_and_the_banner_shows() {
    let (fixture, mut workbench, project) = open(&[("src/main.ts", "let a = 1;\n"), ("README.md", "hi\n")]);
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(banner(&workbench, project), None);

    fixture.write("src/main.ts", "let a = 2;\n");
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), [("src/main.ts".into(), ChangeKind::Modified)]);
    assert_eq!(banner(&workbench, project).as_deref(), Some("1 file changed outside Genea"));
}

#[test]
fn created_and_deleted_files_are_listed_and_a_rename_is_both() {
    let (fixture, mut workbench, project) = open(&[("a.ts", "a\n"), ("old.ts", "old\n"), ("gone.ts", "gone\n")]);

    fixture.write("src/new.ts", "new\n");
    fixture.remove("gone.ts");
    std::fs::rename(fixture.path("old.ts"), fixture.path("renamed.ts")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(
        changes(&workbench, project),
        [
            ("gone.ts".into(), ChangeKind::Deleted),
            ("old.ts".into(), ChangeKind::Deleted),
            ("renamed.ts".into(), ChangeKind::Created),
            ("src/new.ts".into(), ChangeKind::Created),
        ]
    );
    assert_eq!(banner(&workbench, project).as_deref(), Some("4 files changed outside Genea"));
}

#[test]
fn a_folder_moved_out_or_in_lists_every_file_in_it() {
    let (fixture, mut workbench, project) = open(&[("lib/a.ts", "a\n"), ("lib/b.ts", "b\n")]);
    let outside = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(outside.path().join("pkg/src")).unwrap();
    std::fs::write(outside.path().join("pkg/src/c.ts"), "c\n").unwrap();

    std::fs::rename(fixture.path("lib"), outside.path().join("lib")).unwrap();
    std::fs::rename(outside.path().join("pkg"), fixture.path("pkg")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(
        changes(&workbench, project),
        [
            ("lib/a.ts".into(), ChangeKind::Deleted),
            ("lib/b.ts".into(), ChangeKind::Deleted),
            ("pkg/src/c.ts".into(), ChangeKind::Created),
        ]
    );
}

#[test]
fn edits_saved_in_genea_are_never_listed_and_move_the_baseline() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "let a = 1;\n")]);
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();

    for text in ["x", "y", "z"] {
        workbench.dispatch(project, Command::InsertText(text.into()));
        workbench.dispatch(project, Command::Save);
    }
    workbench.settle().unwrap();
    assert_eq!(fixture.read("main.ts"), "xyzlet a = 1;\n");
    assert_eq!(changes(&workbench, project), []);
    assert_eq!(banner(&workbench, project), None);

    // The baseline is the saved text now, so going back is a change.
    fixture.write("main.ts", "let a = 1;\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);
}

#[test]
fn saving_a_file_with_a_pending_change_keeps_it_listed_against_the_old_baseline() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "one\n")]);
    fixture.write("main.ts", "two\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("three ".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();
    assert_eq!(fixture.read("main.ts"), "one\n", "the baseline is still from before the external change");
    assert_eq!(changes(&workbench, project), []);
}

#[test]
fn creating_the_config_from_genea_is_not_listed() {
    let (fixture, mut workbench, project) = open(&[("main.ts", "a\n")]);

    workbench.dispatch(project, Command::OpenConfig);
    workbench.settle().unwrap();

    assert_eq!(fixture.read("genea.jsonc"), "{}\n");
    assert_eq!(changes(&workbench, project), []);
}
