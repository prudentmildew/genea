//! The script runner (ticket #40): a `package.json` script runs as
//! `<pinned package manager> run <script>` in its package's folder, in a
//! terminal tab of its own named `<package>: <script>`, whose status shows
//! whether it runs, finished or failed. Stop interrupts it (and kills it if
//! it doesn't end); re-run replaces its process. The first local URL it
//! prints is its link, in its tab and in the status bar, until it exits.
//!
//! The test host's fake PTY plays the package manager.

use std::{path::Path, time::Duration};

use genea_core::{
    Command, ProjectId, SCRIPT_STOP_TIMEOUT, ScriptLink, TerminalStatus, TerminalTab, TerminalView, Workbench,
};
use genea_testkit::{FakePty, FixtureProject, TestHost};

const ROOT: &str = r#"{
  "name": "shop",
  "packageManager": "pnpm@12.10.1",
  "scripts": { "lint": "oxlint" }
}
"#;

const WEB: &str = r#"{ "name": "@shop/web", "scripts": { "dev": "vite", "build": "vite build" } }"#;

/// A pnpm workspace with a web package, open and settled on a host with
/// the pinned tools published.
fn open(host: &TestHost) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = FixtureProject::new()
        .file("package.json", ROOT)
        .file("pnpm-workspace.yaml", "packages:\n  - apps/*\n")
        .file("apps/web/package.json", WEB)
        .build();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    let mut workbench = Workbench::new(host.shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// Runs a script and returns its fake PTY.
fn run(host: &TestHost, workbench: &mut Workbench, project: ProjectId, package: &str, script: &str) -> FakePty {
    let started = host.ptys().spawned().len();
    workbench.dispatch(project, Command::RunScript { package: package.into(), script: script.into() });
    workbench.settle().unwrap();
    host.ptys().wait_for_spawn(started + 1)
}

fn terminal(workbench: &Workbench, project: ProjectId) -> TerminalView {
    workbench.project(project).unwrap().terminal
}

fn tab(title: &str, status: TerminalStatus) -> TerminalTab {
    TerminalTab { title: title.into(), status, shell: title == "zsh" }
}

/// What a program prints when run with `--version`.
fn version_of(program: &Path) -> String {
    let out = std::process::Command::new(program).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

#[test]
fn a_script_runs_through_the_pinned_package_manager_in_its_packages_folder_in_a_tab_of_its_own() {
    let host = TestHost::new();
    let (fixture, mut workbench, project) = open(&host);

    let dev = run(&host, &mut workbench, project, "apps/web", "dev");

    let spec = dev.spec();
    assert!(spec.program.starts_with(host.support_dir()), "{} isn't from the toolchain store", spec.program.display());
    assert_eq!(version_of(&spec.program), "12.10.1");
    assert_eq!(spec.args, ["run", "dev"]);
    assert_eq!(spec.cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().join("apps/web").as_path()));
    let view = terminal(&workbench, project);
    assert!(view.visible);
    assert_eq!(view.tabs, [tab("zsh", TerminalStatus::Running), tab("@shop/web: dev", TerminalStatus::Running)]);
    assert_eq!(view.active_tab, 1);
}

#[test]
fn a_root_script_runs_in_the_project_root() {
    let host = TestHost::new();
    let (fixture, mut workbench, project) = open(&host);

    let lint = run(&host, &mut workbench, project, "", "lint");

    assert_eq!(lint.spec().args, ["run", "lint"]);
    assert_eq!(lint.spec().cwd.as_deref(), Some(fixture.root().canonicalize().unwrap().as_path()));
    assert_eq!(terminal(&workbench, project).tabs[1], tab("shop: lint", TerminalStatus::Running));
}

#[test]
fn the_tabs_status_shows_whether_the_script_runs_finished_or_failed() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);

    let build = run(&host, &mut workbench, project, "apps/web", "build");
    let lint = run(&host, &mut workbench, project, "", "lint");
    assert_eq!(terminal(&workbench, project).tabs[1].status, TerminalStatus::Running);

    build.exit(0);
    lint.exit(1);
    workbench.settle().unwrap();

    let tabs = terminal(&workbench, project).tabs;
    assert_eq!(tabs[1], tab("@shop/web: build", TerminalStatus::Exited { code: Some(0) }));
    assert_eq!(tabs[2], tab("shop: lint", TerminalStatus::Exited { code: Some(1) }));
}

#[test]
fn running_a_script_again_shows_its_tab_and_runs_it_again_only_once_it_ended() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let first = run(&host, &mut workbench, project, "apps/web", "build");
    workbench.dispatch(project, Command::SelectTerminalTab(0));

    workbench.dispatch(project, Command::RunScript { package: "apps/web".into(), script: "build".into() });
    workbench.settle().unwrap();
    assert_eq!(host.ptys().spawned().len(), 2, "a running script isn't started twice");
    assert_eq!(terminal(&workbench, project).active_tab, 1);

    first.exit(0);
    workbench.settle().unwrap();
    let second = run(&host, &mut workbench, project, "apps/web", "build");
    assert_eq!(second.spec().args, ["run", "build"]);
    assert_eq!(terminal(&workbench, project).tabs.len(), 2);
}

#[test]
fn stop_interrupts_the_script_and_the_tab_shows_it_stopped() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let dev = run(&host, &mut workbench, project, "apps/web", "dev");

    workbench.dispatch(project, Command::StopTerminalTab(1));
    dev.wait_for_interrupt();
    assert!(!dev.killed());
    // Vite ends on SIGINT.
    dev.exit(130);
    workbench.settle().unwrap();

    assert_eq!(terminal(&workbench, project).tabs[1], tab("@shop/web: dev", TerminalStatus::Stopped));
    host.clock().advance(SCRIPT_STOP_TIMEOUT);
    workbench.settle().unwrap();
    assert!(!dev.killed(), "a script that ended isn't killed");
}

#[test]
fn a_script_that_ignores_the_interrupt_is_killed_after_the_timeout() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let dev = run(&host, &mut workbench, project, "apps/web", "dev");

    workbench.dispatch(project, Command::StopTerminalTab(1));
    dev.wait_for_interrupt();
    host.clock().advance(SCRIPT_STOP_TIMEOUT - Duration::from_millis(1));
    workbench.settle().unwrap();
    assert!(!dev.killed());
    assert_eq!(terminal(&workbench, project).tabs[1].status, TerminalStatus::Running);

    host.clock().advance(Duration::from_millis(1));
    dev.wait_for_kill();
    workbench.settle().unwrap();

    assert_eq!(terminal(&workbench, project).tabs[1].status, TerminalStatus::Stopped);
}

#[test]
fn rerun_replaces_the_tabs_process() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let first = run(&host, &mut workbench, project, "apps/web", "dev");
    first.output("VITE ready\r\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RerunTerminalTab(1));
    workbench.settle().unwrap();
    let second = host.ptys().wait_for_spawn(3);

    first.wait_for_hang_up();
    assert_eq!(second.spec().args, ["run", "dev"]);
    assert_eq!(second.spec().cwd, first.spec().cwd);
    second.output("VITE ready again\r\n");
    workbench.settle().unwrap();
    let view = terminal(&workbench, project);
    assert_eq!(view.tabs, [tab("zsh", TerminalStatus::Running), tab("@shop/web: dev", TerminalStatus::Running)]);
    assert_eq!(view.lines[0].text, "VITE ready again");
}

#[test]
fn rerun_runs_an_ended_script_again() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let first = run(&host, &mut workbench, project, "apps/web", "build");
    first.exit(1);
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RerunTerminalTab(1));
    workbench.settle().unwrap();

    let second = host.ptys().wait_for_spawn(3);
    assert_eq!(second.spec().args, ["run", "build"]);
    assert_eq!(terminal(&workbench, project).tabs[1].status, TerminalStatus::Running);
}

fn status_links(workbench: &Workbench, project: ProjectId) -> Vec<ScriptLink> {
    workbench.project(project).unwrap().status.script_links
}

fn link(tab: usize, title: &str, url: &str) -> ScriptLink {
    ScriptLink { tab, title: title.into(), url: url.into() }
}

#[test]
fn the_first_local_url_a_script_prints_is_its_link_in_its_tab_and_the_status_bar_until_it_exits() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let dev = run(&host, &mut workbench, project, "apps/web", "dev");
    assert_eq!(terminal(&workbench, project).url, None);

    // As Vite prints it: colours, and the port in bold.
    dev.output("\r\n  VITE v8.0.1  ready in 312 ms\r\n\r\n");
    dev.output("  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://localhost:\x1b[1m5173\x1b[22m/\x1b[39m\r\n");
    dev.output("  \x1b[32m➜\x1b[39m  \x1b[1mNetwork\x1b[22m: \x1b[36mhttp://192.168.1.20:5173/\x1b[39m\r\n");
    dev.output("  http://127.0.0.1:4000/ is another one\r\n");
    workbench.settle().unwrap();

    assert_eq!(terminal(&workbench, project).url.as_deref(), Some("http://localhost:5173/"));
    assert_eq!(status_links(&workbench, project), [link(1, "@shop/web: dev", "http://localhost:5173/")]);

    // Another tab showing: the status bar still has it.
    workbench.dispatch(project, Command::SelectTerminalTab(0));
    assert_eq!(terminal(&workbench, project).url, None);
    assert_eq!(status_links(&workbench, project).len(), 1);

    dev.exit(130);
    workbench.settle().unwrap();
    workbench.dispatch(project, Command::SelectTerminalTab(1));
    assert_eq!(terminal(&workbench, project).url, None);
    assert_eq!(status_links(&workbench, project), []);
}

#[test]
fn a_url_on_127_0_0_1_or_ipv6_loopback_counts_and_one_split_across_reads_too() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let build = run(&host, &mut workbench, project, "apps/web", "build");
    let lint = run(&host, &mut workbench, project, "", "lint");
    let dev = run(&host, &mut workbench, project, "apps/web", "dev");

    build.output("Listening on https://127.0.0.1:8443/app?x=1.\r\n");
    lint.output("server at http://[::1]:3000, press h for help\r\n");
    dev.output("Started server: http://local");
    dev.output("host:3000\r\n");
    workbench.settle().unwrap();

    assert_eq!(
        status_links(&workbench, project),
        [
            link(1, "@shop/web: build", "https://127.0.0.1:8443/app?x=1"),
            link(2, "shop: lint", "http://[::1]:3000"),
            link(3, "@shop/web: dev", "http://localhost:3000"),
        ]
    );
}

#[test]
fn other_hosts_and_shell_tabs_have_no_link() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let shell = host.ptys().wait_for_spawn(1);
    let dev = run(&host, &mut workbench, project, "apps/web", "dev");

    shell.output("http://localhost:8080/\r\n");
    dev.output("http://0.0.0.0:5173/ http://example.com/ http://localhostess:1/ ftp://localhost/\r\n");
    workbench.settle().unwrap();

    assert_eq!(terminal(&workbench, project).url, None);
    assert_eq!(status_links(&workbench, project), []);
}

#[test]
fn rerun_forgets_the_link_until_the_new_process_prints_one() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let first = run(&host, &mut workbench, project, "apps/web", "dev");
    first.output("http://localhost:5173/\r\n");
    workbench.settle().unwrap();

    workbench.dispatch(project, Command::RerunTerminalTab(1));
    workbench.settle().unwrap();
    let second = host.ptys().wait_for_spawn(3);
    assert_eq!(status_links(&workbench, project), []);

    second.output("http://localhost:5174/\r\n");
    workbench.settle().unwrap();
    assert_eq!(terminal(&workbench, project).url.as_deref(), Some("http://localhost:5174/"));
}

#[test]
fn stop_and_rerun_leave_shell_tabs_alone() {
    let host = TestHost::new();
    let (_fixture, mut workbench, project) = open(&host);
    let shell = host.ptys().wait_for_spawn(1);

    workbench.dispatch(project, Command::StopTerminalTab(0));
    workbench.dispatch(project, Command::RerunTerminalTab(0));
    workbench.settle().unwrap();

    assert!(!shell.interrupted());
    assert!(!shell.hung_up());
    assert_eq!(host.ptys().spawned().len(), 1);
}
