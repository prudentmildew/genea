//! The fake server's code actions (ticket #45): quick fixes and organize
//! imports, scripted as text to find and replace.

use serde_json::{Value, json};

/// A code action the fake offers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ScriptedAction {
    /// The LSP kind: `quickfix`, `source.organizeImports`, …
    pub kind: String,
    pub title: String,
    /// Offered only when the requested range touches an occurrence of this
    /// text; empty offers it anywhere.
    pub at: String,
    /// What it changes in the requesting document, as LSP edits against its
    /// text when asked: each replaces the first occurrence of its `find`
    /// (an empty `find` inserts at the start of the document).
    pub edits: Vec<(String, String)>,
}

impl ScriptedAction {
    pub(super) fn to_json(&self) -> Value {
        let edits: Vec<Value> = self.edits.iter().map(|(find, replace)| json!([find, replace])).collect();
        json!({ "kind": self.kind, "title": self.title, "at": self.at, "edits": edits })
    }

    pub(super) fn from_json(value: &Value) -> Self {
        let text = |key: &str| value[key].as_str().unwrap_or_default().to_owned();
        let edits = value["edits"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|edit| {
                let part = |i: usize| edit[i].as_str().unwrap_or_default().to_owned();
                (part(0), part(1))
            })
            .collect();
        ScriptedAction { kind: text("kind"), title: text("title"), at: text("at"), edits }
    }
}

/// The answer to `textDocument/codeAction`: the scripted actions whose
/// kind `context.only` allows and that apply at the requested range, with
/// their edits for `text`.
pub(super) fn code_actions(actions: &[ScriptedAction], params: &Value, text: &str, utf8: bool) -> Value {
    let uri = params["textDocument"]["uri"].as_str().unwrap_or_default();
    let range = (
        super::offset(text, &params["range"]["start"], utf8),
        super::offset(text, &params["range"]["end"], utf8),
    );
    let only: Option<Vec<&str>> =
        params["context"]["only"].as_array().map(|kinds| kinds.iter().filter_map(Value::as_str).collect());
    let wanted = |kind: &str| {
        only.as_ref().is_none_or(|only| only.iter().any(|o| kind == *o || kind.starts_with(&format!("{o}."))))
    };
    let applies = |at: &str| {
        at.is_empty() || text.match_indices(at).any(|(start, _)| start <= range.1 && range.0 <= start + at.len())
    };
    let answers: Vec<Value> = actions
        .iter()
        .filter(|action| wanted(&action.kind) && applies(&action.at))
        .map(|action| {
            let edits = edits(&action.edits, text, utf8);
            json!({ "title": action.title, "kind": action.kind, "edit": { "changes": { uri: edits } } })
        })
        .collect();
    Value::Array(answers)
}

/// LSP text edits against `text`, each replacing the first occurrence of
/// its find text (an empty find inserts at the start).
pub(super) fn edits(edits: &[(String, String)], text: &str, utf8: bool) -> Vec<Value> {
    edits
        .iter()
        .map(|(find, replace)| {
            let start = if find.is_empty() { Some(0) } else { text.find(find.as_str()) };
            let start = start.unwrap_or(text.len());
            let end = if find.is_empty() || start == text.len() { start } else { start + find.len() };
            let (start, end) = (super::position(text, start, utf8), super::position(text, end, utf8));
            let range = json!({ "start": start, "end": end });
            json!({ "range": range, "newText": replace })
        })
        .collect()
}
