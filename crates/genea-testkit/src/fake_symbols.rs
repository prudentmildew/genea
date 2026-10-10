//! The fake LSP server's symbols (ticket #47): `textDocument/documentSymbol`
//! and `workspace/symbol`, from a rough reading of the declarations in a
//! file, enough for tests to write ordinary TypeScript and get its symbols:
//!
//! - top-level `function`, `class`, `interface`, `enum`, `type`,
//!   `namespace`, `const`, `let` and `var` (after `export`, `default`,
//!   `declare`, `async`, `abstract`), one per line, after an optional
//!   leading `/* … */` comment;
//! - the members of a class, interface or enum, one level deep: methods
//!   (`name(`), the constructor, properties (`name:`, `name =`, `name;`)
//!   and enum members.
//!
//! Braces are counted naively per line. `workspace/symbol` reads every
//! first-class-language file under the workspace root (`node_modules` too,
//! as tsgo can answer with a dependency's declarations; not dot folders),
//! using the open document's text where there is one, and matches names
//! containing the query, ignoring case.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use crate::fake_lsp::{position, uri};

/// LSP's `SymbolKind`s the fake reports.
const NAMESPACE: u32 = 3;
const CLASS: u32 = 5;
const METHOD: u32 = 6;
const PROPERTY: u32 = 7;
const CONSTRUCTOR: u32 = 9;
const ENUM: u32 = 10;
const INTERFACE: u32 = 11;
const FUNCTION: u32 = 12;
const VARIABLE: u32 = 13;
const CONSTANT: u32 = 14;
const ENUM_MEMBER: u32 = 22;
const TYPE_PARAMETER: u32 = 26;

/// A declaration found in a file. Offsets are bytes into the file's text.
struct Declaration {
    name: String,
    kind: u32,
    /// Where the declaration starts (its first keyword) and ends (the end
    /// of its last line).
    start: usize,
    end: usize,
    /// The name.
    name_start: usize,
    children: Vec<Declaration>,
}

/// The answer to `textDocument/documentSymbol` for a document's text:
/// `DocumentSymbol[]` when the client supports the hierarchy, or else
/// `SymbolInformation[]`.
pub(crate) fn document_symbols(uri: &str, text: &str, utf8: bool, hierarchical: bool) -> Value {
    let declarations = declarations(text);
    if hierarchical {
        Value::Array(declarations.iter().map(|d| document_symbol(text, d, utf8)).collect())
    } else {
        let mut symbols = Vec::new();
        for declaration in &declarations {
            symbols.push(information(uri, text, declaration, None, utf8));
            for child in &declaration.children {
                symbols.push(information(uri, text, child, Some(&declaration.name), utf8));
            }
        }
        Value::Array(symbols)
    }
}

/// The answer to `workspace/symbol`: `SymbolInformation[]` for every
/// declaration whose name contains `query` (ignoring case) in the files
/// under `root`.
pub(crate) fn workspace_symbols(query: &str, root: Option<&Path>, documents: &HashMap<String, String>, utf8: bool) -> Value {
    let query = query.to_lowercase();
    let mut files: Vec<(String, String)> = Vec::new();
    if let Some(root) = root {
        let mut paths = Vec::new();
        source_files(root, &mut paths);
        paths.sort();
        for path in paths {
            let uri = uri(&path);
            let text = match documents.get(&uri) {
                Some(text) => text.clone(),
                None => fs::read_to_string(&path).unwrap_or_default(),
            };
            files.push((uri, text));
        }
    }
    let mut symbols = Vec::new();
    let matches = |name: &str| name.to_lowercase().contains(&query);
    for (uri, text) in &files {
        for declaration in declarations(text) {
            if matches(&declaration.name) {
                symbols.push(information(uri, text, &declaration, None, utf8));
            }
            for child in &declaration.children {
                if matches(&child.name) {
                    symbols.push(information(uri, text, child, Some(&declaration.name), utf8));
                }
            }
        }
    }
    Value::Array(symbols)
}

fn document_symbol(text: &str, declaration: &Declaration, utf8: bool) -> Value {
    let children: Vec<Value> = declaration.children.iter().map(|c| document_symbol(text, c, utf8)).collect();
    json!({
        "name": declaration.name,
        "kind": declaration.kind,
        "range": range(text, declaration.start, declaration.end, utf8),
        "selectionRange": range(text, declaration.name_start, declaration.name_start + declaration.name.len(), utf8),
        "children": children,
    })
}

/// A `SymbolInformation`, located at the declaration's name.
fn information(uri: &str, text: &str, declaration: &Declaration, container: Option<&str>, utf8: bool) -> Value {
    let name_end = declaration.name_start + declaration.name.len();
    let mut symbol = json!({
        "name": declaration.name,
        "kind": declaration.kind,
        "location": { "uri": uri, "range": range(text, declaration.name_start, name_end, utf8) },
    });
    if let Some(container) = container {
        symbol["containerName"] = json!(container);
    }
    symbol
}

fn range(text: &str, start: usize, end: usize, utf8: bool) -> Value {
    json!({ "start": position(text, start, utf8), "end": position(text, end, utf8) })
}

/// The declarations in `text`, top level first, with their members.
fn declarations(text: &str) -> Vec<Declaration> {
    let mut top: Vec<Declaration> = Vec::new();
    let mut depth = 0i32;
    // Whether the last top-level declaration has members (a class,
    // interface or enum) whose body is open.
    let mut in_body = false;
    let mut offset = 0;
    for line in text.split_inclusive('\n') {
        let content = line.trim_end_matches(['\n', '\r']);
        let line_end = offset + content.len();
        if depth == 0 {
            in_body = false;
            if let Some((kind, keyword_at, name_at, name)) = top_level(content) {
                in_body = matches!(kind, CLASS | INTERFACE | ENUM);
                top.push(Declaration {
                    name,
                    kind,
                    start: offset + keyword_at,
                    end: line_end,
                    name_start: offset + name_at,
                    children: Vec::new(),
                });
            }
        } else if depth == 1
            && in_body
            && let Some(parent) = top.last_mut()
            && let Some((kind, name_at, name)) = member(content, parent.kind)
        {
            let start = offset + name_at;
            parent.children.push(Declaration { name, kind, start, end: line_end, name_start: start, children: Vec::new() });
        }
        let before = depth;
        depth += content.matches('{').count() as i32 - content.matches('}').count() as i32;
        depth = depth.max(0);
        // A declaration's range runs to the line its body closes on.
        if (before > 0 || depth > 0)
            && let Some(last) = top.last_mut()
        {
            last.end = line_end;
        }
        offset += line.len();
    }
    top
}

/// A top-level declaration on a line: (kind, offset of its first keyword,
/// offset of its name, name).
fn top_level(line: &str) -> Option<(u32, usize, usize, String)> {
    let mut at = skip_comment(line);
    let start = skip_spaces(line, at);
    at = start;
    loop {
        let (keyword, next) = word(line, at)?;
        let kind = match keyword {
            "export" | "default" | "declare" | "async" | "abstract" => {
                at = skip_spaces(line, next);
                continue;
            }
            "function" => FUNCTION,
            "class" => CLASS,
            "interface" => INTERFACE,
            "enum" => ENUM,
            "type" => TYPE_PARAMETER,
            "namespace" => NAMESPACE,
            "const" => CONSTANT,
            "let" | "var" => VARIABLE,
            _ => return None,
        };
        let mut name_at = skip_spaces(line, next);
        if line[name_at..].starts_with('*') {
            name_at = skip_spaces(line, name_at + 1);
        }
        let (name, _) = word(line, name_at)?;
        return Some((kind, start, name_at, name.to_owned()));
    }
}

/// A member of a class, interface or enum on a line: (kind, offset of its
/// name, name).
fn member(line: &str, parent: u32) -> Option<(u32, usize, String)> {
    let mut at = skip_spaces(line, 0);
    loop {
        let (name, next) = word(line, at)?;
        if parent != ENUM
            && matches!(
                name,
                "public" | "private" | "protected" | "static" | "readonly" | "async" | "get" | "set" | "override" | "abstract"
            )
            && word(line, skip_spaces(line, next)).is_some()
        {
            at = skip_spaces(line, next);
            continue;
        }
        let after = line[next..].trim_start();
        let kind = if parent == ENUM {
            ENUM_MEMBER
        } else if name == "constructor" {
            CONSTRUCTOR
        } else if after.starts_with('(') || after.starts_with("?(") || after.starts_with('<') {
            METHOD
        } else if after.starts_with([':', '=', ';', '?']) || after.is_empty() {
            PROPERTY
        } else {
            return None;
        };
        return Some((kind, at, name.to_owned()));
    }
}

/// Skips a leading `/* … */` comment.
fn skip_comment(line: &str) -> usize {
    let at = skip_spaces(line, 0);
    if line[at..].starts_with("/*")
        && let Some(end) = line[at..].find("*/")
    {
        return at + end + 2;
    }
    at
}

fn skip_spaces(line: &str, at: usize) -> usize {
    at + line[at..].len() - line[at..].trim_start().len()
}

/// The identifier at `at`, and the offset after it.
fn word(line: &str, at: usize) -> Option<(&str, usize)> {
    let rest = &line[at..];
    let len = rest.find(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')).unwrap_or(rest.len());
    (len > 0).then(|| (&rest[..len], at + len))
}

/// The first-class-language files under `folder`, without dot folders.
fn source_files(folder: &Path, paths: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(folder) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if path.is_dir() {
            if !name.starts_with('.') {
                source_files(&path, paths);
            }
        } else if matches!(path.extension().and_then(|e| e.to_str()), Some("ts" | "tsx" | "js" | "jsx" | "mts" | "cts" | "mjs" | "cjs")) {
            paths.push(path);
        }
    }
}
