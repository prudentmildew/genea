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

#[test]
fn the_full_stack_template_is_a_workspace_of_web_api_and_shared() {
    let parent = create(Template::FullStack, "shop", node(), pnpm());

    assert_eq!(
        files(&parent, "shop"),
        [
            ".gitignore",
            ".oxlintrc.json",
            "README.md",
            "apps/api/package.json",
            "apps/api/src/app.test.ts",
            "apps/api/src/app.ts",
            "apps/api/src/index.ts",
            "apps/api/tsconfig.json",
            "apps/web/index.html",
            "apps/web/package.json",
            "apps/web/public/favicon.svg",
            "apps/web/src/App.css",
            "apps/web/src/App.tsx",
            "apps/web/src/api.ts",
            "apps/web/src/main.tsx",
            "apps/web/src/name.test.ts",
            "apps/web/src/name.ts",
            "apps/web/tsconfig.json",
            "apps/web/vite.config.ts",
            "package.json",
            "packages/shared/package.json",
            "packages/shared/src/index.test.ts",
            "packages/shared/src/index.ts",
            "packages/shared/tsconfig.json",
            "pnpm-workspace.yaml",
            "tsconfig.base.json",
        ]
    );

    let root = json_file(&parent, "shop/package.json");
    assert_eq!(root["name"], "shop");
    assert_eq!(root["packageManager"], "pnpm@12.10.1");
    assert_eq!(root["devEngines"]["runtime"], json!({ "name": "node", "version": "24.21.0" }));
    assert_eq!(
        root["scripts"],
        json!({
            "dev": "pnpm -r --parallel dev",
            "build": "pnpm -r build",
            "test": "pnpm -r test",
            "typecheck": "pnpm -r typecheck",
            "lint": "oxlint",
            "format": "oxfmt",
        })
    );
    // Oxlint and Oxfmt run once, at the root.
    let root_tools: Vec<&String> = root["devDependencies"].as_object().unwrap().keys().collect();
    assert_eq!(root_tools, ["oxfmt", "oxlint"]);

    let workspace = parent.read("shop/pnpm-workspace.yaml");
    assert!(workspace.starts_with("packages:\n  - apps/*\n  - packages/*\n"), "{workspace}");
    for entry in ["\n  hono: ^4.", "\n  typescript: ^7.", "\n  vitest: ^5.", "\n  \"@types/node\": ^24.0.0\n"] {
        assert!(workspace.contains(entry), "{entry} in {workspace}");
    }

    let web = json_file(&parent, "shop/apps/web/package.json");
    assert_eq!(web["name"], "@shop/web");
    assert_eq!(web["dependencies"]["@shop/shared"], "workspace:*");
    assert_eq!(web["dependencies"]["hono"], "catalog:");
    assert_eq!(web["devDependencies"]["@shop/api"], "workspace:*");
    assert_eq!(web["devDependencies"]["typescript"], "catalog:");
    assert_eq!(web["devDependencies"]["vitest"], "catalog:");
    assert!(parent.read("shop/apps/web/src/api.ts").contains("import type { AppType } from \"@shop/api\";"));
    assert!(parent.read("shop/apps/web/vite.config.ts").contains("\"/api\": \"http://localhost:3000\""));

    let api = json_file(&parent, "shop/apps/api/package.json");
    assert_eq!(api["name"], "@shop/api");
    assert_eq!(api["exports"], json!({ ".": "./src/app.ts" }));
    assert_eq!(api["dependencies"]["@shop/shared"], "workspace:*");
    assert_eq!(api["dependencies"]["hono"], "catalog:");
    assert_eq!(api["devDependencies"]["@types/node"], "catalog:");
    assert_eq!(api["scripts"]["dev"], "node --watch src/index.ts");

    let shared = json_file(&parent, "shop/packages/shared/package.json");
    assert_eq!(shared["name"], "@shop/shared");
    assert_eq!(shared["exports"], json!({ ".": "./src/index.ts" }));

    for package in ["apps/web", "apps/api", "packages/shared"] {
        let manifest = json_file(&parent, &format!("shop/{package}/package.json"));
        assert_eq!(manifest["packageManager"], Value::Null, "pins live at the root only");
        assert_eq!(manifest["scripts"]["typecheck"], "tsc --noEmit", "{package}");
        assert_eq!(manifest["scripts"]["test"], "vitest run", "{package}");
        let tsconfig = json_file(&parent, &format!("shop/{package}/tsconfig.json"));
        assert_eq!(tsconfig["extends"], "../../tsconfig.base.json", "{package}");
    }
}

#[test]
fn the_full_stack_template_with_bun_keeps_the_workspace_and_catalog_in_package_json() {
    let parent = create(Template::FullStack, "shop", RuntimePin::Bun("1.4.2".into()), PackageManagerPin::Bun("1.4.2".into()));

    assert!(!parent.path("shop/pnpm-workspace.yaml").exists());
    let root = json_file(&parent, "shop/package.json");
    assert_eq!(root["packageManager"], "bun@1.4.2");
    assert_eq!(root["devEngines"]["runtime"], json!({ "name": "bun", "version": "1.4.2" }));
    assert_eq!(root["workspaces"]["packages"], json!(["apps/*", "packages/*"]));
    let catalog = root["workspaces"]["catalog"].as_object().unwrap();
    let mut names: Vec<&String> = catalog.keys().collect();
    names.sort();
    assert_eq!(names, ["@types/node", "hono", "typescript", "vitest"]);
    // In parallel: by default Bun waits for the API (a dependency of the
    // web app) to exit before it starts the web app.
    assert_eq!(root["scripts"]["dev"], "bun run --parallel --filter '@shop/*' dev");
    assert_eq!(root["scripts"]["typecheck"], "bun run --filter '@shop/*' typecheck");
    assert_eq!(root["scripts"]["lint"], "oxlint");
}

#[test]
fn a_new_project_is_an_empty_git_repository_on_main() {
    for template in [Template::Frontend, Template::Backend, Template::FullStack] {
        let parent = create(template, "my-app", node(), pnpm());

        assert_eq!(parent.read("my-app/.git/HEAD"), "ref: refs/heads/main\n", "{template:?}");
        // No initial commit.
        assert!(!parent.path("my-app/.git/refs/heads/main").exists(), "{template:?}");
    }
}

#[test]
fn generation_starts_no_process_and_downloads_nothing() {
    let parent = FixtureProject::new().build();
    let host = TestHost::new();
    let mut workbench = Workbench::new(host.shared());

    for (i, template) in [Template::Frontend, Template::Backend, Template::FullStack].into_iter().enumerate() {
        for (j, package_manager) in [pnpm(), PackageManagerPin::Bun("1.4.2".into())].into_iter().enumerate() {
            let folder = parent.path(format!("app-{i}-{j}"));
            let request = NewProject { template, name: "app".into(), folder: folder.clone(), runtime: node(), package_manager };
            workbench.create_project(request);
            workbench.settle().unwrap();
            assert_eq!(workbench.project_creation(), Some(ProjectCreation::Created { folder }));
        }
    }

    assert_eq!(host.processes().spawned(), []);
    assert_eq!(host.downloads().requests(), Vec::<String>::new());
}

/// Generates every template with whatever PATH the process has. Run by
/// `generation_needs_no_git_binary` with an empty PATH.
#[test]
#[ignore = "run by generation_needs_no_git_binary"]
fn generate_every_template() {
    for template in [Template::Frontend, Template::Backend, Template::FullStack] {
        create(template, "my-app", node(), pnpm());
    }
}

#[test]
fn generation_needs_no_git_binary() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["generate_every_template", "--exact", "--ignored"])
        .env("PATH", "")
        .output()
        .unwrap();

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success() && stdout.contains("1 passed"), "{stdout}\n{}", String::from_utf8_lossy(&output.stderr));
}
