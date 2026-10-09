//! Project windows, the welcome and recent projects (ticket #58).
//!
//! Each open project is one window. With no project open, the app shows the
//! welcome, which lists recent projects, most recent first. The list is
//! global and kept in Genea's application-support folder, so it survives a
//! restart (a second workbench on the same test host).

use std::{path::Path, time::Duration};

use genea_core::{Workbench, WelcomeView};
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
