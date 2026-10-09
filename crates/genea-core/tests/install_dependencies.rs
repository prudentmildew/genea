//! Installing dependencies (ticket #41, ADR 0005): when the project's
//! `node_modules` is missing, a notice offers "Install dependencies".
//! Clicking it runs the pinned package manager's install in a terminal tab.
//! Install scripts run project code, so nothing installs without that click,
//! except through the same command, which the new-project flow dispatches.
//!
//! The test host's fake PTY plays the package manager, as it plays the
//! shell in `terminal.rs`.

use std::path::Path;

use genea_core::{Command, Notice, NoticeAction, ProjectId, TerminalStatus, TerminalTab, TerminalView, Workbench};
use genea_testkit::{FakePty, FixtureBuilder, FixtureProject, TestHost};

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

fn terminal(workbench: &Workbench, project: ProjectId) -> TerminalView {
    workbench.project(project).unwrap().terminal
}

/// The terminal's rows as text, without the empty rows at the bottom.
fn screen(workbench: &Workbench, project: ProjectId) -> Vec<String> {
    let mut lines: Vec<String> = terminal(workbench, project).lines.into_iter().map(|line| line.text).collect();
    while lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Clicks the notice's button, and returns the install's fake PTY (the
/// terminal's shell is the first).
fn click_install(host: &TestHost, workbench: &mut Workbench, project: ProjectId) -> FakePty {
    let notice = install_notice(workbench, project).expect("no install notice");
    let started = host.ptys().spawned().len();
    workbench.dispatch(project, notice.action.unwrap().command);
    workbench.settle().unwrap();
    host.ptys().wait_for_spawn(started + 1)
}

/// What a program prints when run with `--version`.
fn version_of(program: &Path) -> String {
    let out = std::process::Command::new(program).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

fn tab(title: &str, status: TerminalStatus) -> TerminalTab {
    TerminalTab { title: title.into(), status }
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

#[test]
fn clicking_it_runs_the_pinned_package_managers_install_in_a_terminal_tab() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();
    let (mut workbench, project) = open(&host, &fixture);

    let install = click_install(&host, &mut workbench, project);

    let spec = install.spec();
    assert!(spec.program.starts_with(host.support_dir()), "{} isn't from the toolchain store", spec.program.display());
    assert_eq!(version_of(&spec.program), "12.10.1");
    assert_eq!(spec.args, ["install"]);
    assert_eq!(spec.cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));

    install.output("Packages: +3\r\nDone in 1.2s\r\n");
    workbench.settle().unwrap();
    let view = terminal(&workbench, project);
    assert!(view.visible);
    assert_eq!(view.tabs, [tab("zsh", TerminalStatus::Running), tab("pnpm install", TerminalStatus::Running)]);
    assert_eq!(view.active_tab, 1);
    assert_eq!(screen(&workbench, project), ["Packages: +3", "Done in 1.2s"]);
}
