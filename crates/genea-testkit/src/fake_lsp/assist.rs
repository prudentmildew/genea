//! The fake server's completion, hover and signature help (ticket #43),
//! scripted per test like the rest of [`LspScript`](super::LspScript).
//!
//! - **Completion** answers every `textDocument/completion` with the
//!   scripted items, unfiltered (tsgo doesn't filter by the typed word
//!   either), and `completionItem/resolve` with the item plus what the
//!   script adds for its label (an auto-import's `additionalTextEdits`, its
//!   documentation).
//! - **Hover** looks up the word at the position.
//! - **Signature help** finds the call whose parentheses the position is
//!   in, looks up the name before them, and counts commas for the active
//!   parameter.

use serde_json::{Value, json};

/// What the fake answers for completion, hover and signature help.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AssistScript {
    /// LSP `CompletionItem`s, returned as they are for every completion.
    pub completions: Vec<Value>,
    /// Fields `completionItem/resolve` adds to the item with this label.
    pub resolved: Vec<(String, Value)>,
    /// Markdown shown for a word under the position.
    pub hovers: Vec<(String, String)>,
    /// Signatures of functions, by the name called.
    pub signatures: Vec<FakeSignature>,
}

/// One function's signature, for signature help.
#[derive(Clone, Debug, PartialEq)]
pub struct FakeSignature {
    /// The name before the `(`.
    pub function: String,
    /// The whole signature, e.g. `add(a: number, b: number): number`.
    pub label: String,
    /// Each parameter's label: a substring of `label`.
    pub parameters: Vec<String>,
    /// Markdown.
    pub documentation: String,
}

impl AssistScript {
    pub(super) fn to_json(&self) -> Value {
        let signatures: Vec<Value> = self
            .signatures
            .iter()
            .map(|s| json!({ "function": s.function, "label": s.label, "parameters": s.parameters, "documentation": s.documentation }))
            .collect();
        json!({
            "completions": self.completions,
            "resolved": self.resolved.iter().map(|(label, patch)| json!([label, patch])).collect::<Vec<_>>(),
            "hovers": self.hovers.iter().map(|(word, text)| json!([word, text])).collect::<Vec<_>>(),
            "signatures": signatures,
        })
    }

    pub(super) fn from_json(value: &Value) -> Self {
        let pairs = |key: &str| -> Vec<(String, Value)> {
            value[key]
                .as_array()
                .into_iter()
                .flatten()
                .map(|pair| (pair[0].as_str().unwrap_or_default().to_owned(), pair[1].clone()))
                .collect()
        };
        AssistScript {
            completions: value["completions"].as_array().cloned().unwrap_or_default(),
            resolved: pairs("resolved"),
            hovers: pairs("hovers").into_iter().map(|(word, text)| (word, text.as_str().unwrap_or_default().to_owned())).collect(),
            signatures: value["signatures"]
                .as_array()
                .into_iter()
                .flatten()
                .map(|s| FakeSignature {
                    function: s["function"].as_str().unwrap_or_default().to_owned(),
                    label: s["label"].as_str().unwrap_or_default().to_owned(),
                    parameters: s["parameters"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                    documentation: s["documentation"].as_str().unwrap_or_default().to_owned(),
                })
                .collect(),
        }
    }
}

/// The server capabilities for these requests, as tsgo announces them.
pub(super) fn capabilities() -> Value {
    json!({
        "completionProvider": { "triggerCharacters": [".", "\"", "'", "`", "/", "@", "<", "#", " "], "resolveProvider": true },
        "hoverProvider": true,
        "signatureHelpProvider": { "triggerCharacters": ["(", ",", "<"], "retriggerCharacters": [")"] },
    })
}

/// The answer to one of these requests, or `None` for another method.
/// `text` is the document's, `offset` turns an LSP position into a byte
/// offset in it and `position` back.
pub(super) fn answer(
    script: &AssistScript,
    method: &str,
    params: &Value,
    text: Option<&str>,
    offset: impl Fn(&str, &Value) -> usize,
    position: impl Fn(&str, usize) -> Value,
) -> Option<Value> {
    match method {
        "textDocument/completion" => Some(json!({ "isIncomplete": false, "items": script.completions })),
        "completionItem/resolve" => {
            let mut item = params.clone();
            let label = item["label"].as_str().unwrap_or_default().to_owned();
            if let Some((_, patch)) = script.resolved.iter().find(|(l, _)| *l == label)
                && let (Some(item), Some(patch)) = (item.as_object_mut(), patch.as_object())
            {
                item.extend(patch.clone());
            }
            Some(item)
        }
        "textDocument/hover" => {
            let text = text.unwrap_or_default();
            let at = offset(text, &params["position"]);
            let (start, end) = word_at(text, at);
            let word = &text[start..end];
            let hover = script.hovers.iter().find(|(w, _)| w == word).map(|(_, markdown)| {
                let range = json!({ "start": position(text, start), "end": position(text, end) });
                json!({ "contents": { "kind": "markdown", "value": markdown }, "range": range })
            });
            Some(hover.unwrap_or(Value::Null))
        }
        "textDocument/signatureHelp" => {
            let text = text.unwrap_or_default();
            let at = offset(text, &params["position"]);
            let help = call_at(text, at).and_then(|(function, argument)| {
                let signature = script.signatures.iter().find(|s| s.function == function)?;
                let parameters: Vec<Value> = signature.parameters.iter().map(|p| json!({ "label": p })).collect();
                let documentation = json!({ "kind": "markdown", "value": signature.documentation });
                let info = json!({ "label": signature.label, "documentation": documentation, "parameters": parameters });
                Some(json!({ "signatures": [info], "activeSignature": 0, "activeParameter": argument }))
            });
            Some(help.unwrap_or(Value::Null))
        }
        _ => None,
    }
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_' || c == '$'
}

/// The byte range of the word around `at` (empty if there is none).
fn word_at(text: &str, at: usize) -> (usize, usize) {
    let start = text[..at].char_indices().rev().take_while(|(_, c)| is_word(*c)).last().map_or(at, |(i, _)| i);
    let end = text[at..].char_indices().find(|(_, c)| !is_word(*c)).map_or(text.len(), |(i, _)| at + i);
    (start, end)
}

/// The function called by the innermost open parenthesis before `at`, and
/// which argument `at` is in (0-based).
fn call_at(text: &str, at: usize) -> Option<(String, usize)> {
    let mut depth = 0;
    let mut commas = 0;
    for (i, c) in text[..at].char_indices().rev() {
        match c {
            ')' => depth += 1,
            '(' if depth > 0 => depth -= 1,
            '(' => {
                let (start, _) = word_at(text, i);
                return (start < i).then(|| (text[start..i].to_owned(), commas));
            }
            ',' if depth == 0 => commas += 1,
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_the_call_and_argument_around_a_position() {
        assert_eq!(call_at("add(1, f(2, 3), ", 16), Some(("add".into(), 2)));
        assert_eq!(call_at("add(1", 5), Some(("add".into(), 0)));
        assert_eq!(call_at("add(1) ", 7), None);
    }

    #[test]
    fn finds_the_word_around_a_position() {
        assert_eq!(word_at("let count = 1", 6), (4, 9));
        assert_eq!(word_at("let count = 1", 9), (4, 9));
        assert_eq!(word_at("let count = 1", 10), (10, 10));
    }
}
