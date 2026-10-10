//! The workspace model (ticket #40): a project's packages are its root
//! package plus the folders `pnpm-workspace.yaml` (pnpm) or the root
//! `package.json`'s `workspaces` (Bun) match, never inside `node_modules`.
//! The script runner lists each package's scripts, root first, then the
//! other packages in path order, and follows `package.json` changes live.

use std::path::PathBuf;

use genea_core::{Command, FinderMode, LeftColumnView, PackageScripts, ProjectId, Script, Workbench};
use genea_testkit::{FixtureBuilder, FixtureProject, TestHost};

fn open(fixture: FixtureBuilder) -> (FixtureProject, Workbench, ProjectId) {
    let fixture = fixture.build();
    let mut workbench = Workbench::new(TestHost::new().shared());
    let project = workbench.open_project(fixture.root()).unwrap();
    workbench.settle().unwrap();
    (fixture, workbench, project)
}

/// The script runner as the user reads it: each package's name and folder,
/// and its scripts' names.
fn listed(workbench: &Workbench, project: ProjectId) -> Vec<(String, PathBuf, Vec<String>)> {
    let scripts: Vec<PackageScripts> = workbench.project(project).unwrap().scripts;
    scripts
        .into_iter()
        .map(|package| (package.name, package.path, package.scripts.into_iter().map(|s| s.name).collect()))
        .collect()
}

fn entry(name: &str, path: &str, scripts: &[&str]) -> (String, PathBuf, Vec<String>) {
    (name.into(), path.into(), scripts.iter().map(|&s| s.to_owned()).collect())
}

const PNPM_ROOT: &str = r#"{
  "name": "shop",
  "packageManager": "pnpm@12.10.1",
  "scripts": { "dev": "pnpm -r dev", "lint": "oxlint" }
}
"#;

#[test]
fn a_pnpm_workspace_lists_the_root_then_each_package_in_path_order() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file("package.json", PNPM_ROOT)
            .file("pnpm-workspace.yaml", "packages:\n  - \"packages/*\"\n  - 'apps/**'\n  - '!**/fixtures/**'\n")
            .file("packages/shared/package.json", r#"{ "name": "@shop/shared", "scripts": { "build": "tsgo", "test": "vitest" } }"#)
            .file("apps/web/package.json", r#"{ "name": "@shop/web", "scripts": { "dev": "vite", "build": "vite build" } }"#)
            .file("apps/api/package.json", r#"{ "name": "@shop/api", "scripts": { "start": "node src/main.ts" } }"#)
            // Excluded by the negated pattern, outside the patterns, and in node_modules.
            .file("apps/web/fixtures/demo/package.json", r#"{ "name": "demo", "scripts": { "x": "y" } }"#)
            .file("tools/gen/package.json", r#"{ "name": "gen", "scripts": { "gen": "node gen.ts" } }"#)
            .file("packages/shared/node_modules/dep/package.json", r#"{ "name": "dep", "scripts": { "x": "y" } }"#)
            .file("node_modules/other/package.json", r#"{ "name": "other", "scripts": { "x": "y" } }"#),
    );

    assert_eq!(
        listed(&workbench, project),
        [
            entry("shop", "", &["dev", "lint"]),
            entry("@shop/api", "apps/api", &["start"]),
            entry("@shop/web", "apps/web", &["dev", "build"]),
            entry("@shop/shared", "packages/shared", &["build", "test"]),
        ]
    );
}

#[test]
fn the_scripts_action_shows_the_script_runner_in_the_left_column() {
    let (_fixture, mut workbench, project) = open(FixtureProject::new().file("package.json", PNPM_ROOT));

    workbench.dispatch(project, Command::OpenFinder(FinderMode::Actions));
    workbench.dispatch(project, Command::SetFinderQuery("Scripts".into()));
    workbench.settle().unwrap();
    let items = workbench.project(project).unwrap().finder.unwrap().items;
    let index = items.iter().position(|item| item.label == "Scripts").expect("Scripts is an action");
    workbench.dispatch(project, Command::SelectFinderItem(index));
    workbench.dispatch(project, Command::AcceptFinder);
    workbench.settle().unwrap();

    assert_eq!(workbench.project(project).unwrap().left_column, Some(LeftColumnView::Scripts));
}

#[test]
fn a_script_shows_its_command() {
    let (_fixture, workbench, project) = open(FixtureProject::new().file("package.json", PNPM_ROOT));

    let scripts = workbench.project(project).unwrap().scripts;
    assert_eq!(
        scripts[0].scripts,
        [Script { name: "dev".into(), command: "pnpm -r dev".into() }, Script { name: "lint".into(), command: "oxlint".into() }]
    );
}

#[test]
fn a_bun_workspace_takes_its_packages_from_the_root_workspaces_field() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file(
                "package.json",
                r#"{ "name": "stack", "packageManager": "bun@1.4.2", "workspaces": ["packages/*", "apps/web"], "scripts": { "dev": "bun --filter '*' dev" } }"#,
            )
            .file("apps/web/package.json", r#"{ "name": "web", "scripts": { "dev": "vite" } }"#)
            .file("apps/admin/package.json", r#"{ "name": "admin", "scripts": { "dev": "vite" } }"#)
            .file("packages/server/package.json", r#"{ "name": "server", "scripts": { "dev": "bun --watch src/index.ts" } }"#)
            .file("packages/types/package.json", r#"{ "name": "types" }"#)
            // A Bun project ignores pnpm's file.
            .file("pnpm-workspace.yaml", "packages:\n  - apps/*\n"),
    );

    assert_eq!(
        listed(&workbench, project),
        [
            entry("stack", "", &["dev"]),
            entry("web", "apps/web", &["dev"]),
            entry("server", "packages/server", &["dev"]),
            entry("types", "packages/types", &[]),
        ]
    );
}

#[test]
fn bun_workspaces_may_be_an_object_with_a_packages_list() {
    let (_fixture, workbench, project) = open(
        FixtureProject::new()
            .file("package.json", r#"{ "name": "stack", "packageManager": "bun@1.4.2", "workspaces": { "packages": ["packages/*"] } }"#)
            .file("packages/server/package.json", r#"{ "name": "server", "scripts": { "dev": "bun run src/index.ts" } }"#),
    );

    assert_eq!(listed(&workbench, project), [entry("stack", "", &[]), entry("server", "packages/server", &["dev"])]);
}

#[test]
fn a_package_without_a_name_goes_by_its_folders_name() {
    let (fixture, workbench, project) = open(
        FixtureProject::new()
            .file("package.json", r#"{ "scripts": { "build": "tsgo" } }"#)
            .file("pnpm-workspace.yaml", "packages: [libs/*]\n")
            .file("libs/util/package.json", r#"{ "scripts": { "test": "vitest" } }"#),
    );

    let root = fixture.root().file_name().unwrap().to_string_lossy().into_owned();
    assert_eq!(listed(&workbench, project), [entry(&root, "", &["build"]), entry("util", "libs/util", &["test"])]);
}

#[test]
fn a_folder_without_a_root_package_json_has_no_scripts() {
    let (_fixture, workbench, project) =
        open(FixtureProject::new().file("web/package.json", r#"{ "name": "web", "scripts": { "dev": "vite" } }"#));

    assert_eq!(listed(&workbench, project), []);
}

#[test]
fn adding_a_package_or_a_script_updates_the_list_live() {
    let (fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("package.json", PNPM_ROOT)
            .file("pnpm-workspace.yaml", "packages:\n  - packages/*\n")
            .file("packages/shared/package.json", r#"{ "name": "@shop/shared", "scripts": { "build": "tsgo" } }"#),
    );

    fixture.write("packages/api/package.json", r#"{ "name": "@shop/api", "scripts": { "start": "node main.ts" } }"#);
    workbench.settle().unwrap();
    assert_eq!(
        listed(&workbench, project),
        [
            entry("shop", "", &["dev", "lint"]),
            entry("@shop/api", "packages/api", &["start"]),
            entry("@shop/shared", "packages/shared", &["build"]),
        ]
    );

    fixture.write("packages/shared/package.json", r#"{ "name": "@shop/shared", "scripts": { "build": "tsgo", "test": "vitest" } }"#);
    fixture.write("pnpm-workspace.yaml", "packages:\n  - packages/*\n  - apps/*\n");
    fixture.write("apps/web/package.json", r#"{ "name": "@shop/web", "scripts": { "dev": "vite" } }"#);
    workbench.settle().unwrap();
    assert_eq!(
        listed(&workbench, project),
        [
            entry("shop", "", &["dev", "lint"]),
            entry("@shop/web", "apps/web", &["dev"]),
            entry("@shop/api", "packages/api", &["start"]),
            entry("@shop/shared", "packages/shared", &["build", "test"]),
        ]
    );
}

#[test]
fn removing_a_packages_folder_drops_it() {
    let (fixture, mut workbench, project) = open(
        FixtureProject::new()
            .file("package.json", PNPM_ROOT)
            .file("pnpm-workspace.yaml", "packages:\n  - packages/*\n")
            .file("packages/shared/package.json", r#"{ "name": "@shop/shared", "scripts": { "build": "tsgo" } }"#),
    );

    std::fs::remove_dir_all(fixture.path("packages/shared")).unwrap();
    workbench.settle().unwrap();

    assert_eq!(listed(&workbench, project), [entry("shop", "", &["dev", "lint"])]);
}
