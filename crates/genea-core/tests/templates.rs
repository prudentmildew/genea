//! Creating a project from a template (ticket #60, decided in #12).
//!
//! Every template × package-manager combination is generated into a temp
//! folder and checked for structure: files, scripts, pins, and package names
//! derived from the project name. `templates_slow.rs` installs them and runs
//! their scripts.

use genea_core::{NewProject, PackageManagerPin, ProjectCreation, RuntimePin, Template, Workbench};
use genea_testkit::{FixtureProject, TestHost};
use serde_json::{Value, json};

/// Creates `name` from `template` inside a fresh temp folder and waits for it.
fn create(template: Template, name: &str, runtime: RuntimePin, package_manager: PackageManagerPin) -> FixtureProject {
    let parent = FixtureProject::new().build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let folder = parent.path(name);

    workbench.create_project(NewProject { template, name: name.into(), folder: folder.clone(), runtime, package_manager });
    workbench.settle().unwrap();

    assert_eq!(workbench.project_creation(), Some(ProjectCreation::Created { folder }));
    parent
}

fn node() -> RuntimePin {
    RuntimePin::Node("24.21.0".into())
}

fn pnpm() -> PackageManagerPin {
    PackageManagerPin::Pnpm("12.10.1".into())
}

fn json_file(parent: &FixtureProject, path: &str) -> Value {
    serde_json::from_str(&parent.read(path)).unwrap_or_else(|e| panic!("{path} is not JSON: {e}"))
}

#[test]
fn the_frontend_template_writes_a_pinned_package_named_after_the_project() {
    let parent = create(Template::Frontend, "my-app", node(), pnpm());

    let package = json_file(&parent, "my-app/package.json");
    assert_eq!(package["name"], "my-app");
    assert_eq!(package["private"], true);
    assert_eq!(package["type"], "module");
    assert_eq!(package["packageManager"], "pnpm@12.10.1");
    assert_eq!(package["devEngines"], json!({ "runtime": { "name": "node", "version": "24.21.0" } }));
    assert_eq!(
        package["scripts"],
        json!({
            "dev": "vite",
            "build": "vite build",
            "preview": "vite preview",
            "test": "vitest run",
            "lint": "oxlint",
            "format": "oxfmt",
            "typecheck": "tsc --noEmit",
        })
    );
}
