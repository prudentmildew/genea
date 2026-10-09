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
