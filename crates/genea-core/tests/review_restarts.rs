//! Review across restarts (ticket #54): the review baseline survives
//! closing a project and restarting Genea, and changes made while the
//! project was closed are listed in Changes when it opens again.

use std::path::PathBuf;

use genea_core::{ChangeKind, Command, ProjectId, Workbench};
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

/// Quits Genea, runs `while_closed`, and starts Genea again on the same
/// support folder, opening the project.
fn restart(
    host: &TestHost,
    workbench: Workbench,
    fixture: &FixtureProject,
    while_closed: impl FnOnce(),
) -> (Workbench, ProjectId) {
    drop(workbench);
    while_closed();
    let mut workbench = Workbench::new(host.shared());
    let_changes_pass(&mut workbench);
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

#[test]
fn unreviewed_changes_from_before_a_restart_are_still_listed_after_it() {
    let (fixture, host, mut workbench, project) =
        first_open(&[("edited.ts", "before\n"), ("deleted.ts", "deleted\n"), ("same.ts", "same\n")]);
    fixture.write("edited.ts", "after\n");
    fixture.remove("deleted.ts");
    fixture.write("created.ts", "created\n");
    workbench.settle().unwrap();
    let before = changes(&workbench, project);
    assert_eq!(
        before,
        [
            ("created.ts".into(), ChangeKind::Created),
            ("deleted.ts".into(), ChangeKind::Deleted),
            ("edited.ts".into(), ChangeKind::Modified),
        ]
    );

    let (workbench, project) = restart(&host, workbench, &fixture, || {});

    assert_eq!(changes(&workbench, project), before);
    assert_eq!(workbench.project(project).unwrap().review_banner.as_deref(), Some("3 files changed outside Genea"));
}

#[test]
fn keep_revert_and_saves_from_before_a_restart_hold_after_it() {
    let (fixture, host, mut workbench, project) =
        first_open(&[("kept.ts", "kept\n"), ("reverted.ts", "reverted\n"), ("saved.ts", "saved\n")]);
    fixture.write("kept.ts", "kept, changed\n");
    fixture.write("reverted.ts", "reverted, changed\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::KeepChange("kept.ts".into()));
    workbench.dispatch(project, Command::RevertChange("reverted.ts".into()));
    workbench.dispatch(project, Command::OpenFile("saved.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("// ".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), []);

    let (mut workbench, project) = restart(&host, workbench, &fixture, || {});
    assert_eq!(changes(&workbench, project), [], "the kept and saved contents are the baseline");

    // And they stay the baseline for later changes.
    fixture.write("kept.ts", "kept\n");
    fixture.write("saved.ts", "saved\n");
    workbench.settle().unwrap();
    assert_eq!(
        changes(&workbench, project),
        [("kept.ts".into(), ChangeKind::Modified), ("saved.ts".into(), ChangeKind::Modified)]
    );
}

#[test]
fn a_change_undone_while_genea_was_closed_is_no_longer_listed() {
    let (fixture, host, mut workbench, project) = first_open(&[("a.ts", "a\n"), ("b.ts", "b\n")]);
    fixture.write("a.ts", "changed\n");
    fixture.remove("b.ts");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project).len(), 2);

    let (workbench, project) = restart(&host, workbench, &fixture, || {
        fixture.write("a.ts", "a\n");
        fixture.write("b.ts", "b\n");
    });

    assert_eq!(changes(&workbench, project), []);
    assert_eq!(workbench.project(project).unwrap().review_banner, None);
}

#[test]
fn a_saved_file_with_a_pending_change_is_still_listed_after_a_restart() {
    let (fixture, host, mut workbench, project) = first_open(&[("main.ts", "one\n")]);
    fixture.write("main.ts", "two\n");
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::OpenFile("main.ts".into()));
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::InsertText("three ".into()));
    workbench.dispatch(project, Command::Save);
    workbench.settle().unwrap();

    let (mut workbench, project) = restart(&host, workbench, &fixture, || {});
    assert_eq!(changes(&workbench, project), [("main.ts".into(), ChangeKind::Modified)]);

    workbench.dispatch(project, Command::RevertChange("main.ts".into()));
    workbench.settle().unwrap();
    assert_eq!(fixture.read("main.ts"), "one\n", "the baseline is still from before the external change");
}

#[test]
fn files_ignored_while_genea_was_closed_leave_review() {
    let (fixture, host, mut workbench, project) = first_open(&[("a.ts", "a\n"), ("out/b.js", "b\n")]);
    fixture.write("out/b.js", "b, changed\n");
    workbench.settle().unwrap();
    assert_eq!(changes(&workbench, project), [("out/b.js".into(), ChangeKind::Modified)]);

    let (mut workbench, project) = restart(&host, workbench, &fixture, || {
        fixture.write(".gitignore", "out/\n");
        fixture.write("out/c.js", "c\n");
    });
    assert_eq!(changes(&workbench, project), [(".gitignore".into(), ChangeKind::Created)]);

    // Un-ignored again, the file has no baseline: it comes back as created.
    fixture.remove(".gitignore");
    workbench.settle().unwrap();
    assert_eq!(
        changes(&workbench, project),
        [("out/b.js".into(), ChangeKind::Created), ("out/c.js".into(), ChangeKind::Created)]
    );
}
