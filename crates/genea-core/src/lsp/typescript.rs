//! TypeScript 7's language server, tsgo (ADR 0002): finding it in the
//! project's `node_modules`, how it is started, and "Add TypeScript 7".
//!
//! The binary is the platform package's `lib/tsc`
//! (`@typescript/typescript-darwin-arm64`), run directly: `.bin/tsc` is
//! only a Node launcher for it. It sits beside the `typescript` package,
//! which with pnpm is in the virtual store, so the search starts from that
//! package's real folder. A project may also have TS 7 under an alias, with
//! `typescript` itself an older version (vscode: `"@typescript/native":
//! "npm:typescript@^7.0.2"`), so every root dependency that aliases
//! `typescript` is a candidate too. Only the root `node_modules` counts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Map, Value};

use crate::templates::TYPESCRIPT_VERSION;

/// The npm package with the macOS arm64 tsgo binary.
pub(crate) const PLATFORM_PACKAGE: &str = "@typescript/typescript-darwin-arm64";

/// The dependency sections of a `package.json` that can name TypeScript.
const SECTIONS: [&str; 4] = ["dependencies", "devDependencies", "optionalDependencies", "peerDependencies"];

/// What the project has of TypeScript 7.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Detection {
    /// No root `package.json`, or one that isn't a JSON object: Genea
    /// doesn't manage TypeScript here, and says nothing.
    NoPackageJson,
    /// tsgo is installed.
    Found {
        binary: PathBuf,
        /// The TypeScript package's version, e.g. `7.0.2`.
        version: String,
    },
    /// `package.json` asks for TypeScript 7, but it isn't installed.
    NotInstalled,
    /// The project has no TypeScript 7: none, or only an older version.
    Missing {
        /// The older version it has, if any.
        older: Option<String>,
    },
}

/// Looks for tsgo in the project. Reads the disk: call it in the background.
pub(crate) fn detect(root: &Path) -> Detection {
    let Ok(text) = fs::read_to_string(root.join("package.json")) else { return Detection::NoPackageJson };
    // A broken package.json: the toolchain already says so.
    let Ok(package @ Value::Object(_)) = serde_json::from_str::<Value>(&text) else { return Detection::NoPackageJson };
    let node_modules = root.join("node_modules");
    let mut older = None;
    for name in candidates(&package) {
        let folder = node_modules.join(&name);
        let Some(version) = installed_version(&folder) else { continue };
        if major(&version).is_none_or(|major| major < 7) {
            older.get_or_insert(version);
            continue;
        }
        if let Some(binary) = binary_near(&folder, &node_modules) {
            return Detection::Found { binary, version };
        }
    }
    let declared = declared_typescript(&package);
    match declared.as_deref().and_then(major) {
        Some(major) if major >= 7 => Detection::NotInstalled,
        _ => Detection::Missing { older: older.or(declared) },
    }
}

/// `typescript`, then every root dependency that aliases it.
fn candidates(package: &Value) -> Vec<String> {
    let mut names = vec!["typescript".to_owned()];
    for section in SECTIONS {
        for (name, spec) in package[section].as_object().into_iter().flatten() {
            if spec.as_str().is_some_and(|spec| spec.starts_with("npm:typescript@")) && !names.contains(name) {
                names.push(name.clone());
            }
        }
    }
    names
}

fn installed_version(folder: &Path) -> Option<String> {
    let text = fs::read_to_string(folder.join("package.json")).ok()?;
    let package: Value = serde_json::from_str(&text).ok()?;
    package["version"].as_str().map(str::to_owned)
}

/// The tsgo binary in the platform package next to the TypeScript package
/// at `folder`: in a `node_modules` above its real location (pnpm's virtual
/// store), or in the root `node_modules`.
fn binary_near(folder: &Path, root_node_modules: &Path) -> Option<PathBuf> {
    let real = folder.canonicalize().ok()?;
    let above = real.ancestors().skip(1).filter(|dir| dir.file_name().is_some_and(|name| name == "node_modules"));
    above
        .chain([root_node_modules])
        .map(|node_modules| node_modules.join(PLATFORM_PACKAGE).join("lib/tsc"))
        .find(|binary| binary.is_file())
}

/// The version range `package.json` gives TypeScript itself, if any.
fn declared_typescript(package: &Value) -> Option<String> {
    let specs = SECTIONS.iter().flat_map(|section| package[section].as_object().into_iter().flatten());
    let mut aliased = None;
    for (name, spec) in specs {
        let Some(spec) = spec.as_str() else { continue };
        if name == "typescript" && !spec.starts_with("npm:") {
            return Some(spec.to_owned());
        }
        if let Some(range) = spec.strip_prefix("npm:typescript@") {
            aliased.get_or_insert(range.to_owned());
        }
    }
    aliased
}

/// The major version a version or range starts with (`^7.0.2` → 7).
fn major(version: &str) -> Option<u64> {
    let digits: String = version.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// "Add TypeScript 7": `package.json` with `typescript` set to Genea's TS 7
/// range, where the project already lists it, else in `devDependencies`.
/// Everything else, the key order and the indentation are kept.
pub(crate) fn add_typescript(package_json: &str) -> Result<String, String> {
    let mut json: Value = serde_json::from_str(package_json).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let Value::Object(object) = &mut json else { return Err("it isn't a JSON object".into()) };
    let section = SECTIONS
        .into_iter()
        .find(|section| object.get(*section).and_then(|deps| deps.get("typescript")).is_some())
        .unwrap_or("devDependencies");
    let deps = object.entry(section).or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(deps) = deps else { return Err(format!("its {section} isn't an object")) };
    deps.insert("typescript".into(), Value::String(TYPESCRIPT_VERSION.into()));

    let indent = text_indentation(package_json);
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&json, &mut serializer).map_err(|e| e.to_string())?;
    let mut text = String::from_utf8(out).expect("serde_json writes UTF-8");
    if package_json.ends_with('\n') {
        text.push('\n');
    }
    Ok(text)
}

/// The indentation of the first indented line, or two spaces.
fn text_indentation(text: &str) -> &str {
    text.lines().map(|line| &line[..line.len() - line.trim_start().len()]).find(|indent| !indent.is_empty()).unwrap_or("  ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn majors_of_versions_and_ranges() {
        assert_eq!(major("7.0.2"), Some(7));
        assert_eq!(major("^7.1.0-dev.1"), Some(7));
        assert_eq!(major(">=6 <8"), Some(6));
        assert_eq!(major("catalog:"), None);
    }

    #[test]
    fn adding_typescript_replaces_an_older_one_where_it_is_listed() {
        let package = "{\n    \"name\": \"x\",\n    \"dependencies\": { \"typescript\": \"^5.9.0\", \"zod\": \"4\" }\n}\n";
        let added = add_typescript(package).unwrap();
        assert_eq!(
            added,
            "{\n    \"name\": \"x\",\n    \"dependencies\": {\n        \"typescript\": \"^7.0.2\",\n        \"zod\": \"4\"\n    }\n}\n"
        );
    }
}
