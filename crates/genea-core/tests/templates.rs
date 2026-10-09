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

/// Every file under `folder`, relative and sorted, leaving out `.git`.
fn files(parent: &FixtureProject, folder: &str) -> Vec<String> {
    fn walk(dir: &std::path::Path, base: &std::path::Path, out: &mut Vec<String>) {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(base).unwrap().to_str().unwrap().to_owned();
            if relative == ".git" {
                continue;
            }
            if path.is_dir() {
                walk(&path, base, out);
            } else {
                out.push(relative);
            }
        }
    }
    let base = parent.path(folder);
    let mut out = Vec::new();
    walk(&base, &base, &mut out);
    out.sort();
    out
}

#[test]
fn the_frontend_template_is_a_vite_react_app() {
    let parent = create(Template::Frontend, "my-app", node(), pnpm());

    assert_eq!(
        files(&parent, "my-app"),
        [
            ".gitignore",
            ".oxlintrc.json",
            "README.md",
            "index.html",
            "package.json",
            "public/favicon.svg",
            "src/App.css",
            "src/App.tsx",
            "src/clicks.test.ts",
            "src/clicks.ts",
            "src/main.tsx",
            "tsconfig.json",
            "vite.config.ts",
        ]
    );
    assert!(parent.read("my-app/index.html").contains("<title>my-app</title>"));
    assert!(parent.read("my-app/src/App.tsx").contains("<h1>my-app</h1>"));
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

#[test]
fn the_backend_template_is_a_hono_server_with_no_build_step() {
    let parent = create(Template::Backend, "my-api", node(), pnpm());

    assert_eq!(
        files(&parent, "my-api"),
        [".gitignore", ".oxlintrc.json", "README.md", "package.json", "src/app.test.ts", "src/app.ts", "src/index.ts", "tsconfig.json"]
    );
    let package = json_file(&parent, "my-api/package.json");
    assert_eq!(package["name"], "my-api");
    assert_eq!(package["type"], "module");
    assert_eq!(
        package["scripts"],
        json!({
            "dev": "node --watch src/index.ts",
            "start": "node src/index.ts",
            "test": "vitest run",
            "lint": "oxlint",
            "format": "oxfmt",
            "typecheck": "tsc --noEmit",
        })
    );
    assert!(package["dependencies"]["hono"].as_str().unwrap().starts_with("^4."));
    assert!(package["dependencies"]["@hono/node-server"].is_string());
    // @types/node follows the pinned Node major.
    assert_eq!(package["devDependencies"]["@types/node"], "^24.0.0");
    let tsconfig = json_file(&parent, "my-api/tsconfig.json");
    assert_eq!(tsconfig["compilerOptions"]["module"], "nodenext");
    assert_eq!(tsconfig["compilerOptions"]["allowImportingTsExtensions"], true);
}
