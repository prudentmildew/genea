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
