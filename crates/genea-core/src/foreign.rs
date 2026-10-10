//! Foreign formatters and linters (ticket #51, ADR 0001): ESLint, Prettier,
//! Biome and dprint config at the project root or a package root. Genea
//! never runs these tools; their config turns off format and fix on save
//! (`Project::format_on_save`, `Project::fix_on_save`), so Genea never
//! fights the project's own tooling.
//!
//! Found by the workspace model (`workspace.rs`), in the same background
//! read as the packages, and again whenever the watcher sees one of the
//! config files below (or a `package.json`) change.

use std::{
    fs,
    path::{Path, PathBuf},
};

use serde_json::Value;

use crate::problems::{Problem, Severity, TextPosition};

/// Each tool's config files, by name.
const CONFIG_FILES: [(&str, &[&str]); 4] = [
    (
        "ESLint",
        &[
            "eslint.config.js",
            "eslint.config.mjs",
            "eslint.config.cjs",
            "eslint.config.ts",
            "eslint.config.mts",
            "eslint.config.cts",
            ".eslintrc",
            ".eslintrc.js",
            ".eslintrc.cjs",
            ".eslintrc.yaml",
            ".eslintrc.yml",
            ".eslintrc.json",
        ],
    ),
    (
        "Prettier",
        &[
            ".prettierrc",
            ".prettierrc.json",
            ".prettierrc.yaml",
            ".prettierrc.yml",
            ".prettierrc.json5",
            ".prettierrc.js",
            ".prettierrc.cjs",
            ".prettierrc.mjs",
            ".prettierrc.ts",
            ".prettierrc.cts",
            ".prettierrc.mts",
            ".prettierrc.toml",
            "prettier.config.js",
            "prettier.config.cjs",
            "prettier.config.mjs",
            "prettier.config.ts",
            "prettier.config.cts",
            "prettier.config.mts",
        ],
    ),
    ("Biome", &["biome.json", "biome.jsonc", ".biome.json", ".biome.jsonc"]),
    ("dprint", &["dprint.json", "dprint.jsonc", ".dprint.json", ".dprint.jsonc"]),
];

/// `package.json` keys that configure a tool.
const PACKAGE_JSON_KEYS: [(&str, &str); 2] = [("eslintConfig", "ESLint"), ("prettier", "Prettier")];

/// A foreign formatter or linter's config.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ForeignConfig {
    /// `ESLint`, `Prettier`, `Biome` or `dprint`.
    pub(crate) tool: &'static str,
    /// The file, relative to the project root.
    pub(crate) path: PathBuf,
    /// Where in it: the `package.json` key, else the start.
    pub(crate) at: TextPosition,
}

/// Whether a file named `name` may configure a foreign formatter or linter.
pub(crate) fn is_config_file(name: &str) -> bool {
    CONFIG_FILES.iter().any(|(_, files)| files.contains(&name))
}

/// The foreign configs in each package folder (relative to `root`; empty
/// for the root), in that order (blocking).
pub(crate) fn find<'a>(root: &Path, packages: impl IntoIterator<Item = &'a Path>) -> Vec<ForeignConfig> {
    let mut found = Vec::new();
    for package in packages {
        let folder = root.join(package);
        for (tool, files) in CONFIG_FILES {
            for file in files.iter().filter(|file| folder.join(file).is_file()) {
                found.push(ForeignConfig { tool, path: package.join(file), at: TextPosition::default() });
            }
        }
        let Ok(text) = fs::read_to_string(folder.join("package.json")) else { continue };
        let Ok(Value::Object(manifest)) = serde_json::from_str::<Value>(&text) else { continue };
        for (key, tool) in PACKAGE_JSON_KEYS.into_iter().filter(|(key, _)| manifest.contains_key(*key)) {
            let at = text.find(&format!("\"{key}\"")).map(|i| TextPosition::of_byte_offset(&text, i)).unwrap_or_default();
            found.push(ForeignConfig { tool, path: package.join("package.json"), at });
        }
    }
    found
}

/// The tools configured, each once, in the order found.
pub(crate) fn tools(configs: &[ForeignConfig]) -> Vec<&'static str> {
    let mut tools: Vec<&'static str> = Vec::new();
    for config in configs {
        if !tools.contains(&config.tool) {
            tools.push(config.tool);
        }
    }
    tools
}

/// The one warning for every foreign config, on the first one; `None`
/// without any.
pub(crate) fn warning(configs: &[ForeignConfig]) -> Option<Problem> {
    let first = configs.first()?;
    let tools = tools(configs);
    let paths: Vec<String> = configs.iter().map(|c| c.path.display().to_string()).collect();
    let (verb, them) = if tools.len() == 1 { ("is", tools[0]) } else { ("are", "them") };
    let message = format!(
        "{} {verb} configured in {}. Genea doesn't run {them}, so format and fix on save are off.",
        and_list(&tools),
        and_list(&paths),
    );
    Some(Problem { severity: Severity::Warning, path: first.path.clone(), start: first.at, end: first.at, message })
}

/// `a`, `a and b`, `a, b and c`.
fn and_list(items: &[impl AsRef<str>]) -> String {
    match items {
        [] => String::new(),
        [only] => only.as_ref().to_owned(),
        [rest @ .., last] => {
            format!("{} and {}", rest.iter().map(AsRef::as_ref).collect::<Vec<_>>().join(", "), last.as_ref())
        }
    }
}
