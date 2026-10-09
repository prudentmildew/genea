//! Writing a template's files. Runs on a background thread.
//!
//! Static files live in `crates/genea-core/templates/` and are embedded with
//! `include_str!`; `{{name}}` in them becomes the project name. Files that
//! depend on the package manager or the pins (`package.json`, the README)
//! are built here.

use std::{fs, path::Path};

use serde_json::{Map, Value, json};

use super::{NewProject, PackageManagerPin, RuntimePin, Template};

/// Dependency versions, frozen per Genea release as caret ranges (#12).
/// Refresh them with each release and run the slow lane
/// (`tests/templates_slow.rs`).
mod versions {
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
}

/// Embeds files from `templates/<dir>/` as `(path in the project, contents)`.
macro_rules! embed {
    ($dir:literal: $($path:literal),* $(,)?) => {
        &[$(($path, include_str!(concat!("../../templates/", $dir, "/", $path)))),*]
    };
}

type Files = &'static [(&'static str, &'static str)];

const GITIGNORE: &str = include_str!("../../templates/common/gitignore");

const FRONTEND: Files = embed!("frontend":
    ".oxlintrc.json",
    "index.html",
    "public/favicon.svg",
    "src/App.css",
    "src/App.tsx",
    "src/clicks.test.ts",
    "src/clicks.ts",
    "src/main.tsx",
    "tsconfig.json",
    "vite.config.ts",
);

pub(super) fn generate(request: &NewProject) -> Result<(), String> {
    let folder = &request.folder;
    fs::create_dir_all(folder).map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    let out = Writer { folder, name: &request.name };
    match request.template {
        Template::Frontend => {
            out.files(FRONTEND)?;
            let mut package = package(&request.name);
            package.insert(
                "scripts".into(),
                json!({
                    "dev": "vite",
                    "build": "vite build",
                    "preview": "vite preview",
                    "test": "vitest run",
                    "lint": "oxlint",
                    "format": "oxfmt",
                    "typecheck": "tsc --noEmit",
                }),
            );
            package.insert(
                "dependencies".into(),
                json!({
                    "react": versions::REACT,
                    "react-dom": versions::REACT_DOM,
                }),
            );
            package.insert(
                "devDependencies".into(),
                json!({
                    "@types/react": versions::TYPES_REACT,
                    "@types/react-dom": versions::TYPES_REACT_DOM,
                    "@vitejs/plugin-react": versions::VITEJS_PLUGIN_REACT,
                    "oxfmt": versions::OXFMT,
                    "oxlint": versions::OXLINT,
                    "typescript": versions::TYPESCRIPT,
                    "vite": versions::VITE,
                    "vitest": versions::VITEST,
                }),
            );
            pins(&mut package, request);
            out.write("package.json", &package_json(package))?;
        }
        Template::Backend | Template::FullStack => {}
    }
    out.write(".gitignore", GITIGNORE)?;
    out.write("README.md", &readme(request))?;
    Ok(())
}

/// The start of every `package.json`: name, private, ESM.
fn package(name: &str) -> Map<String, Value> {
    let mut package = Map::new();
    package.insert("name".into(), json!(name));
    package.insert("private".into(), json!(true));
    package.insert("type".into(), json!("module"));
    package
}

/// Adds the exact toolchain pins (ADR 0005).
fn pins(package: &mut Map<String, Value>, request: &NewProject) {
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
}

/// `package.json` as package managers and Oxfmt write it: two-space indent,
/// every object and array expanded, a final newline.
fn package_json(package: Map<String, Value>) -> String {
    let mut text = serde_json::to_string_pretty(&Value::Object(package)).expect("JSON values always serialise");
    text.push('\n');
    text
}

/// A short README: what the project is and its scripts.
fn readme(request: &NewProject) -> String {
    let (about, scripts): (&str, &[(&str, &str)]) = match request.template {
        Template::Frontend => (
            "A React single-page app built with Vite.",
            &[
                ("dev", "starts the dev server"),
                ("build", "builds the app into `dist/`"),
                ("preview", "serves the built app"),
                ("test", "runs the tests with Vitest"),
                ("lint", "lints with Oxlint"),
                ("format", "formats with Oxfmt"),
                ("typecheck", "type-checks with TypeScript"),
            ],
        ),
        Template::Backend | Template::FullStack => ("", &[]),
    };
    let run = match request.package_manager {
        PackageManagerPin::Pnpm(_) => "pnpm run",
        PackageManagerPin::Bun(_) => "bun run",
    };
    let mut text = format!("# {}\n\n{about}\n\n## Scripts\n\nRun them from Genea's script runner, or with `{run} <script>`.\n\n", request.name);
    for (script, what) in scripts {
        text.push_str(&format!("- `{script}` {what}.\n"));
    }
    text
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
