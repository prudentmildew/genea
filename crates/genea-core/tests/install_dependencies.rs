//! Installing dependencies (ticket #41, ADR 0005): when the project's
//! `node_modules` is missing, a notice offers "Install dependencies".
//! Clicking it runs the pinned package manager's install in a terminal tab.
//! Install scripts run project code, so nothing installs without that click,
//! except through the same command, which the new-project flow dispatches.
//!
//! The test host's fake PTY plays the package manager, as it plays the
//! shell in `terminal.rs`.

use genea_core::{Command, Notice, NoticeAction, ProjectId, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

const PNPM: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\"\n}\n";

fn with_package_json(package_json: &str) -> FixtureBuilder {
    FixtureProject::new().file("package.json", package_json)
}

/// Opens the fixture on a host with the pinned tools published, settled.
fn open(host: &TestHost, fixture: &FixtureProject) -> (Workbench, ProjectId) {
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (workbench, project)
}

/// The notice offering the install, if the window shows one.
fn install_notice(workbench: &Workbench, project: ProjectId) -> Option<Notice> {
    let notices = workbench.project(project).unwrap().notices;
    notices.into_iter().find(|n| n.action.as_ref().is_some_and(|a| a.command == Command::InstallDependencies))
}

#[test]
fn a_project_without_node_modules_offers_to_install_its_dependencies() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();

    let (workbench, project) = open(&host, &fixture);

    let notice = install_notice(&workbench, project).expect("no install notice");
    assert_eq!(
        notice.action,
        Some(NoticeAction { label: "Install dependencies".into(), command: Command::InstallDependencies })
    );
    assert_eq!(notice.message, "This project's dependencies aren't installed.");
}

#[test]
fn a_project_with_node_modules_doesnt() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).dir("node_modules").build();

    let (workbench, project) = open(&host, &fixture);

    assert_eq!(install_notice(&workbench, project), None);
}
