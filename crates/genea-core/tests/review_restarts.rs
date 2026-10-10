//! Review across restarts (ticket #54): the review baseline survives
//! closing a project and restarting Genea, and changes made while the
//! project was closed are listed in Changes when it opens again.

use std::path::PathBuf;

use genea_core::{ChangeKind, ProjectId, Workbench};
use genea_testkit::{FixtureProject, TestHost};

/// A project with these files, opened once so its baseline is snapshotted.
fn first_open(files: &[(&str, &str)]) -> (FixtureProject, TestHost, Workbench, ProjectId) {
    let fixture = files.iter().fold(FixtureProject::new(), |f, (path, text)| f.file(path, text)).build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, host, workbench, project)
}

/// Quits Genea and starts it again on the same support folder, opening the
/// project.
fn restart(host: &TestHost, workbench: Workbench, fixture: &FixtureProject) -> (Workbench, ProjectId) {
    drop(workbench);
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (workbench, project)
}

/// Waits until the file system has reported every change made so far, so
/// a watcher started afterwards doesn't see them: FSEvents may still deliver
/// a change made just before a stream started. It settles a scratch project
/// (whose watcher's cookie comes after those changes) and closes it.
fn let_changes_pass(workbench: &mut Workbench) {
    let scratch = FixtureProject::new().file("scratch.ts", "\n").build();
    let project = workbench.open_project(scratch.root()).unwrap();
    workbench.settle().unwrap();
    workbench.close_project(project);
}

/// The Changes list as (path, kind).
fn changes(workbench: &Workbench, project: ProjectId) -> Vec<(PathBuf, ChangeKind)> {
    workbench.project(project).unwrap().changes.iter().map(|c| (c.path.clone(), c.kind)).collect()
}

#[test]
fn a_file_edited_while_the_project_was_closed_is_listed_when_it_opens_again() {
    let (fixture, _host, mut workbench, project) = first_open(&[("src/main.ts", "let a = 1;\n"), ("README.md", "hi\n")]);
    workbench.close_project(project);

    fixture.write("src/main.ts", "let a = 22;\n");
    let_changes_pass(&mut workbench);
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();

    assert_eq!(changes(&workbench, project), [("src/main.ts".into(), ChangeKind::Modified)]);
    assert_eq!(workbench.project(project).unwrap().review_banner.as_deref(), Some("1 file changed outside Genea"));
}
