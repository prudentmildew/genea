//! Toolchain pins in a project's root `package.json` (ADR 0005):
//!
//! ```json
//! { "devEngines": { "runtime": { "name": "node", "version": "24.18.0" } },
//!   "packageManager": "pnpm@12.10.1" }
//! ```
//!
//! Genea reads them on every open and writes them only on a click, always
//! as exact versions.

use serde_json::{Map, Value};

use crate::{Request, Tool, Version};

/// The two pinned roles.
#[derive(Clone, Debug, PartialEq)]
pub struct Pins {
    /// `devEngines.runtime`: Node or Bun.
    pub runtime: Pin,
    /// `packageManager`: pnpm or Bun.
    pub package_manager: Pin,
}

/// One role's pin.
#[derive(Clone, Debug, PartialEq)]
pub enum Pin {
    /// The field is missing: Genea uses its built-in default.
    Unpinned,
    Pinned { tool: Tool, request: Request },
    /// A tool Genea doesn't run for this role (npm, Yarn, Deno, …). The
    /// role is turned off; nothing is downloaded.
    Foreign(String),
    /// The field is there but can't be understood; the message says why.
    Invalid(String),
}

/// Reads the pins from `package.json`'s text.
pub fn read(package_json: &str) -> Result<Pins, String> {
    let json: Value = serde_json::from_str(package_json).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let Value::Object(json) = json else { return Err("it isn't a JSON object".into()) };
    Ok(Pins { runtime: runtime_pin(&json), package_manager: package_manager_pin(&json) })
}

fn runtime_pin(json: &Map<String, Value>) -> Pin {
    let Some(runtime) = json.get("devEngines").and_then(|d| d.get("runtime")) else { return Pin::Unpinned };
    // devEngines allows a list of acceptable runtimes; take the first one
    // Genea can run, else report the first.
    let candidates: Vec<&Value> = match runtime {
        Value::Array(list) => list.iter().collect(),
        other => vec![other],
    };
    let first_known = candidates
        .iter()
        .find(|c| c.get("name").and_then(Value::as_str).is_some_and(|n| matches!(n, "node" | "bun")));
    let Some(runtime) = first_known.or(candidates.first()) else { return Pin::Unpinned };
    let Some(name) = runtime.get("name").and_then(Value::as_str) else {
        return Pin::Invalid("devEngines.runtime has no name".into());
    };
    let tool = match name {
        "node" => Tool::Node,
        "bun" => Tool::Bun,
        other => return Pin::Foreign(other.to_owned()),
    };
    match runtime.get("version") {
        None => Pin::Pinned { tool, request: Request::any() },
        Some(Value::String(version)) => match Request::parse(version) {
            Some(request) => Pin::Pinned { tool, request },
            None => Pin::Invalid(format!("devEngines.runtime pins {tool} to \"{version}\", which isn't a version")),
        },
        Some(_) => Pin::Invalid("devEngines.runtime.version isn't a string".into()),
    }
}

fn package_manager_pin(json: &Map<String, Value>) -> Pin {
    let Some(field) = json.get("packageManager") else { return Pin::Unpinned };
    let Some(field) = field.as_str() else { return Pin::Invalid("packageManager isn't a string".into()) };
    // `name@version`, optionally followed by Corepack's `+sha512.…` hash.
    let Some((name, version)) = field.split_once('@') else {
        return Pin::Invalid(format!("packageManager \"{field}\" isn't name@version"));
    };
    let version = version.split('+').next().unwrap_or_default();
    let tool = match name {
        "pnpm" => Tool::Pnpm,
        "bun" => Tool::Bun,
        other => return Pin::Foreign(other.to_owned()),
    };
    match Request::parse(version) {
        Some(request) => Pin::Pinned { tool, request },
        None => Pin::Invalid(format!("packageManager pins {tool} to \"{version}\", which isn't a version")),
    }
}

/// Returns `package_json` with exact pins written for the roles given, and
/// everything else kept. Indentation and the trailing newline follow the
/// original file.
pub fn write(
    package_json: &str,
    runtime: Option<(Tool, &Version)>,
    package_manager: Option<(Tool, &Version)>,
) -> Result<String, String> {
    let mut json: Value = serde_json::from_str(package_json).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let Value::Object(object) = &mut json else { return Err("it isn't a JSON object".into()) };
    if let Some((tool, version)) = runtime {
        let pin = Value::Object(Map::from_iter([
            ("name".to_owned(), Value::String(tool.id().into())),
            ("version".to_owned(), Value::String(version.to_string())),
        ]));
        match object.get_mut("devEngines") {
            Some(Value::Object(engines)) => {
                engines.insert("runtime".into(), pin);
            }
            _ => {
                object.insert("devEngines".into(), Value::Object(Map::from_iter([("runtime".to_owned(), pin)])));
            }
        }
    }
    if let Some((tool, version)) = package_manager {
        object.insert("packageManager".into(), Value::String(format!("{}@{version}", tool.id())));
    }

    let indent = indentation(package_json);
    let mut out = Vec::new();
    let formatter = serde_json::ser::PrettyFormatter::with_indent(indent.as_bytes());
    let mut serializer = serde_json::Serializer::with_formatter(&mut out, formatter);
    serde::Serialize::serialize(&json, &mut serializer).map_err(|e| e.to_string())?;
    let mut text = String::from_utf8(out).expect("serde_json writes UTF-8");
    if package_json.ends_with('\n') || package_json.trim().is_empty() {
        text.push('\n');
    }
    Ok(text)
}

/// The indentation of the first indented line, or two spaces.
fn indentation(text: &str) -> String {
    text.lines()
        .map(|line| &line[..line.len() - line.trim_start().len()])
        .find(|indent| !indent.is_empty())
        .unwrap_or("  ")
        .to_owned()
}
