//! Completion, hover and signature help on the protocol side (ticket #43):
//! what Genea tells the server it can show, what the server offers, the
//! requests' params, and their answers as plain data. The project's side
//! (`project/assist.rs`) decides when to ask and whether an answer is still
//! wanted.
//!
//! Answers are read from JSON by hand rather than through `gen-lsp-types`:
//! servers fill these loosely (tsgo leaves out `textEdit` and sends
//! parameter labels as strings), and only a few fields matter.

use std::ops::Range;

use gen_lsp_types::ServerCapabilities;
use serde_json::{Value, json};

use crate::view::{CompletionKind, MarkupBlock};

/// What a server offers of these requests, from its `initialize` answer.
#[derive(Clone, Debug, Default)]
pub(crate) struct AssistCapabilities {
    pub(crate) completion: bool,
    /// Characters that open the completion list when typed (tsgo: `.`,
    /// quotes, `/`, `@`, `<`, `#`, space, …).
    pub(crate) completion_triggers: Vec<String>,
    /// The server fills in details (an auto-import's edit, documentation)
    /// on `completionItem/resolve`.
    pub(crate) resolve: bool,
    pub(crate) hover: bool,
    pub(crate) signature_help: bool,
    /// Characters that open signature help (tsgo: `(`, `,`, `<`).
    pub(crate) signature_triggers: Vec<String>,
    /// Characters that ask again while it is open (tsgo: `)`).
    pub(crate) signature_retriggers: Vec<String>,
}

impl AssistCapabilities {
    pub(crate) fn new(capabilities: &ServerCapabilities) -> Self {
        let capabilities = serde_json::to_value(capabilities).unwrap_or_default();
        let strings = |value: &Value| -> Vec<String> {
            value.as_array().into_iter().flatten().filter_map(Value::as_str).map(str::to_owned).collect()
        };
        let completion = &capabilities["completionProvider"];
        let signature = &capabilities["signatureHelpProvider"];
        let hover = &capabilities["hoverProvider"];
        AssistCapabilities {
            completion: completion.is_object(),
            completion_triggers: strings(&completion["triggerCharacters"]),
            resolve: completion["resolveProvider"].as_bool().unwrap_or(false),
            hover: hover.is_object() || hover.as_bool() == Some(true),
            signature_help: signature.is_object(),
            signature_triggers: strings(&signature["triggerCharacters"]),
            signature_retriggers: strings(&signature["retriggerCharacters"]),
        }
    }
}

/// Adds what Genea can show to `capabilities.textDocument`: plain-text
/// completions (no snippets) resolved for their auto-import edits, and
/// Markdown hover and signature help.
pub(crate) fn client_capabilities(text_document: &mut Value) {
    let markup = json!(["markdown", "plaintext"]);
    text_document["completion"] = json!({
        "completionItem": {
            "snippetSupport": false,
            "documentationFormat": markup,
            "labelDetailsSupport": true,
            "resolveSupport": { "properties": ["documentation", "detail", "additionalTextEdits"] },
        },
        "contextSupport": true,
    });
    text_document["hover"] = json!({ "contentFormat": markup });
    text_document["signatureHelp"] = json!({
        "signatureInformation": {
            "documentationFormat": markup,
            "parameterInformation": { "labelOffsetSupport": true },
            "activeParameterSupport": true,
        },
        "contextSupport": true,
    });
}

/// An LSP position: line and column in the server's encoding.
pub(crate) type Position = (u32, u32);

fn position_json((line, character): Position) -> Value {
    json!({ "line": line, "character": character })
}

fn position(value: &Value) -> Option<Position> {
    Some((value["line"].as_u64()? as u32, value["character"].as_u64()? as u32))
}

/// `textDocument/completion` params. `trigger` is the character typed,
/// if one of the server's trigger characters opened the list.
pub(crate) fn completion_params(uri: &str, at: Position, trigger: Option<&str>) -> Value {
    let context = match trigger {
        Some(character) => json!({ "triggerKind": 2, "triggerCharacter": character }),
        None => json!({ "triggerKind": 1 }),
    };
    json!({ "textDocument": { "uri": uri }, "position": position_json(at), "context": context })
}

/// `textDocument/hover` params.
pub(crate) fn hover_params(uri: &str, at: Position) -> Value {
    json!({ "textDocument": { "uri": uri }, "position": position_json(at) })
}

/// Why signature help is asked for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum SignatureTrigger {
    /// ⌘P.
    Invoked,
    /// A trigger (or, while open, retrigger) character was typed.
    Character(String),
    /// The text or the caret moved while it was open.
    ContentChange,
}

/// `textDocument/signatureHelp` params.
pub(crate) fn signature_help_params(uri: &str, at: Position, trigger: &SignatureTrigger, retrigger: bool) -> Value {
    let mut context = match trigger {
        SignatureTrigger::Invoked => json!({ "triggerKind": 1 }),
        SignatureTrigger::Character(character) => json!({ "triggerKind": 2, "triggerCharacter": character }),
        SignatureTrigger::ContentChange => json!({ "triggerKind": 3 }),
    };
    context["isRetrigger"] = json!(retrigger);
    json!({ "textDocument": { "uri": uri }, "position": position_json(at), "context": context })
}

/// A server's text edit, in its positions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub(crate) start: Position,
    pub(crate) end: Position,
    pub(crate) text: String,
}

fn text_edit(value: &Value) -> Option<TextEdit> {
    // A `TextEdit` has a range; an `InsertReplaceEdit` an insert range.
    let range = value.get("range").or_else(|| value.get("insert"))?;
    Some(TextEdit {
        start: position(&range["start"])?,
        end: position(&range["end"])?,
        text: value["newText"].as_str()?.to_owned(),
    })
}

/// One completion as the server offered it.
#[derive(Clone, Debug)]
pub(crate) struct Completion {
    /// The item as it came, to send back for resolving.
    pub(crate) raw: Value,
    pub(crate) label: String,
    /// What typing is matched against.
    pub(crate) filter: String,
    /// What accepting it inserts.
    pub(crate) insert: String,
    pub(crate) sort: String,
    pub(crate) kind: CompletionKind,
    pub(crate) label_detail: Option<String>,
    pub(crate) source: Option<String>,
    /// Where the server wants it inserted, if it says.
    pub(crate) edit: Option<TextEdit>,
    pub(crate) preselect: bool,
}

/// A `textDocument/completion` answer.
#[derive(Debug, Default)]
pub(crate) struct Completions {
    /// Typing more should ask again rather than narrow this list.
    pub(crate) incomplete: bool,
    pub(crate) items: Vec<Completion>,
}

pub(crate) fn completions(result: Value) -> Completions {
    let (incomplete, items) = match result {
        Value::Array(items) => (false, items),
        Value::Object(mut list) => (
            list.get("isIncomplete").and_then(Value::as_bool).unwrap_or(false),
            match list.remove("items") {
                Some(Value::Array(items)) => items,
                _ => Vec::new(),
            },
        ),
        _ => (false, Vec::new()),
    };
    let items = items.into_iter().filter_map(completion).collect();
    Completions { incomplete, items }
}

fn completion(raw: Value) -> Option<Completion> {
    let label = raw["label"].as_str()?.to_owned();
    let edit = raw.get("textEdit").and_then(text_edit);
    let string = |key: &str| raw[key].as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    let insert = edit.as_ref().map(|e| e.text.clone()).or_else(|| string("insertText")).unwrap_or_else(|| label.clone());
    Some(Completion {
        filter: string("filterText").unwrap_or_else(|| label.clone()),
        sort: string("sortText").unwrap_or_else(|| label.clone()),
        kind: completion_kind(raw["kind"].as_u64()),
        label_detail: raw["labelDetails"]["detail"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
        source: raw["labelDetails"]["description"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
        preselect: raw["preselect"].as_bool().unwrap_or(false),
        insert,
        edit,
        label,
        raw,
    })
}

/// LSP's `CompletionItemKind`, folded into the kinds Genea draws.
fn completion_kind(kind: Option<u64>) -> CompletionKind {
    match kind {
        Some(2) => CompletionKind::Method,
        Some(3) => CompletionKind::Function,
        Some(4) => CompletionKind::Constructor,
        Some(5 | 10) => CompletionKind::Property,
        Some(6) => CompletionKind::Variable,
        Some(7) => CompletionKind::Class,
        Some(8) => CompletionKind::Interface,
        Some(9) => CompletionKind::Module,
        Some(13) => CompletionKind::Enum,
        Some(14) => CompletionKind::Keyword,
        Some(17 | 19) => CompletionKind::File,
        Some(20) => CompletionKind::EnumMember,
        Some(21) => CompletionKind::Constant,
        Some(22) => CompletionKind::Class,
        Some(25) => CompletionKind::TypeParameter,
        _ => CompletionKind::Text,
    }
}

/// What `completionItem/resolve` added to an item.
#[derive(Clone, Debug, Default)]
pub(crate) struct Resolved {
    pub(crate) detail: Option<String>,
    pub(crate) documentation: Vec<MarkupBlock>,
    /// Edits elsewhere in the file: an auto-import's import.
    pub(crate) additional: Vec<TextEdit>,
}

/// An item's details, from a completion or a resolve answer.
pub(crate) fn resolved(item: &Value) -> Resolved {
    Resolved {
        detail: item["detail"].as_str().filter(|s| !s.is_empty()).map(str::to_owned),
        documentation: markup(&item["documentation"]),
        additional: item["additionalTextEdits"].as_array().into_iter().flatten().filter_map(text_edit).collect(),
    }
}

/// A `textDocument/hover` answer: its contents, and where the hovered
/// code starts if the server says.
pub(crate) fn hover(result: &Value) -> Option<(Vec<MarkupBlock>, Option<Position>)> {
    let contents = markup(&result["contents"]);
    (!contents.is_empty()).then(|| (contents, position(&result["range"]["start"])))
}

/// A `textDocument/signatureHelp` answer's active signature.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Signature {
    pub(crate) label: String,
    /// In chars of `label`.
    pub(crate) active_parameter: Option<Range<usize>>,
    pub(crate) documentation: Vec<MarkupBlock>,
    pub(crate) index: usize,
    pub(crate) count: usize,
}

pub(crate) fn signature_help(result: &Value) -> Option<Signature> {
    let signatures = result["signatures"].as_array().filter(|s| !s.is_empty())?;
    let index = (result["activeSignature"].as_u64().unwrap_or(0) as usize).min(signatures.len() - 1);
    let signature = &signatures[index];
    let label = signature["label"].as_str()?.to_owned();
    let active = signature["activeParameter"].as_u64().or_else(|| result["activeParameter"].as_u64());
    let parameter = active.and_then(|i| signature["parameters"].get(i as usize));
    let active_parameter = parameter.and_then(|parameter| match &parameter["label"] {
        Value::String(text) => {
            let byte = label.find(text.as_str())?;
            let start = label[..byte].chars().count();
            Some(start..start + text.chars().count())
        }
        Value::Array(offsets) => {
            // UTF-16 offsets into the label.
            let unit = |i: usize| offsets.get(i).and_then(Value::as_u64).map(|u| u as usize);
            let (from, to) = (unit(0)?, unit(1)?);
            let mut units = 0;
            let (mut start, mut end) = (None, None);
            for (i, c) in label.chars().chain(std::iter::once('\0')).enumerate() {
                if units == from && start.is_none() {
                    start = Some(i);
                }
                if units == to && end.is_none() {
                    end = Some(i);
                }
                units += c.len_utf16();
            }
            Some(start?..end?)
        }
        _ => None,
    });
    let mut documentation = markup(&signature["documentation"]);
    if let Some(parameter) = parameter {
        documentation.extend(markup(&parameter["documentation"]));
    }
    Some(Signature { label, active_parameter, documentation, index, count: signatures.len() })
}

/// Hover contents or documentation: a string, `MarkupContent`, a
/// `MarkedString` or a list of them, split into code and prose.
pub(crate) fn markup(value: &Value) -> Vec<MarkupBlock> {
    match value {
        Value::String(text) => markdown(text),
        Value::Array(items) => items.iter().flat_map(markup).collect(),
        Value::Object(object) => match (object.get("language"), object.get("value").and_then(Value::as_str)) {
            (Some(_), Some(code)) => vec![MarkupBlock::Code(code.trim_end().to_owned())],
            (None, Some(text)) if object.get("kind").and_then(Value::as_str) == Some("plaintext") => {
                let text = text.trim();
                if text.is_empty() { Vec::new() } else { vec![MarkupBlock::Text(text.to_owned())] }
            }
            (None, Some(text)) => markdown(text),
            _ => Vec::new(),
        },
        _ => Vec::new(),
    }
}

/// Splits Markdown into fenced code blocks and the prose between them.
fn markdown(text: &str) -> Vec<MarkupBlock> {
    let mut blocks = Vec::new();
    let mut prose = String::new();
    let mut code: Option<String> = None;
    let flush = |prose: &mut String, blocks: &mut Vec<MarkupBlock>| {
        let text = prose.trim();
        if !text.is_empty() {
            blocks.push(MarkupBlock::Text(text.to_owned()));
        }
        prose.clear();
    };
    for line in text.lines() {
        let fence = line.trim_start().starts_with("```");
        match &mut code {
            Some(lines) if fence => {
                blocks.push(MarkupBlock::Code(lines.trim_end_matches('\n').to_owned()));
                code = None;
            }
            Some(lines) => {
                lines.push_str(line);
                lines.push('\n');
            }
            None if fence => {
                flush(&mut prose, &mut blocks);
                code = Some(String::new());
            }
            None => {
                prose.push_str(line);
                prose.push('\n');
            }
        }
    }
    if let Some(lines) = code {
        blocks.push(MarkupBlock::Code(lines.trim_end_matches('\n').to_owned()));
    }
    flush(&mut prose, &mut blocks);
    blocks
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn markdown_splits_into_code_and_prose() {
        let blocks = markdown("```typescript\nconst count: 1\n```\nHow many.\n\n*@param* `x`\n");
        assert_eq!(
            blocks,
            [MarkupBlock::Code("const count: 1".into()), MarkupBlock::Text("How many.\n\n*@param* `x`".into())]
        );
    }

    #[test]
    fn a_parameter_label_can_be_a_substring_or_utf16_offsets() {
        let by_text = json!({ "signatures": [{ "label": "f(a: é, b: 😀)", "parameters": [{ "label": "a: é" }, { "label": "b: 😀" }], "activeParameter": 1 }] });
        assert_eq!(signature_help(&by_text).unwrap().active_parameter, Some(8..12));
        let by_offsets = json!({ "signatures": [{ "label": "f(a: é, b: 😀)", "parameters": [{ "label": [2, 6] }, { "label": [8, 13] }] }], "activeParameter": 1 });
        assert_eq!(signature_help(&by_offsets).unwrap().active_parameter, Some(8..12));
    }
}
