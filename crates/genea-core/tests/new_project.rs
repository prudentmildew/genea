//! The new-project flow (ticket #61): New Project… opens a dialog that asks
//! for the template, a name, a parent folder and the toolchain pins. Create
//! generates the project into `<parent>/<name>`, opens it (the app gives it a
//! new window) and installs its dependencies in a terminal tab once the
//! toolchain is ready: creating the project was the click (ADR 0005).
//!
//! Version lists and downloads come from the test host's local download
//! fixture server; the fake PTY plays the install.

use std::path::Path;

use genea_core::{
    NewProjectCommand, NewProjectDialog, PackageManagerPin, ProjectId, RuntimePin, Template, Workbench,
};
use genea_testkit::{FixtureProject, TestHost};
use serde_json::{Value, json};

/// A host with Genea's default tool versions published.
fn host() -> TestHost {
    let host = TestHost::new();
    host.tools().node("24.21.0");
    host.tools().pnpm("12.10.1");
    host.tools().bun("1.4.2");
    host
}

fn open_dialog(workbench: &mut Workbench) -> NewProjectDialog {
    workbench.dispatch_new_project(NewProjectCommand::Open);
    workbench.settle().unwrap();
    workbench.new_project_dialog().expect("the dialog is open")
}

/// Fills in the name and parent folder and clicks Create.
fn create(workbench: &mut Workbench, name: &str, parent: &Path) {
    workbench.dispatch_new_project(NewProjectCommand::SetName(name.into()));
    workbench.dispatch_new_project(NewProjectCommand::SetParent(parent.to_owned()));
    workbench.dispatch_new_project(NewProjectCommand::Create);
    workbench.settle().unwrap();
}

fn package_json(folder: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(folder.join("package.json")).unwrap()).unwrap()
}

fn only_project(workbench: &Workbench) -> ProjectId {
    let projects = workbench.projects();
    assert_eq!(projects.len(), 1, "one project is open");
    projects[0]
}

#[test]
fn create_opens_the_new_project_with_exact_pins() {
    let host = host();
    let parent = FixtureProject::new().build();
    let mut workbench = Workbench::new(host.shared());
    open_dialog(&mut workbench);

    create(&mut workbench, "my-app", parent.root());

    assert_eq!(workbench.new_project_dialog(), None, "the dialog closes");
    let project = workbench.project(only_project(&workbench)).unwrap();
    let folder = parent.root().canonicalize().unwrap().join("my-app");
    assert_eq!(project.root, folder);
    let package = package_json(&folder);
    assert_eq!(package["name"], "my-app");
    assert_eq!(package["packageManager"], "pnpm@12.10.1");
    assert_eq!(package["devEngines"]["runtime"], json!({ "name": "node", "version": "24.21.0" }));
    assert_eq!(workbench.welcome(), None);
}

/// What a program prints when run with `--version`.
fn version_of(program: &Path) -> String {
    let out = std::process::Command::new(program).arg("--version").output().unwrap();
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// The install's PTY, once both it and the terminal's shell have started
/// (in either order: they wait for the same toolchain).
fn install_spec(host: &TestHost) -> genea_core::ProcessSpec {
    host.ptys().wait_for_spawn(2);
    let specs: Vec<_> = host.ptys().spawned().iter().map(|pty| pty.spec()).collect();
    specs.into_iter().find(|spec| spec.args == ["install"]).expect("no install started")
}

#[test]
fn the_install_starts_in_the_terminal_once_the_toolchain_is_ready() {
    let host = host();
    let parent = FixtureProject::new().build();
    let mut workbench = Workbench::new(host.shared());
    open_dialog(&mut workbench);

    create(&mut workbench, "my-app", parent.root());

    let spec = install_spec(&host);
    assert!(spec.program.starts_with(host.support_dir()), "{} isn't from the toolchain store", spec.program.display());
    assert_eq!(version_of(&spec.program), "12.10.1");
    assert_eq!(spec.args, ["install"]);
    assert_eq!(spec.cwd.as_deref(), Some(parent.root().canonicalize().unwrap().join("my-app").as_path()));
    let terminal = workbench.project(only_project(&workbench)).unwrap().terminal;
    assert_eq!(terminal.tabs[terminal.active_tab].title, "pnpm install");
}
