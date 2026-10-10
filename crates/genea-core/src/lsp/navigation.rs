//! Code navigation and rename on the wire (ticket #44): the requests'
//! params and their answers as plain data. `project/language/navigation.rs`
//! decides what to ask and applies the answers.
//!
//! - `textDocument/definition`, `typeDefinition`, `implementation` and
//!   `references` answer locations: a `Location`, a list of them, or a list
//!   of `LocationLink`s (Genea takes their `targetSelectionRange`).
//! - `textDocument/prepareRename` answers the range to rename (with or
//!   without a placeholder), `{ defaultBehavior }`, or null where nothing
//!   can be renamed.
//! - `textDocument/rename` answers a `WorkspaceEdit`: `changes` or
//!   `documentChanges` (text edits only; file operations are left out).

use std::path::PathBuf;

use ropey::Rope;
use serde_json::{Value, json};

use super::{
    Output,
    connection::ResponseError,
    text::{self, Encoding},
};
use crate::problems::TextPosition;

/// What a location request looks for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Locate {
    Definition,
    TypeDefinition,
    Implementation,
    Usages,
}

impl Locate {
    pub(crate) fn method(self) -> &'static str {
        match self {
            Locate::Definition => "textDocument/definition",
            Locate::TypeDefinition => "textDocument/typeDefinition",
            Locate::Implementation => "textDocument/implementation",
            Locate::Usages => "textDocument/references",
        }
    }
}

/// A navigation request waiting for its answer, with the project's
/// generation for it (a newer request makes an older answer moot).
#[derive(Debug)]
pub(crate) enum Ask {
    Locate { locate: Locate, generation: u64 },
    PrepareRename { generation: u64 },
    Rename { generation: u64 },
}

/// A position as LSP has it: a line, and a column in the server's
/// encoding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Position {
    pub(crate) line: u32,
    pub(crate) character: u32,
}

impl Position {
    fn from_json(value: &Value) -> Option<Self> {
        Some(Position { line: value["line"].as_u64()? as u32, character: value["character"].as_u64()? as u32 })
    }

    /// Where this is in `text`, clamped to it.
    pub(crate) fn in_text(self, text: &Rope, encoding: Encoding) -> TextPosition {
        text::text_position(text, self.line, self.character, encoding)
    }

    /// A text position (line, char column) as the server counts it.
    pub(crate) fn of(text: &Rope, at: TextPosition, encoding: Encoding) -> Self {
        let line = at.line.min(text.len_lines().saturating_sub(1));
        let character = text
            .line(line)
            .chars()
            .take(at.column)
            .map(|c| match encoding {
                Encoding::Utf8 => c.len_utf8(),
                Encoding::Utf16 => c.len_utf16(),
                Encoding::Utf32 => 1,
            })
            .sum::<usize>();
        Position { line: line as u32, character: character as u32 }
    }

    pub(crate) fn to_json(self) -> Value {
        json!({ "line": self.line, "character": self.character })
    }
}

/// A range in a file, as the server sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Location {
    /// Absolute.
    pub(crate) path: PathBuf,
    pub(crate) start: Position,
    pub(crate) end: Position,
}

/// A text edit of a file, as the server sent it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub(crate) start: Position,
    pub(crate) end: Position,
    pub(crate) text: String,
}

/// A navigation request's answer, for the project.
#[derive(Debug)]
pub(crate) enum Answer {
    Located { locate: Locate, generation: u64, found: Result<Vec<Location>, String> },
    /// The range to rename, with the server's placeholder if it gave one;
    /// `Ok(None)` where nothing can be renamed. A server without
    /// `prepareRename` answers `Err(None)`: rename the word at the caret.
    RenameRange { generation: u64, range: Result<Option<(Position, Position, Option<String>)>, Option<String>> },
    /// The edits per file (absolute paths), in file order.
    Renamed { generation: u64, edits: Result<Vec<(PathBuf, Vec<TextEdit>)>, String> },
}

/// "Method not found": the server doesn't do this.
const METHOD_NOT_FOUND: i64 = -32601;

/// The params of a request at a place in a file.
pub(crate) fn at(uri: &str, position: Position) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": position.to_json() })
}

/// Turns a server's answer into what the project applies.
pub(crate) fn answered(ask: Ask, result: Result<Value, ResponseError>) -> Vec<Output> {
    let answer = match ask {
        Ask::Locate { locate, generation } => {
            let found = result.map(|value| locations(&value)).map_err(|error| error.message);
            Answer::Located { locate, generation, found }
        }
        Ask::PrepareRename { generation } => {
            let range = match result {
                Ok(value) if value.is_null() => Ok(None),
                Ok(value) if value.get("defaultBehavior").is_some() => Err(None),
                Ok(value) => {
                    let range = if value.get("range").is_some() { &value["range"] } else { &value };
                    let placeholder = value["placeholder"].as_str().map(str::to_owned);
                    match (Position::from_json(&range["start"]), Position::from_json(&range["end"])) {
                        (Some(start), Some(end)) => Ok(Some((start, end, placeholder))),
                        _ => Ok(None),
                    }
                }
                Err(error) if error.code == METHOD_NOT_FOUND => Err(None),
                Err(error) => Err(Some(error.message)),
            };
            Answer::RenameRange { generation, range }
        }
        Ask::Rename { generation } => {
            Answer::Renamed { generation, edits: result.map(|value| workspace_edit(&value)).map_err(|e| e.message) }
        }
    };
    vec![Output::Navigation(answer)]
}

/// The locations in a definition-like answer.
fn locations(value: &Value) -> Vec<Location> {
    let items = match value {
        Value::Array(items) => items.iter().collect(),
        Value::Object(_) => vec![value],
        _ => Vec::new(),
    };
    items
        .into_iter()
        .filter_map(|item| {
            let (uri, range) = match item.get("targetUri") {
                Some(uri) => (uri, &item["targetSelectionRange"]),
                None => (&item["uri"], &item["range"]),
            };
            Some(Location {
                path: text::path(uri.as_str()?)?,
                start: Position::from_json(&range["start"])?,
                end: Position::from_json(&range["end"])?,
            })
        })
        .collect()
}

/// The text edits of a `WorkspaceEdit`, per file.
fn workspace_edit(value: &Value) -> Vec<(PathBuf, Vec<TextEdit>)> {
    let mut files: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
    let mut add = |uri: &Value, edits: &Value| {
        let Some(path) = uri.as_str().and_then(text::path) else { return };
        let edits = edits.as_array().into_iter().flatten().filter_map(|edit| {
            Some(TextEdit {
                start: Position::from_json(&edit["range"]["start"])?,
                end: Position::from_json(&edit["range"]["end"])?,
                text: edit["newText"].as_str()?.to_owned(),
            })
        });
        match files.iter_mut().find(|(p, _)| *p == path) {
            Some((_, existing)) => existing.extend(edits),
            None => files.push((path, edits.collect())),
        }
    };
    match value.get("documentChanges").and_then(Value::as_array) {
        Some(changes) => {
            // Text document edits have a `textDocument`; file operations a `kind`.
            for change in changes.iter().filter(|c| c.get("textDocument").is_some()) {
                add(&change["textDocument"]["uri"], &change["edits"]);
            }
        }
        None => {
            for (uri, edits) in value["changes"].as_object().into_iter().flatten() {
                add(&Value::String(uri.clone()), edits);
            }
        }
    }
    files
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locations_come_as_one_a_list_or_links() {
        let range = json!({ "start": { "line": 1, "character": 2 }, "end": { "line": 1, "character": 5 } });
        let expected = vec![Location {
            path: "/p/a.ts".into(),
            start: Position { line: 1, character: 2 },
            end: Position { line: 1, character: 5 },
        }];
        assert_eq!(locations(&json!({ "uri": "file:///p/a.ts", "range": range })), expected);
        assert_eq!(locations(&json!([{ "uri": "file:///p/a.ts", "range": range }])), expected);
        let link = json!({ "targetUri": "file:///p/a.ts", "targetRange": {}, "targetSelectionRange": range });
        assert_eq!(locations(&json!([link])), expected);
        assert_eq!(locations(&Value::Null), []);
    }

    #[test]
    fn workspace_edits_come_as_changes_or_document_changes() {
        let edit = json!({ "range": { "start": { "line": 0, "character": 0 }, "end": { "line": 0, "character": 1 } }, "newText": "b" });
        let expected = vec![(
            PathBuf::from("/p/a.ts"),
            vec![TextEdit { start: Position { line: 0, character: 0 }, end: Position { line: 0, character: 1 }, text: "b".into() }],
        )];
        assert_eq!(workspace_edit(&json!({ "changes": { "file:///p/a.ts": [edit] } })), expected);
        let document = json!({ "textDocument": { "uri": "file:///p/a.ts", "version": 3 }, "edits": [edit] });
        let create = json!({ "kind": "create", "uri": "file:///p/new.ts" });
        assert_eq!(workspace_edit(&json!({ "documentChanges": [create, document] })), expected);
    }

    #[test]
    fn positions_count_in_the_servers_encoding() {
        let text = Rope::from_str("é😀x\n");
        let x = TextPosition { line: 0, column: 2 };
        assert_eq!(Position::of(&text, x, Encoding::Utf8), Position { line: 0, character: 6 });
        assert_eq!(Position::of(&text, x, Encoding::Utf16), Position { line: 0, character: 3 });
        assert_eq!(Position::of(&text, x, Encoding::Utf32).in_text(&text, Encoding::Utf32), x);
    }
}
