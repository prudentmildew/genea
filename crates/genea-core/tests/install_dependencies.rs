//! Installing dependencies (ticket #41, ADR 0005): when the project's
//! `node_modules` is missing, a notice offers "Install dependencies".
//! Clicking it runs the pinned package manager's install in a terminal tab.
//! Install scripts run project code, so nothing installs without that click,
//! except through the same command, which the new-project flow dispatches.
//!
//! The test host's fake PTY plays the package manager, as it plays the
//! shell in `terminal.rs`.

use std::{path::Path, sync::mpsc, time::Duration};

use genea_core::{
    Command, Notice, NoticeAction, ProjectId, TerminalStatus, TerminalTab, TerminalView, ToolState, ToolView, Workbench,
};
use genea_testkit::{FakePty, FixtureBuilder, FixtureProject, TestHost};

const PNPM: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"pnpm@12.10.1\"\n}\n";
const BUN: &str = "{\n  \"name\": \"app\",\n  \"packageManager\": \"bun@1.4.2\"\n}\n";

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
    TerminalTab { title: title.into(), status, shell: title == "zsh" }
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

#[test]
fn nothing_installs_without_the_click() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();

    let (workbench, project) = open(&host, &fixture);

    // Only the terminal's shell runs.
    let started: Vec<_> = host.ptys().spawned().iter().map(|pty| pty.spec().args).collect();
    assert_eq!(started, [["-l"]]);
    assert_eq!(host.processes().spawned(), []);
    assert_eq!(terminal(&workbench, project).tabs, [tab("zsh", TerminalStatus::Running)]);
}

#[test]
fn the_notice_goes_while_the_install_runs_and_once_node_modules_is_there() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();
    let (mut workbench, project) = open(&host, &fixture);

    let install = click_install(&host, &mut workbench, project);
    assert_eq!(install_notice(&workbench, project), None);

    fixture.write("node_modules/.modules.yaml", "layoutVersion: 5\n");
    install.exit(0);
    workbench.settle().unwrap();
    assert_eq!(install_notice(&workbench, project), None);
    assert_eq!(terminal(&workbench, project).tabs[1], tab("pnpm install", TerminalStatus::Exited { code: Some(0) }));

    // Deleting node_modules offers the install again.
    std::fs::remove_dir_all(fixture.path("node_modules")).unwrap();
    workbench.settle().unwrap();
    assert!(install_notice(&workbench, project).is_some());
}

#[test]
fn a_failed_install_offers_it_again() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();
    let (mut workbench, project) = open(&host, &fixture);

    let install = click_install(&host, &mut workbench, project);
    install.output("ERR_PNPM_FETCH_404\r\n");
    install.exit(1);
    workbench.settle().unwrap();

    assert_eq!(terminal(&workbench, project).tabs[1], tab("pnpm install", TerminalStatus::Exited { code: Some(1) }));
    assert!(install_notice(&workbench, project).is_some());
}

#[test]
fn a_bun_project_installs_with_the_pinned_bun() {
    let host = TestHost::new();
    let fixture = with_package_json(BUN).build();
    let (mut workbench, project) = open(&host, &fixture);

    let install = click_install(&host, &mut workbench, project);

    let spec = install.spec();
    assert!(spec.program.starts_with(host.support_dir()), "{} isn't from the toolchain store", spec.program.display());
    assert_eq!(version_of(&spec.program), "1.4.2");
    assert_eq!(spec.args, ["install"]);
    assert_eq!(terminal(&workbench, project).tabs[1].title, "bun install");
}

#[test]
fn clicking_again_shows_the_running_install_and_reruns_one_that_ended() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();
    let (mut workbench, project) = open(&host, &fixture);
    let first = click_install(&host, &mut workbench, project);
    workbench.dispatch(project, Command::SelectTerminalTab(0));
    assert_eq!(terminal(&workbench, project).active_tab, 0);

    workbench.dispatch(project, Command::InstallDependencies);
    workbench.settle().unwrap();
    assert_eq!(host.ptys().spawned().len(), 2, "a second install started");
    assert_eq!(terminal(&workbench, project).active_tab, 1);

    first.output("failed\r\n");
    first.exit(1);
    workbench.settle().unwrap();
    let again = click_install(&host, &mut workbench, project);

    assert_eq!(again.spec().args, ["install"]);
    let view = terminal(&workbench, project);
    assert_eq!(view.tabs, [tab("zsh", TerminalStatus::Running), tab("pnpm install", TerminalStatus::Running)]);
    assert_eq!(view.active_tab, 1);
    assert_eq!(screen(&workbench, project), Vec::<String>::new());
}

#[test]
fn the_shell_tab_shows_again_when_selected() {
    let host = TestHost::new();
    let fixture = with_package_json(PNPM).build();
    let (mut workbench, project) = open(&host, &fixture);
    let shell = host.ptys().last();
    shell.output("$ ");
    let install = click_install(&host, &mut workbench, project);
    install.output("Progress: resolved 1\r\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::SelectTerminalTab(0));

    assert_eq!(screen(&workbench, project), ["$"]);
    workbench.dispatch(project, Command::TerminalText("ls".into()));
    assert_eq!(shell.wait_for_input("ls"), "ls");
    assert_eq!(install.input(), "");
}

#[test]
fn an_install_clicked_during_the_toolchain_download_waits_for_it() {
    let host = TestHost::new();
    let node = host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.download_server().hold(&node);
    let fixture = with_package_json(PNPM).build();
    let mut workbench = Workbench::new(host.shared());
    let (notify, notified) = mpsc::channel();
    workbench.set_notifier(move || {
        let _ = notify.send(());
    });
    let project = workbench.open_project(fixture.root()).unwrap();
    // Applies start the downloads: pump until Node's is under way.
    loop {
        workbench.pump();
        let runtime = workbench.project(project).unwrap().toolchain.runtime;
        if matches!(runtime, Some(ToolView { state: ToolState::Downloading { .. }, .. })) {
            break;
        }
        notified.recv_timeout(Duration::from_secs(10)).expect("Node never started downloading");
    }
    host.download_server().wait_held(&node);

    workbench.dispatch(project, Command::InstallDependencies);
    workbench.pump();
    assert_eq!(host.ptys().spawned().len(), 0);
    assert_eq!(terminal(&workbench, project).tabs[1], tab("pnpm install", TerminalStatus::Starting));

    host.download_server().release(&node);
    workbench.settle().unwrap();
    let install = host.ptys().wait_for_spawn(2);
    assert_eq!(version_of(&install.spec().program), "12.10.1");
    assert_eq!(install.spec().args, ["install"]);
}

#[test]
fn a_folder_without_package_json_has_nothing_to_install() {
    let host = TestHost::new();
    let fixture = FixtureProject::new().file("main.ts", "").build();
    let (mut workbench, project) = open(&host, &fixture);
    assert_eq!(install_notice(&workbench, project), None);

    workbench.dispatch(project, Command::InstallDependencies);
    workbench.settle().unwrap();

    assert_eq!(host.ptys().spawned().len(), 1);
    let notices = workbench.project(project).unwrap().notices;
    assert!(notices.iter().any(|n| n.message.contains("package.json")), "{notices:?}");
}
