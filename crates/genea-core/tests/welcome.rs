//! Project windows, the welcome and recent projects (ticket #58).
//!
//! Each open project is one window. With no project open, the app shows the
//! welcome, which lists recent projects, most recent first. The list is
//! global and kept in Genea's application-support folder, so it survives a
//! restart (a second workbench on the same test host).

use std::{path::Path, time::Duration};

use genea_core::{Command, Workbench, WelcomeView};
use genea_testkit::{FixtureProject, TestHost};

/// The recent-projects list is written in the background after a short
/// pause. Advancing past it and settling waits for the write.
const SAVED: Duration = Duration::from_secs(5);

fn recent_roots(welcome: &WelcomeView) -> Vec<&Path> {
    welcome.recent_projects.iter().map(|recent| recent.root.as_path()).collect()
}

#[test]
fn the_welcome_lists_projects_that_were_open_most_recent_first() {
    let first = FixtureProject::new().build();
    let second = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let a = workbench.open_project(first.root()).unwrap();
    let b = workbench.open_project(second.root()).unwrap();

    workbench.close_project(a);
    workbench.close_project(b);

    let welcome = workbench.welcome().expect("the welcome shows once the last project closes");
    assert_eq!(recent_roots(&welcome), [second.root(), first.root()]);
    assert_eq!(welcome.recent_projects[0].name, second.root().file_name().unwrap().to_str().unwrap());
}

#[test]
fn recent_projects_survive_a_restart() {
    let first = FixtureProject::new().build();
    let second = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());
    workbench.open_project(first.root()).unwrap();
    workbench.open_project(second.root()).unwrap();
    host.clock().advance(SAVED);
    workbench.settle().unwrap();
    drop(workbench);

    let restarted = Workbench::new(host.shared());

    let welcome = restarted.welcome().unwrap();
    assert_eq!(recent_roots(&welcome), [second.root(), first.root()]);
}

#[test]
fn the_welcome_keeps_the_ten_most_recent_projects() {
    let fixtures: Vec<FixtureProject> = (0..12).map(|_| FixtureProject::new().build()).collect();
    let mut workbench = Workbench::new(TestHost::new().shared());

    for fixture in &fixtures {
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.close_project(project);
    }

    let welcome = workbench.welcome().unwrap();
    let newest_first: Vec<&Path> = fixtures.iter().rev().take(10).map(FixtureProject::root).collect();
    assert_eq!(recent_roots(&welcome), newest_first);
}

#[test]
fn a_recent_project_whose_folder_is_gone_leaves_the_list_when_opened() {
    let kept = FixtureProject::new().build();
    let gone = FixtureProject::new().build();
    let gone_root = gone.root().to_owned();
    let mut workbench = Workbench::new(TestHost::new().shared());
    for fixture in [&kept, &gone] {
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.close_project(project);
    }
    drop(gone);

    assert!(workbench.open_project(&gone_root).is_err());

    assert_eq!(recent_roots(&workbench.welcome().unwrap()), [kept.root()]);
}

#[test]
fn two_projects_are_open_at_once_each_with_its_own_view() {
    let first = FixtureProject::new().file("a.ts", "first").build();
    let second = FixtureProject::new().file("a.ts", "second").build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let a = workbench.open_project(first.root()).unwrap();
    let b = workbench.open_project(second.root()).unwrap();

    workbench.dispatch(a, Command::OpenFile("a.ts".into()));
    workbench.dispatch(b, Command::OpenFile("a.ts".into()));
    workbench.settle().unwrap();

    assert_ne!(a, b);
    assert_eq!(workbench.projects(), [a, b]);
    assert_eq!(workbench.project(a).unwrap().editor.unwrap().lines[0].text, "first");
    assert_eq!(workbench.project(b).unwrap().editor.unwrap().lines[0].text, "second");
    assert_eq!(workbench.welcome(), None, "no welcome while a project is open");

    workbench.close_project(a);
    assert_eq!(workbench.welcome(), None, "no welcome while the other project is open");
}

#[test]
fn opening_an_open_project_again_gives_its_window_and_makes_it_most_recent() {
    let first = FixtureProject::new().build();
    let second = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let a = workbench.open_project(first.root()).unwrap();
    let b = workbench.open_project(second.root()).unwrap();

    assert_eq!(workbench.open_project(first.root()).unwrap(), a);

    workbench.close_project(a);
    workbench.close_project(b);
    assert_eq!(recent_roots(&workbench.welcome().unwrap()), [first.root(), second.root()]);
}

#[test]
fn opening_a_recent_project_from_the_welcome_moves_it_first() {
    let first = FixtureProject::new().build();
    let second = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    for fixture in [&first, &second] {
        let project = workbench.open_project(fixture.root()).unwrap();
        workbench.close_project(project);
    }

    let recent = workbench.welcome().unwrap().recent_projects[1].root.clone();
    let project = workbench.open_project(&recent).unwrap();
    workbench.close_project(project);

    assert_eq!(recent_roots(&workbench.welcome().unwrap()), [first.root(), second.root()]);
}
