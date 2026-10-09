//! Writing a template's files. Runs on a background thread.
//!
//! Static files live in `crates/genea-core/templates/` and are embedded with
//! `include_str!`; `{{name}}` in them becomes the project name. Files that
//! depend on the package manager or the pins (every `package.json`,
//! `pnpm-workspace.yaml`, the README) are built here. Nothing else differs
//! between the pnpm and Bun variants.

use std::{fs, path::Path};

use serde_json::{Map, Value, json};

use super::{NewProject, PackageManagerPin, RuntimePin, Template};

/// Dependency versions, frozen per Genea release as caret ranges (#12).
/// Refresh them with each release and run the slow lane
/// (`tests/templates_slow.rs`).
pub(crate) mod versions {
    pub const HONO: &str = "^4.13.13";
    pub const HONO_NODE_SERVER: &str = "^2.1.4";
    pub const OXFMT: &str = "^0.72.0";
    pub const OXLINT: &str = "^1.87.0";
    pub const REACT: &str = "^19.3.0";
    pub const REACT_DOM: &str = "^19.3.0";
    pub const TYPES_REACT: &str = "^19.3.0";
    pub const TYPES_REACT_DOM: &str = "^19.3.0";
    pub const TYPESCRIPT: &str = "^7.0.2";
    pub const VITE: &str = "^8.3.4";
    pub const VITEJS_PLUGIN_REACT: &str = "^6.1.2";
    pub const VITEST: &str = "^5.0.3";
    /// `@types/node` follows the pinned Node major. A Bun runtime gets the
    /// types of the Node major Genea defaults to.
    pub const TYPES_NODE_WITH_BUN: &str = "^24.0.0";
}

/// Embeds a file from `templates/` as `(path in the project, contents)`.
macro_rules! file {
    ($path:literal from $source:literal) => {
        ($path, include_str!(concat!("../../templates/", $source)))
    };
}

type Files = &'static [(&'static str, &'static str)];

const GITIGNORE: &str = include_str!("../../templates/common/gitignore");

const FRONTEND: Files = &[
    file!(".oxlintrc.json" from "frontend/.oxlintrc.json"),
    file!("index.html" from "frontend/index.html"),
    file!("public/favicon.svg" from "frontend/public/favicon.svg"),
    file!("src/App.css" from "frontend/src/App.css"),
    file!("src/App.tsx" from "frontend/src/App.tsx"),
    file!("src/clicks.test.ts" from "frontend/src/clicks.test.ts"),
    file!("src/clicks.ts" from "frontend/src/clicks.ts"),
    file!("src/main.tsx" from "frontend/src/main.tsx"),
    file!("tsconfig.json" from "frontend/tsconfig.json"),
    file!("vite.config.ts" from "frontend/vite.config.ts"),
];

const BACKEND: Files = &[
    file!(".oxlintrc.json" from "backend/.oxlintrc.json"),
    file!("src/app.test.ts" from "backend/src/app.test.ts"),
    file!("src/app.ts" from "backend/src/app.ts"),
    file!("src/index.ts" from "backend/src/index.ts"),
    file!("tsconfig.json" from "backend/tsconfig.json"),
];

/// The full-stack workspace. Files that are the same as in the frontend or
/// backend template come from there.
const FULL_STACK: Files = &[
    file!(".oxlintrc.json" from "full-stack/.oxlintrc.json"),
    file!("tsconfig.base.json" from "full-stack/tsconfig.base.json"),
    file!("apps/api/src/app.test.ts" from "full-stack/apps/api/src/app.test.ts"),
    file!("apps/api/src/app.ts" from "full-stack/apps/api/src/app.ts"),
    file!("apps/api/src/index.ts" from "backend/src/index.ts"),
    file!("apps/api/tsconfig.json" from "full-stack/apps/api/tsconfig.json"),
    file!("apps/web/index.html" from "frontend/index.html"),
    file!("apps/web/public/favicon.svg" from "frontend/public/favicon.svg"),
    file!("apps/web/src/App.css" from "frontend/src/App.css"),
    file!("apps/web/src/App.tsx" from "full-stack/apps/web/src/App.tsx"),
    file!("apps/web/src/api.ts" from "full-stack/apps/web/src/api.ts"),
    file!("apps/web/src/main.tsx" from "frontend/src/main.tsx"),
    file!("apps/web/src/name.test.ts" from "full-stack/apps/web/src/name.test.ts"),
    file!("apps/web/src/name.ts" from "full-stack/apps/web/src/name.ts"),
    file!("apps/web/tsconfig.json" from "full-stack/apps/web/tsconfig.json"),
    file!("apps/web/vite.config.ts" from "full-stack/apps/web/vite.config.ts"),
    file!("packages/shared/src/index.test.ts" from "full-stack/packages/shared/src/index.test.ts"),
    file!("packages/shared/src/index.ts" from "full-stack/packages/shared/src/index.ts"),
    file!("packages/shared/tsconfig.json" from "full-stack/packages/shared/tsconfig.json"),
];

/// The workspace's package folders, as globs.
const WORKSPACE_PACKAGES: [&str; 2] = ["apps/*", "packages/*"];

pub(super) fn generate(request: &NewProject) -> Result<(), String> {
    let folder = &request.folder;
    fs::create_dir_all(folder).map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    let mut entries = fs::read_dir(folder).map_err(|e| format!("Couldn't read {}: {e}", folder.display()))?;
    if entries.next().is_some() {
        return Err(format!("{} isn't empty. Choose a new or empty folder.", folder.display()));
    }
    let out = Writer { folder, name: &request.name };
    let root = match request.template {
        Template::Frontend => frontend(request, &out)?,
        Template::Backend => backend(request, &out)?,
        Template::FullStack => full_stack(request, &out)?,
    };
    let readme = readme(request, &root);
    out.write("package.json", &package_json(with_pins(root, request)))?;
    out.write(".gitignore", GITIGNORE)?;
    out.write("README.md", &readme)?;
    init_repository(folder)
}

/// `git init`, in-process: no `git` binary is run (ADR 0005). HEAD points at
/// `main`, and there is no initial commit (#12).
fn init_repository(folder: &Path) -> Result<(), String> {
    gix::create::into(folder, gix::create::Kind::WithWorktree, gix::create::Options::default())
        .map(|_| ())
        .map_err(|e| format!("Couldn't create a git repository in {}: {e}", folder.display()))
}

/// Writes the frontend template and returns its `package.json`.
fn frontend(request: &NewProject, out: &Writer) -> Result<Map<String, Value>, String> {
    out.files(FRONTEND)?;
    Ok(package(
        &request.name,
        json!({
            "scripts": {
                "dev": "vite",
                "build": "vite build",
                "preview": "vite preview",
                "test": "vitest run",
                "lint": "oxlint",
                "format": "oxfmt",
                "typecheck": "tsc --noEmit",
            },
            "dependencies": {
                "react": versions::REACT,
                "react-dom": versions::REACT_DOM,
            },
            "devDependencies": {
                "@types/react": versions::TYPES_REACT,
                "@types/react-dom": versions::TYPES_REACT_DOM,
                "@vitejs/plugin-react": versions::VITEJS_PLUGIN_REACT,
                "oxfmt": versions::OXFMT,
                "oxlint": versions::OXLINT,
                "typescript": versions::TYPESCRIPT,
                "vite": versions::VITE,
                "vitest": versions::VITEST,
            },
        }),
    ))
}

/// Writes the backend template and returns its `package.json`.
fn backend(request: &NewProject, out: &Writer) -> Result<Map<String, Value>, String> {
    out.files(BACKEND)?;
    Ok(package(
        &request.name,
        json!({
            "scripts": {
                "dev": "node --watch src/index.ts",
                "start": "node src/index.ts",
                "test": "vitest run",
                "lint": "oxlint",
                "format": "oxfmt",
                "typecheck": "tsc --noEmit",
            },
            "dependencies": {
                "@hono/node-server": versions::HONO_NODE_SERVER,
                "hono": versions::HONO,
            },
            "devDependencies": {
                "@types/node": types_node(&request.runtime),
                "oxfmt": versions::OXFMT,
                "oxlint": versions::OXLINT,
                "typescript": versions::TYPESCRIPT,
                "vitest": versions::VITEST,
            },
        }),
    ))
}

/// Writes the full-stack workspace and returns its root `package.json`.
///
/// Packages are `@<name>/web`, `@<name>/api` and `@<name>/shared`. Oxlint
/// and Oxfmt are root tools; TypeScript, Vitest, `@types/node` and Hono come
/// from the catalog, which keeps Hono identical on both sides of the RPC.
fn full_stack(request: &NewProject, out: &Writer) -> Result<Map<String, Value>, String> {
    out.files(FULL_STACK)?;
    let scope = |package: &str| format!("@{}/{package}", request.name);
    let catalog = json!({
        "@types/node": types_node(&request.runtime),
        "hono": versions::HONO,
        "typescript": versions::TYPESCRIPT,
        "vitest": versions::VITEST,
    });

    let web = package(
        &scope("web"),
        json!({
            "scripts": {
                "dev": "vite",
                "build": "vite build",
                "preview": "vite preview",
                "test": "vitest run",
                "typecheck": "tsc --noEmit",
            },
            "dependencies": {
                scope("shared"): "workspace:*",
                "hono": "catalog:",
                "react": versions::REACT,
                "react-dom": versions::REACT_DOM,
            },
            "devDependencies": {
                scope("api"): "workspace:*",
                "@types/react": versions::TYPES_REACT,
                "@types/react-dom": versions::TYPES_REACT_DOM,
                "@vitejs/plugin-react": versions::VITEJS_PLUGIN_REACT,
                "typescript": "catalog:",
                "vite": versions::VITE,
                "vitest": "catalog:",
            },
        }),
    );
    let api = package(
        &scope("api"),
        json!({
            "exports": { ".": "./src/app.ts" },
            "scripts": {
                "dev": "node --watch src/index.ts",
                "start": "node src/index.ts",
                "test": "vitest run",
                "typecheck": "tsc --noEmit",
            },
            "dependencies": {
                "@hono/node-server": versions::HONO_NODE_SERVER,
                scope("shared"): "workspace:*",
                "hono": "catalog:",
            },
            "devDependencies": {
                "@types/node": "catalog:",
                "typescript": "catalog:",
                "vitest": "catalog:",
            },
        }),
    );
    let shared = package(
        &scope("shared"),
        json!({
            "exports": { ".": "./src/index.ts" },
            "scripts": {
                "test": "vitest run",
                "typecheck": "tsc --noEmit",
            },
            "devDependencies": {
                "@types/node": "catalog:",
                "typescript": "catalog:",
                "vitest": "catalog:",
            },
        }),
    );
    out.write("apps/web/package.json", &package_json(web))?;
    out.write("apps/api/package.json", &package_json(api))?;
    out.write("packages/shared/package.json", &package_json(shared))?;

    let (workspaces, scripts) = match request.package_manager {
        PackageManagerPin::Pnpm(_) => {
            out.write("pnpm-workspace.yaml", &pnpm_workspace(&catalog))?;
            let scripts = json!({
                "dev": "pnpm -r --parallel dev",
                "build": "pnpm -r build",
                "test": "pnpm -r test",
                "typecheck": "pnpm -r typecheck",
                "lint": "oxlint",
                "format": "oxfmt",
            });
            (Value::Null, scripts)
        }
        PackageManagerPin::Bun(_) => {
            let workspaces = json!({ "packages": WORKSPACE_PACKAGES, "catalog": catalog });
            // Filtering on the scope leaves out the root, whose scripts
            // these are.
            let each = |script: &str| format!("bun run --filter '@{}/*' {script}", request.name);
            let scripts = json!({
                // Bun runs filtered scripts in dependency order, so without
                // --parallel the web app would wait for the API to exit.
                "dev": format!("bun run --parallel --filter '@{}/*' dev", request.name),
                "build": each("build"),
                "test": each("test"),
                "typecheck": each("typecheck"),
                "lint": "oxlint",
                "format": "oxfmt",
            });
            (workspaces, scripts)
        }
    };
    Ok(package(
        &request.name,
        json!({
            "workspaces": workspaces,
            "scripts": scripts,
            "devDependencies": {
                "oxfmt": versions::OXFMT,
                "oxlint": versions::OXLINT,
            },
        }),
    ))
}

/// `pnpm-workspace.yaml`: the package globs and the catalog.
fn pnpm_workspace(catalog: &Value) -> String {
    let mut text = String::from("packages:\n");
    for glob in WORKSPACE_PACKAGES {
        text.push_str(&format!("  - {glob}\n"));
    }
    text.push_str("\ncatalog:\n");
    for (name, version) in catalog.as_object().into_iter().flatten() {
        // A plain YAML key can't start with `@`.
        let key = if name.starts_with('@') { format!("\"{name}\"") } else { name.clone() };
        text.push_str(&format!("  {key}: {}\n", version.as_str().unwrap_or_default()));
    }
    text
}

/// A `package.json`: name, private and ESM, then `fields` in order, with
/// keys in the order Oxfmt sorts them. Null fields are left out, and
/// dependencies are sorted as package managers sort them.
fn package(name: &str, fields: Value) -> Map<String, Value> {
    let Value::Object(mut fields) = fields else { unreachable!("package fields are an object") };
    let mut package = Map::new();
    package.insert("name".into(), json!(name));
    package.insert("private".into(), json!(true));
    if let Some(workspaces) = fields.shift_remove("workspaces").filter(|w| !w.is_null()) {
        package.insert("workspaces".into(), workspaces);
    }
    package.insert("type".into(), json!("module"));
    for (key, value) in fields {
        let value = match (key.as_str(), value) {
            (_, Value::Null) => continue,
            ("dependencies" | "devDependencies", Value::Object(dependencies)) => {
                let mut dependencies: Vec<_> = dependencies.into_iter().collect();
                dependencies.sort_by(|a, b| a.0.cmp(&b.0));
                Value::Object(dependencies.into_iter().collect())
            }
            (_, value) => value,
        };
        package.insert(key, value);
    }
    package
}

/// Adds the exact toolchain pins (ADR 0005).
fn with_pins(mut package: Map<String, Value>, request: &NewProject) -> Map<String, Value> {
    let (runtime, version) = match &request.runtime {
        RuntimePin::Node(version) => ("node", version),
        RuntimePin::Bun(version) => ("bun", version),
    };
    package.insert("devEngines".into(), json!({ "runtime": { "name": runtime, "version": version } }));
    let package_manager = match &request.package_manager {
        PackageManagerPin::Pnpm(version) => format!("pnpm@{version}"),
        PackageManagerPin::Bun(version) => format!("bun@{version}"),
    };
    package.insert("packageManager".into(), json!(package_manager));
    package
}

/// The `@types/node` range for the pinned runtime: the Node major's types.
fn types_node(runtime: &RuntimePin) -> String {
    match runtime {
        RuntimePin::Node(version) => {
            let major = version.split('.').next().unwrap_or(version);
            format!("^{major}.0.0")
        }
        RuntimePin::Bun(_) => versions::TYPES_NODE_WITH_BUN.into(),
    }
}

/// `package.json` as package managers and Oxfmt write it: two-space indent,
/// every object and array expanded, a final newline.
fn package_json(package: Map<String, Value>) -> String {
    let mut text = serde_json::to_string_pretty(&Value::Object(package)).expect("JSON values always serialise");
    text.push('\n');
    text
}

/// A short README: what the project is and the root package's scripts.
fn readme(request: &NewProject, root: &Map<String, Value>) -> String {
    let about = match request.template {
        Template::Frontend => "A React single-page app built with Vite.",
        Template::Backend => "A Hono server. Node runs the TypeScript source directly, with no build step.",
        Template::FullStack => {
            "A workspace with a React app in `apps/web`, a Hono API in `apps/api` and the code they share in \
             `packages/shared`. The web app calls the API with types from Hono RPC, and in development Vite \
             proxies `/api` to the API, so both run on one URL."
        }
    };
    let run = match request.package_manager {
        PackageManagerPin::Pnpm(_) => "pnpm run",
        PackageManagerPin::Bun(_) => "bun run",
    };
    let mut text = format!(
        "# {}\n\n{about}\n\n## Scripts\n\nRun them from Genea's script runner, or with `{run} <script>`.\n\n",
        request.name
    );
    for script in root["scripts"].as_object().into_iter().flat_map(|scripts| scripts.keys()) {
        text.push_str(&format!("- `{script}` {}.\n", describe(request.template, script)));
    }
    text
}

/// What a root script does, for the README.
fn describe(template: Template, script: &str) -> &'static str {
    match (template, script) {
        (Template::Frontend, "dev") => "starts the dev server",
        (Template::Backend, "dev") => "starts the server and restarts it when a file changes",
        (Template::FullStack, "dev") => "starts the web app and the API together",
        (Template::FullStack, "build") => "builds the web app into `apps/web/dist/`",
        (Template::FullStack, "test") => "runs every package's tests with Vitest",
        (Template::FullStack, "typecheck") => "type-checks every package with TypeScript",
        (_, "start") => "starts the server",
        (_, "build") => "builds the app into `dist/`",
        (_, "preview") => "serves the built app",
        (_, "test") => "runs the tests with Vitest",
        (_, "lint") => "lints with Oxlint",
        (_, "format") => "formats with Oxfmt",
        (_, "typecheck") => "type-checks with TypeScript",
        _ => "",
    }
}

/// Writes files into the new project.
struct Writer<'a> {
    folder: &'a Path,
    name: &'a str,
}

impl Writer<'_> {
    /// Writes embedded files, filling in the project name.
    fn files(&self, files: Files) -> Result<(), String> {
        for (path, contents) in files {
            self.write(path, &contents.replace("{{name}}", self.name))?;
        }
        Ok(())
    }

    fn write(&self, relative: &str, contents: &str) -> Result<(), String> {
        let path = self.folder.join(relative);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
        }
        fs::write(&path, contents).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
    }
}
