//! What the fake server shows on top of the text (ticket #46): semantic
//! tokens, inlay hints and code lenses, each scripted as text to find in
//! an open document.
//!
//! Like tsgo, it reads the inlay-hint and code-lens settings from the
//! client: after `initialized` it asks `workspace/configuration` for the
//! `typescript` section, and serves hints only if the answer turns some on
//! (`inlayHints.*.enabled`), and lenses only if it turns on
//! `referencesCodeLens.enabled` or `implementationsCodeLens.enabled`.

use serde_json::{Value, json};

use super::{Output, position};

/// tsgo's legend (7.0.2, for a client offering every standard type): the
/// fake's tokens index into it.
pub(super) const TOKEN_TYPES: [&str; 22] = [
    "namespace",
    "class",
    "enum",
    "interface",
    "struct",
    "typeParameter",
    "type",
    "parameter",
    "variable",
    "property",
    "enumMember",
    "decorator",
    "event",
    "function",
    "method",
    "macro",
    "comment",
    "string",
    "keyword",
    "number",
    "regexp",
    "operator",
];
pub(super) const TOKEN_MODIFIERS: [&str; 10] = [
    "declaration",
    "definition",
    "readonly",
    "static",
    "deprecated",
    "abstract",
    "async",
    "modification",
    "documentation",
    "defaultLibrary",
];

/// The scripted tokens, hints and lenses.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Decorations {
    pub tokens: Vec<TokenMarker>,
    pub hints: Vec<HintMarker>,
    pub lenses: Vec<LensMarker>,
}

/// Every occurrence of `text` is a semantic token of this type (a name
/// from tsgo's legend) and modifiers.
#[derive(Clone, Debug, PartialEq)]
pub struct TokenMarker {
    pub text: String,
    pub token_type: String,
    pub modifiers: Vec<String>,
}

/// An inlay hint at every occurrence of `text`: a type hint (kind 1)
/// after it, or a parameter hint (kind 2) before it, padded as tsgo pads
/// them.
#[derive(Clone, Debug, PartialEq)]
pub struct HintMarker {
    pub text: String,
    pub label: String,
    pub parameter: bool,
}

/// A code lens on every occurrence of `text`, which resolves to `title`.
#[derive(Clone, Debug, PartialEq)]
pub struct LensMarker {
    pub text: String,
    pub title: String,
}

impl Decorations {
    pub(super) fn to_json(&self) -> Value {
        let tokens: Vec<Value> = self
            .tokens
            .iter()
            .map(|t| json!({ "text": t.text, "type": t.token_type, "modifiers": t.modifiers }))
            .collect();
        let hints: Vec<Value> =
            self.hints.iter().map(|h| json!({ "text": h.text, "label": h.label, "parameter": h.parameter })).collect();
        let lenses: Vec<Value> = self.lenses.iter().map(|l| json!({ "text": l.text, "title": l.title })).collect();
        json!({ "tokens": tokens, "hints": hints, "lenses": lenses })
    }

    pub(super) fn from_json(value: &Value) -> Self {
        let list = |key: &str| value[key].as_array().cloned().unwrap_or_default();
        let text = |v: &Value, key: &str| v[key].as_str().unwrap_or_default().to_owned();
        Decorations {
            tokens: list("tokens")
                .iter()
                .map(|t| TokenMarker {
                    text: text(t, "text"),
                    token_type: text(t, "type"),
                    modifiers: t["modifiers"].as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect(),
                })
                .collect(),
            hints: list("hints")
                .iter()
                .map(|h| HintMarker { text: text(h, "text"), label: text(h, "label"), parameter: h["parameter"].as_bool().unwrap_or(false) })
                .collect(),
            lenses: list("lenses").iter().map(|l| LensMarker { text: text(l, "text"), title: text(l, "title") }).collect(),
        }
    }
}

/// The capabilities the fake adds to its `initialize` answer.
pub(super) fn capabilities(capabilities: &mut Value) {
    capabilities["semanticTokensProvider"] =
        json!({ "legend": { "tokenTypes": TOKEN_TYPES, "tokenModifiers": TOKEN_MODIFIERS }, "full": true, "range": true });
    capabilities["inlayHintProvider"] = json!(true);
    capabilities["codeLensProvider"] = json!({ "resolveProvider": true });
}

/// After `initialized`: asks for the settings, as tsgo does, if hints or
/// lenses are scripted.
pub(super) fn initialized(script: &Decorations, out: &mut Output<impl std::io::Write>) {
    if script.hints.is_empty() && script.lenses.is_empty() {
        return;
    }
    let items: Vec<Value> = ["js/ts", "typescript", "javascript", "editor"].iter().map(|s| json!({ "section": s })).collect();
    out.request("workspace/configuration", json!({ "items": items }));
}

/// Once the settings are in, asks the client to fetch again what they turn
/// on, as tsgo does: hints or lenses asked for before the settings came
/// were answered with none.
pub(super) fn configured((hints, lenses): (bool, bool), out: &mut Output<impl std::io::Write>) {
    if hints {
        out.request("workspace/inlayHint/refresh", Value::Null);
    }
    if lenses {
        out.request("workspace/codeLens/refresh", Value::Null);
    }
}

/// What the client's `workspace/configuration` answer turns on: (inlay
/// hints, code lenses), from its `typescript` section.
pub(super) fn settings(answer: &Value) -> (bool, bool) {
    let typescript = &answer[1];
    let hints = typescript["inlayHints"].as_object().is_some_and(|hints| {
        hints.values().any(|hint| matches!(&hint["enabled"], Value::Bool(true)) || matches!(&hint["enabled"], Value::String(s) if s != "none"))
    });
    let lenses = typescript["referencesCodeLens"]["enabled"] == true || typescript["implementationsCodeLens"]["enabled"] == true;
    (hints, lenses)
}

/// Every occurrence of `needle` in `text`, as byte ranges, in order.
fn occurrences<'a>(text: &'a str, needle: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    text.match_indices(needle).filter(|_| !needle.is_empty()).map(|(start, found)| (start, start + found.len()))
}

/// `textDocument/semanticTokens/full`: the tokens, relative-encoded.
pub(super) fn semantic_tokens(script: &Decorations, text: &str, utf8: bool) -> Value {
    let mut tokens = Vec::new();
    for marker in &script.tokens {
        let Some(token_type) = TOKEN_TYPES.iter().position(|t| *t == marker.token_type) else { continue };
        let modifiers = marker
            .modifiers
            .iter()
            .filter_map(|m| TOKEN_MODIFIERS.iter().position(|known| known == m))
            .fold(0u32, |bits, index| bits | 1 << index);
        for (start, end) in occurrences(text, &marker.text) {
            let (from, to) = (position(text, start, utf8), position(text, end, utf8));
            let line = from["line"].as_u64().unwrap_or(0);
            let character = from["character"].as_u64().unwrap_or(0);
            let length = to["character"].as_u64().unwrap_or(0) - character;
            tokens.push((line, character, length, token_type, modifiers));
        }
    }
    tokens.sort_unstable();
    let mut data = Vec::new();
    let (mut last_line, mut last_start) = (0, 0);
    for (line, character, length, token_type, modifiers) in tokens {
        let delta_start = if line == last_line { character - last_start } else { character };
        data.extend([line - last_line, delta_start, length, token_type as u64, u64::from(modifiers)]);
        (last_line, last_start) = (line, character);
    }
    json!({ "data": data })
}

/// `textDocument/inlayHint`, if the client's settings turn hints on.
pub(super) fn inlay_hints(script: &Decorations, text: &str, utf8: bool, enabled: bool) -> Value {
    if !enabled {
        return Value::Null;
    }
    let mut hints = Vec::new();
    for marker in &script.hints {
        for (start, end) in occurrences(text, &marker.text) {
            let mut hint = json!({ "label": [{ "value": marker.label }] });
            if marker.parameter {
                hint["position"] = position(text, start, utf8);
                hint["kind"] = json!(2);
                hint["paddingRight"] = json!(true);
            } else {
                hint["position"] = position(text, end, utf8);
                hint["kind"] = json!(1);
                hint["paddingLeft"] = json!(true);
            }
            hints.push(hint);
        }
    }
    Value::Array(hints)
}

/// `textDocument/codeLens`, unresolved, if the client's settings turn
/// lenses on.
pub(super) fn code_lenses(script: &Decorations, text: &str, utf8: bool, enabled: bool) -> Value {
    if !enabled {
        return Value::Null;
    }
    let mut lenses = Vec::new();
    for (index, marker) in script.lenses.iter().enumerate() {
        for (start, end) in occurrences(text, &marker.text) {
            let range = json!({ "start": position(text, start, utf8), "end": position(text, end, utf8) });
            lenses.push(json!({ "range": range, "data": { "marker": index } }));
        }
    }
    Value::Array(lenses)
}

/// `codeLens/resolve`: the lens with its command.
pub(super) fn resolve_code_lens(script: &Decorations, mut lens: Value) -> Value {
    let marker = lens["data"]["marker"].as_u64().and_then(|i| script.lenses.get(i as usize));
    let title = marker.map(|m| m.title.clone()).unwrap_or_default();
    lens["command"] = json!({ "title": title, "command": "" });
    lens
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decorations_survive_json() {
        let decorations = Decorations {
            tokens: vec![TokenMarker { text: "a".into(), token_type: "parameter".into(), modifiers: vec!["readonly".into()] }],
            hints: vec![HintMarker { text: "b".into(), label: ": number".into(), parameter: false }],
            lenses: vec![LensMarker { text: "c".into(), title: "1 reference".into() }],
        };
        assert_eq!(Decorations::from_json(&decorations.to_json()), decorations);
    }

    #[test]
    fn tokens_are_relative_encoded_in_the_documents_order() {
        let script = Decorations {
            tokens: vec![
                TokenMarker { text: "T".into(), token_type: "interface".into(), modifiers: vec![] },
                TokenMarker { text: "x".into(), token_type: "parameter".into(), modifiers: vec!["declaration".into()] },
            ],
            ..Decorations::default()
        };
        let data = semantic_tokens(&script, "f(x: T) {\n  x;\n}", true);
        assert_eq!(data["data"], json!([0, 2, 1, 7, 1, 0, 3, 1, 3, 0, 1, 2, 1, 7, 1]));
    }
}
