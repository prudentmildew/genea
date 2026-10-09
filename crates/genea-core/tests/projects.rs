//! Opening and closing projects, the change notification and settle
//! (ticket #20).

use std::sync::mpsc;
use std::time::Duration;

use genea_core::{Command, Workbench};
use genea_testkit::{FixtureProject, TestHost};

#[test]
fn a_project_is_named_after_its_folder() {
    let fixture = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());

    let project = workbench.open_project(fixture.root()).unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.root, fixture.root());
    assert_eq!(view.name, fixture.root().file_name().unwrap().to_str().unwrap());
    assert_eq!(view.editor, None);
}

#[test]
fn opening_the_same_folder_twice_gives_the_same_project() {
    let fixture = FixtureProject::new().dir("src").build();
    let mut workbench = Workbench::new(TestHost::new().shared());

    let first = workbench.open_project(fixture.root()).unwrap();
    let again = workbench.open_project(fixture.path("src/..")).unwrap();

    assert_eq!(first, again);
    assert_eq!(workbench.projects(), [first]);
}

#[test]
fn a_missing_folder_or_a_file_cant_be_opened_as_a_project() {
    let fixture = FixtureProject::new().file("package.json", "{}").build();
    let mut workbench = Workbench::new(TestHost::new().shared());

    assert!(workbench.open_project(fixture.path("missing")).is_err());
    assert!(workbench.open_project(fixture.path("package.json")).is_err());
    assert!(workbench.projects().is_empty());
}

#[test]
fn a_closed_project_has_no_view_and_ignores_late_results() {
    let fixture = FixtureProject::new().file("a.ts", "a").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    workbench.close_project(project);
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project), None);
    assert!(workbench.projects().is_empty());
}

#[test]
fn a_file_that_cant_be_read_leaves_the_open_file_and_shows_a_notice() {
    let fixture = FixtureProject::new().file("a.ts", "const a = 1;").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::OpenFile("missing.ts".into()));
    workbench.settle().unwrap();

    let view = workbench.project(project).unwrap();
    assert_eq!(view.editor.unwrap().title, "a.ts");
    assert_eq!(view.notices.len(), 1);
    assert!(view.notices[0].message.starts_with("Couldn't open missing.ts"), "{:?}", view.notices);
}

#[test]
fn the_last_file_opened_wins() {
    let fixture = FixtureProject::new().file("a.ts", "a").file("b.ts", "b").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    workbench.dispatch(project, Command::OpenFile("b.ts".into()));
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().editor.unwrap().title, "b.ts");
}

#[test]
fn a_file_can_be_opened_by_its_absolute_path() {
    let fixture = FixtureProject::new().file("src/a.ts", "a").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();

    workbench.dispatch(project, Command::OpenFile(fixture.path("src/a.ts")));
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().editor.unwrap().path, std::path::Path::new("src/a.ts"));
}

#[test]
fn background_work_notifies_and_shows_up_after_pump() {
    let fixture = FixtureProject::new().file("a.ts", "a").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let (notified_tx, notified) = mpsc::channel();
    workbench.set_notifier(move || {
        let _ = notified_tx.send(());
    });
    let project = workbench.open_project(fixture.root()).unwrap();
    // Opening starts background work of its own (reading the config).
    workbench.settle().unwrap();
    while notified.try_recv().is_ok() {}

    workbench.dispatch(project, Command::OpenFile("a.ts".into()));
    notified.recv_timeout(Duration::from_secs(5)).expect("a change notification");

    // Nothing changes until the main thread pumps.
    assert_eq!(workbench.project(project).unwrap().editor, None);
    assert!(workbench.pump());
    assert_eq!(workbench.project(project).unwrap().editor.unwrap().title, "a.ts");
    assert!(!workbench.pump(), "nothing left to apply");
}

#[test]
fn settle_returns_at_once_when_nothing_is_running() {
    let mut workbench = Workbench::new(TestHost::new().shared());

    workbench.settle().unwrap();
}
