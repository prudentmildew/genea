//! Oxlint and Oxfmt (spec #19, Toolchain: the linter/formatter role; ticket
//! #49): whether the project has them, where `oxlint --lsp` starts from, and
//! "Add Oxlint and Oxfmt".
//!
//! The role needs both packages in the root `package.json`; without either,
//! lint and format are off. `oxlint`'s `bin/oxlint` is a Node script that
//! loads a native binding, so Genea runs it on the pinned runtime (Node, or
//! Bun) rather than through `.bin`. The standalone binaries are never used
//! (they skip `jsPlugins` and have no Oxfmt `--lsp`). Only the root
//! `node_modules` counts.

use std::{
    fs,
    path::{Path, PathBuf},
};

use jsonc_parser::{JsonValue, ParseOptions, parse_to_value};
use serde_json::{Map, Value};

use super::typescript::{SECTIONS, write_like};
use crate::templates::{OXFMT_VERSION, OXLINT_VERSION};

/// The launcher inside the `oxlint` package.
const OXLINT_SCRIPT: &str = "bin/oxlint";

/// Oxlint's root config files, in the order it prefers them.
pub(crate) const OXLINT_CONFIGS: [&str; 2] = [".oxlintrc.json", ".oxlintrc.jsonc"];

/// What the project has of Oxlint and Oxfmt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum OxcDetection {
    /// No root `package.json`, or one that isn't a JSON object: Genea
    /// doesn't manage linting here, and says nothing.
    NoPackageJson,
    /// `package.json` doesn't list both Oxlint and Oxfmt: lint and format
    /// are off.
    Missing,
    /// Both are listed, but Oxlint isn't in `node_modules`.
    NotInstalled,
    /// Oxlint is installed.
    Found(Oxlint),
}

/// An installed Oxlint, and how the project configures it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Oxlint {
    /// `node_modules/oxlint/bin/oxlint` (absolute), run by the runtime.
    pub(crate) script: PathBuf,
    /// The package's version, e.g. `1.87.0`.
    pub(crate) version: String,
    /// The root `.oxlintrc.json` turns on type-aware linting
    /// (`options.typeAware`). Off otherwise.
    pub(crate) type_aware: bool,
}

/// Looks for Oxlint and Oxfmt in the project. Reads the disk: call it in
/// the background.
pub(crate) fn detect(root: &Path) -> OxcDetection {
    let Ok(text) = fs::read_to_string(root.join("package.json")) else { return OxcDetection::NoPackageJson };
    let Ok(package @ Value::Object(_)) = serde_json::from_str::<Value>(&text) else {
        return OxcDetection::NoPackageJson;
    };
    if !declares(&package, "oxlint") || !declares(&package, "oxfmt") {
        return OxcDetection::Missing;
    }
    let folder = root.join("node_modules/oxlint");
    let script = folder.join(OXLINT_SCRIPT);
    if !script.is_file() {
        return OxcDetection::NotInstalled;
    }
    let version = fs::read_to_string(folder.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|package| package["version"].as_str().map(str::to_owned))
        .unwrap_or_default();
    OxcDetection::Found(Oxlint { script, version, type_aware: type_aware(root) })
}

/// An installed Oxfmt (ticket #50).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Oxfmt {
    /// `node_modules/oxfmt/bin/oxfmt` (absolute), run by the runtime.
    pub(crate) script: PathBuf,
    /// The package's version, e.g. `0.72.0`.
    pub(crate) version: String,
}

/// The launcher inside the `oxfmt` package.
const OXFMT_SCRIPT: &str = "bin/oxfmt";

/// Looks for an installed Oxfmt: `None` unless `package.json` lists both
/// Oxlint and Oxfmt (the role) and `node_modules/oxfmt` has its launcher.
/// Reads the disk: call it in the background.
pub(crate) fn detect_oxfmt(root: &Path) -> Option<Oxfmt> {
    let text = fs::read_to_string(root.join("package.json")).ok()?;
    let package: Value = serde_json::from_str(&text).ok()?;
    if !declares(&package, "oxlint") || !declares(&package, "oxfmt") {
        return None;
    }
    let folder = root.join("node_modules/oxfmt");
    let script = folder.join(OXFMT_SCRIPT);
    if !script.is_file() {
        return None;
    }
    let version = fs::read_to_string(folder.join("package.json"))
        .ok()
        .and_then(|text| serde_json::from_str::<Value>(&text).ok())
        .and_then(|package| package["version"].as_str().map(str::to_owned))
        .unwrap_or_default();
    Some(Oxfmt { script, version })
}

/// The language id of a file Oxfmt formats (ticket #50): the first-class
/// languages, JSON, CSS, YAML, Markdown and HTML. Not `.env`.
pub(crate) fn oxfmt_language_id(path: &Path) -> Option<&'static str> {
    if let Some(id) = super::text::language_id(path) {
        return Some(id);
    }
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match extension.as_str() {
        "json" => "json",
        "jsonc" => "jsonc",
        "css" => "css",
        "yaml" | "yml" => "yaml",
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        _ => return None,
    })
}

/// Whether any dependency section of `package` lists `name`.
fn declares(package: &Value, name: &str) -> bool {
    SECTIONS.iter().any(|section| package[section].get(name).is_some())
}

/// Whether the root Oxlint config says `"options": { "typeAware": true }`.
/// One Genea can't read counts as off.
fn type_aware(root: &Path) -> bool {
    let Some(text) = OXLINT_CONFIGS.iter().find_map(|name| fs::read_to_string(root.join(name)).ok()) else {
        return false;
    };
    let Ok(Some(JsonValue::Object(config))) = parse_to_value(&text, &ParseOptions::default()) else { return false };
    let Some(JsonValue::Object(options)) = config.get("options") else { return false };
    matches!(options.get("typeAware"), Some(JsonValue::Boolean(true)))
}

/// "Add Oxlint and Oxfmt": `package.json` with Genea's Oxlint and Oxfmt
/// ranges added to `devDependencies`, for whichever of the two the project
/// doesn't list yet. Everything else, the key order and the indentation are
/// kept.
pub(crate) fn add_oxc(package_json: &str) -> Result<String, String> {
    let mut json: Value = serde_json::from_str(package_json).map_err(|e| format!("it isn't valid JSON ({e})"))?;
    let missing: Vec<(&str, &str)> = [("oxfmt", OXFMT_VERSION), ("oxlint", OXLINT_VERSION)]
        .into_iter()
        .filter(|(name, _)| !declares(&json, name))
        .collect();
    let Value::Object(object) = &mut json else { return Err("it isn't a JSON object".into()) };
    let deps = object.entry("devDependencies").or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(deps) = deps else { return Err("its devDependencies isn't an object".into()) };
    for (name, version) in missing {
        deps.insert(name.into(), Value::String(version.into()));
    }
    write_like(package_json, &json)
}

