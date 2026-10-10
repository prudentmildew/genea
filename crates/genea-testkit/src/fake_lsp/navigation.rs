//! The fake server's code navigation and rename (ticket #44), on words:
//! it knows identifiers only by their text, which is enough to stand in for
//! tsgo in tests.
//!
//! - A word's **references** are its whole-word occurrences in the
//!   project's script files (`.ts`, `.tsx`, `.js`, … outside `node_modules`
//!   and `.git`), in open documents as the client sent them and in the
//!   other files as they are on disk; files in path order.
//! - Its **definitions** are the occurrences right after a declaration
//!   keyword (`function`, `const`, `let`, `var`, `class`, `interface`,
//!   `type`, `enum`).
//! - Its **type definition** is the definition of the type annotated on its
//!   definition (`const shape: Shape` → `Shape`'s), else its definition.
//! - Its **implementations** are the classes declared with
//!   `implements Word` or `extends Word` (the class names).
//! - **prepareRename** answers the word's range and text, and **rename**
//!   replaces every reference.

use std::{
    collections::{BTreeMap, HashMap},
    fs,
    path::{Path, PathBuf},
};

use serde_json::{Value, json};

use super::{offset, position};

const DECLARATIONS: [&str; 8] = ["function", "const", "let", "var", "class", "interface", "type", "enum"];
const SCRIPTS: [&str; 8] = ["ts", "tsx", "mts", "cts", "js", "jsx", "mjs", "cjs"];

/// What the fake knows: the workspace folder and the open documents.
pub(super) struct Workspace<'a> {
    pub(super) root: Option<&'a Path>,
    pub(super) documents: &'a HashMap<String, String>,
    pub(super) utf8: bool,
}

/// A word's occurrence in a file.
struct Occurrence {
    uri: String,
    /// The file's text, for positions.
    text: std::rc::Rc<str>,
    start: usize,
    end: usize,
}

impl Occurrence {
    fn range(&self, utf8: bool) -> Value {
        json!({ "start": position(&self.text, self.start, utf8), "end": position(&self.text, self.end, utf8) })
    }

    fn location(&self, utf8: bool) -> Value {
        json!({ "uri": self.uri, "range": self.range(utf8) })
    }

    /// The line's text before the occurrence.
    fn before(&self) -> &str {
        let line_start = self.text[..self.start].rfind('\n').map_or(0, |i| i + 1);
        &self.text[line_start..self.start]
    }

    /// The line's text after the occurrence.
    fn after(&self) -> &str {
        let rest = &self.text[self.end..];
        &rest[..rest.find('\n').unwrap_or(rest.len())]
    }

    fn is_declaration(&self) -> bool {
        let before = self.before();
        let trimmed = before.trim_end();
        trimmed.len() < before.len()
            && DECLARATIONS.iter().any(|keyword| {
                trimmed.strip_suffix(keyword).is_some_and(|rest| !rest.chars().next_back().is_some_and(is_word_char))
            })
    }
}

/// Answers a navigation or rename request, or `None` for another method.
pub(super) fn answer(workspace: &Workspace, method: &str, params: &Value) -> Option<Value> {
    let navigation = matches!(
        method,
        "textDocument/definition"
            | "textDocument/typeDefinition"
            | "textDocument/implementation"
            | "textDocument/references"
            | "textDocument/prepareRename"
            | "textDocument/rename"
    );
    if !navigation {
        return None;
    }
    let utf8 = workspace.utf8;
    let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
    let Some(text) = workspace.text(uri) else { return Some(Value::Null) };
    let at = offset(&text, &params["position"], utf8);
    let Some((start, end)) = word_at(&text, at) else { return Some(Value::Null) };
    let word = &text[start..end];
    let occurrences = workspace.occurrences(word);
    let locations = |found: Vec<&Occurrence>| {
        if found.is_empty() { Value::Null } else { Value::Array(found.iter().map(|o| o.location(utf8)).collect()) }
    };
    Some(match method {
        "textDocument/definition" => locations(occurrences.iter().filter(|o| o.is_declaration()).collect()),
        "textDocument/typeDefinition" => {
            let declaration = occurrences.iter().find(|o| o.is_declaration());
            let annotated = declaration.and_then(|o| o.after().trim_start().strip_prefix(':')).and_then(|rest| {
                let rest = rest.trim_start();
                let len = rest.find(|c: char| !is_word_char(c)).unwrap_or(rest.len());
                (len > 0).then(|| rest[..len].to_owned())
            });
            match annotated {
                Some(ty) => locations(workspace.occurrences(&ty).iter().filter(|o| o.is_declaration()).collect()),
                None => locations(declaration.into_iter().collect()),
            }
        }
        "textDocument/implementation" => {
            let classes: Vec<Occurrence> = occurrences
                .iter()
                .filter(|o| ["implements", "extends"].iter().any(|k| o.before().trim_end().ends_with(k)))
                .filter_map(|o| {
                    let before = o.before();
                    let name = before.trim_start().strip_prefix("export ").unwrap_or(before.trim_start());
                    let name = name.strip_prefix("class ")?;
                    let len = name.find(|c: char| !is_word_char(c)).unwrap_or(name.len());
                    let line_start = o.start - before.len();
                    let start = line_start + (before.len() - name.len());
                    Some(Occurrence { uri: o.uri.clone(), text: o.text.clone(), start, end: start + len })
                })
                .collect();
            locations(classes.iter().collect())
        }
        "textDocument/references" => {
            let declarations = params["context"]["includeDeclaration"].as_bool().unwrap_or(true);
            locations(occurrences.iter().filter(|o| declarations || !o.is_declaration()).collect())
        }
        "textDocument/prepareRename" => {
            let range = json!({ "start": position(&text, start, utf8), "end": position(&text, end, utf8) });
            json!({ "range": range, "placeholder": word })
        }
        _ => {
            let new_name = params["newName"].as_str().unwrap_or_default();
            let mut changes: BTreeMap<String, Vec<Value>> = BTreeMap::new();
            for o in &occurrences {
                changes.entry(o.uri.clone()).or_default().push(json!({ "range": o.range(utf8), "newText": new_name }));
            }
            json!({ "changes": changes })
        }
    })
}

impl Workspace<'_> {
    /// A file's text: as the client sent it if it is open, else on disk.
    fn text(&self, uri: &str) -> Option<String> {
        if let Some(text) = self.documents.get(uri) {
            return Some(text.clone());
        }
        fs::read_to_string(path(uri)?).ok()
    }

    /// Every whole-word occurrence of `word` in the project's script files.
    fn occurrences(&self, word: &str) -> Vec<Occurrence> {
        let mut files = Vec::new();
        if let Some(root) = self.root {
            scripts(root, &mut files);
        }
        files.sort();
        let mut found = Vec::new();
        for file in files {
            let uri = uri(&file);
            let Some(text) = self.text(&uri) else { continue };
            let text: std::rc::Rc<str> = text.into();
            for (start, _) in text.match_indices(word) {
                let end = start + word.len();
                let whole = !text[..start].chars().next_back().is_some_and(is_word_char)
                    && !text[end..].chars().next().is_some_and(is_word_char);
                if whole {
                    found.push(Occurrence { uri: uri.clone(), text: text.clone(), start, end });
                }
            }
        }
        found
    }
}

/// The script files under `dir`, outside `node_modules` and `.git`.
fn scripts(dir: &Path, found: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { continue };
        if kind.is_dir() {
            if entry.file_name() != "node_modules" && entry.file_name() != ".git" {
                scripts(&path, found);
            }
        } else if path.extension().and_then(|e| e.to_str()).is_some_and(|e| SCRIPTS.contains(&e)) {
            found.push(path);
        }
    }
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The byte range of the word at (or just before) `at`.
fn word_at(text: &str, at: usize) -> Option<(usize, usize)> {
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word_char(*c)).last().map_or(at, |(i, _)| i);
    let end = at + text[at..].find(|c: char| !is_word_char(c)).unwrap_or(text.len() - at);
    (start < end).then_some((start, end))
}

/// The `file:` URI of a path, encoded as Genea encodes them.
pub(crate) fn uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(*byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The path of a `file:` URI.
pub(crate) fn path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match rest.get(i + 1..i + 3).filter(|_| bytes[i] == b'%').and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
            Some(byte) => {
                decoded.push(byte);
                i += 3;
            }
            None => {
                decoded.push(bytes[i]);
                i += 1;
            }
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_word_around_a_place() {
        assert_eq!(word_at("a greet(x)", 4), Some((2, 7)));
        assert_eq!(word_at("a greet(x)", 7), Some((2, 7)), "just after the word");
        assert_eq!(word_at("a ( b", 2), None);
    }
}
