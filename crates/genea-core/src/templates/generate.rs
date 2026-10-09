//! Writing a template's files. Runs on a background thread.

use std::{fs, path::Path};

use serde_json::{Map, Value, json};

use super::{NewProject, PackageManagerPin, RuntimePin, Template};

pub(super) fn generate(request: &NewProject) -> Result<(), String> {
    let folder = &request.folder;
    fs::create_dir_all(folder).map_err(|e| format!("Couldn't create {}: {e}", folder.display()))?;
    match request.template {
        Template::Frontend => {
            let mut package = Map::new();
            package.insert("name".into(), json!(request.name));
            package.insert("private".into(), json!(true));
            package.insert("type".into(), json!("module"));
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
            pins(&mut package, request);
            write(folder, "package.json", &package_json(package))?;
        }
        Template::Backend | Template::FullStack => {}
    }
    Ok(())
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

fn write(folder: &Path, relative: &str, contents: &str) -> Result<(), String> {
    let path = folder.join(relative);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Couldn't create {}: {e}", parent.display()))?;
    }
    fs::write(&path, contents).map_err(|e| format!("Couldn't write {}: {e}", path.display()))
}
