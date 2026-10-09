//! The config: `genea.jsonc` at the project root (#13, ticket #29).
//!
//! JSONC (comments and trailing commas), flat camelCase keys. Every key has
//! a default, and a mistake never breaks Genea:
//!
//! - a syntax error falls back to all defaults, with one error;
//! - a wrong type or value (or a bad `exclude` glob) falls back for that key
//!   only, with an error at the value;
//! - an unknown key gives a warning, so a config for a newer Genea loads;
//! - `$schema` is accepted and ignored.
//!
//! A `genea.jsonc` below the root is ignored with a warning (the project
//! does that part, since it needs the file tree). The features that own a
//! key read it from [`Config`]; only `theme` is wired by #29.

use std::path::Path;

use jsonc_parser::{
    CollectOptions, ParseOptions,
    ast::{ObjectPropName, Value},
    common::Ranged,
    parse_to_ast,
};

use crate::problems::{Problem, Severity, TextPosition};

/// The config's file name. It counts only at the project root.
pub(crate) const CONFIG_FILE: &str = "genea.jsonc";

/// The project's effective config: what `genea.jsonc` says, with defaults
/// for everything it leaves out or gets wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Config {
    /// `theme`: built-in light or dark, or follow the system.
    pub theme: Theme,
    /// `terminalPosition`: where the terminal sits next to the editor.
    pub terminal_position: TerminalPosition,
    /// `formatOnSave`: Oxfmt formatting on save (#50).
    pub format_on_save: bool,
    /// `fixOnSave`: Oxlint safe fixes on save (#50).
    pub fix_on_save: bool,
    /// `inlayHints` (#46).
    pub inlay_hints: bool,
    /// `codeLens` (#46).
    pub code_lens: bool,
    /// `exclude`: `.gitignore`-syntax patterns relative to the project root
    /// that the file tree, finder and search hide (#30). Each one is a valid
    /// pattern: a bad one makes the whole key fall back to `[]`.
    pub exclude: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            theme: Theme::System,
            terminal_position: TerminalPosition::Right,
            format_on_save: true,
            fix_on_save: true,
            inlay_hints: false,
            code_lens: false,
            exclude: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Theme {
    /// Follow the system's light or dark appearance.
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum TerminalPosition {
    #[default]
    Right,
    Bottom,
}

/// Reads a config file's text: the config it gives, and its problems, all
/// in `path` (the config's path relative to the project root).
pub(crate) fn parse(text: &str, path: &Path) -> (Config, Vec<Problem>) {
    let mut config = Config::default();
    let mut problems = Vec::new();
    let mut report = |severity, range: jsonc_parser::common::Range, message: String| {
        problems.push(Problem {
            severity,
            path: path.to_owned(),
            start: TextPosition::of_byte_offset(text, range.start),
            end: TextPosition::of_byte_offset(text, range.end),
            message,
        })
    };

    let options = ParseOptions {
        allow_comments: true,
        allow_trailing_commas: true,
        allow_loose_object_property_names: false,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
        allow_bare_decimal_point_numbers: false,
        allow_non_finite_numbers: false,
        allow_extended_string_escapes: false,
    };
    let value = match parse_to_ast(text, &CollectOptions::default(), &options) {
        Ok(parsed) => parsed.value,
        Err(error) => {
            report(Severity::Error, error.range(), format!("{}. The config isn't applied.", error.kind()));
            return (config, problems);
        }
    };
    // An empty file is an empty config.
    let Some(value) = value else { return (config, problems) };
    let Value::Object(object) = value else {
        report(Severity::Error, value.range(), "The config must be an object. It isn't applied.".into());
        return (config, problems);
    };

    for property in &object.properties {
        let key = property.name.as_str();
        let value = &property.value;
        let checked = match key {
            "$schema" => Ok(()),
            "theme" => one_of(value, &[("system", Theme::System), ("light", Theme::Light), ("dark", Theme::Dark)])
                .map(|v| config.theme = v),
            "terminalPosition" => {
                one_of(value, &[("right", TerminalPosition::Right), ("bottom", TerminalPosition::Bottom)])
                    .map(|v| config.terminal_position = v)
            }
            "formatOnSave" => boolean(value).map(|v| config.format_on_save = v),
            "fixOnSave" => boolean(value).map(|v| config.fix_on_save = v),
            "inlayHints" => boolean(value).map(|v| config.inlay_hints = v),
            "codeLens" => boolean(value).map(|v| config.code_lens = v),
            "exclude" => patterns(value).map(|v| config.exclude = v),
            _ => {
                let name_range = match &property.name {
                    ObjectPropName::String(name) => name.range,
                    ObjectPropName::Word(name) => name.range,
                };
                report(Severity::Warning, name_range, format!("Unknown key \"{key}\". It is ignored."));
                Ok(())
            }
        };
        if let Err((range, expected)) = checked {
            report(Severity::Error, range, format!("\"{key}\" {expected}. Using the default."));
        }
    }
    (config, problems)
}

/// Why a value is wrong: where, and what was expected.
type Invalid = (jsonc_parser::common::Range, String);

fn one_of<T: Copy>(value: &Value, choices: &[(&str, T)]) -> Result<T, Invalid> {
    let found = match value {
        Value::StringLit(s) => choices.iter().find(|(name, _)| *name == s.value).map(|(_, v)| *v),
        _ => None,
    };
    found.ok_or_else(|| {
        let names: Vec<String> = choices.iter().map(|(name, _)| format!("\"{name}\"")).collect();
        let (last, rest) = names.split_last().expect("at least one choice");
        (value.range(), format!("must be {} or {last}", rest.join(", ")))
    })
}

fn boolean(value: &Value) -> Result<bool, Invalid> {
    match value {
        Value::BooleanLit(b) => Ok(b.value),
        _ => Err((value.range(), "must be true or false".into())),
    }
}

/// A list of `.gitignore`-syntax patterns, each of which must be valid.
fn patterns(value: &Value) -> Result<Vec<String>, Invalid> {
    let not_a_list = || (value.range(), "must be a list of strings".to_string());
    let Value::Array(array) = value else { return Err(not_a_list()) };
    let mut patterns = Vec::new();
    for element in &array.elements {
        let Value::StringLit(pattern) = element else { return Err(not_a_list()) };
        let mut builder = ignore::gitignore::GitignoreBuilder::new("/");
        if let Err(error) = builder.add_line(None, &pattern.value) {
            let reason = match error {
                ignore::Error::Glob { err, .. } => err,
                other => other.to_string(),
            };
            return Err((pattern.range, format!("has a bad pattern: {reason}")));
        }
        patterns.push(pattern.value.to_string());
    }
    Ok(patterns)
}
