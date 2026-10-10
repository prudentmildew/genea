//! Code actions (ticket #45): quick fixes for the problems at the caret,
//! and organize imports. Generic over servers: each server is asked with
//! its own diagnostics, and answers with edits in its own encoding (tsgo
//! now; Oxlint's fixes later, #49).
//!
//! Only actions that carry their edit are offered (tsgo's all do): an
//! action that needs a `codeAction/resolve` or runs a command has nothing
//! Genea can apply, and is left out.

use std::{ops::Range, path::PathBuf};

use gen_lsp_types::{
    ClientCodeActionKindOptions, ClientCodeActionLiteralOptions, CodeActionClientCapabilities, CodeActionKind,
    Diagnostic, Position,
};
use ropey::Rope;
use serde_json::{Value, json};

use super::{LanguageServer, Output, Pending, connection::ResponseError, text, text::Encoding};

/// What a code-action request is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActionKind {
    /// ⌥⏎: quick fixes for the problems at the caret.
    QuickFix,
    /// ⌃⌥O: `source.organizeImports` for the whole file.
    OrganizeImports,
}

/// One code action a server offered, ready to apply.
#[derive(Clone, Debug)]
pub(crate) struct CodeActionFix {
    pub(crate) title: String,
    /// The LSP kind, e.g. `quickfix` or `source.organizeImports`.
    pub(crate) kind: String,
    /// The columns of `edits`.
    pub(crate) encoding: Encoding,
    /// The edits, per file (absolute paths), against the text the server
    /// had when it was asked.
    pub(crate) edits: Vec<(PathBuf, Vec<TextEdit>)>,
}

/// An LSP text edit: a range (line, column in the server's encoding) and
/// what replaces it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct TextEdit {
    pub(crate) start: (u32, u32),
    pub(crate) end: (u32, u32),
    pub(crate) text: String,
}

/// What Genea tells servers about code actions: it takes `CodeAction`
/// literals (not just commands), and shows disabled ones by leaving them
/// out.
pub(super) fn client_capabilities() -> CodeActionClientCapabilities {
    let kinds = [CodeActionKind::QuickFix, CodeActionKind::Source, CodeActionKind::SourceOrganizeImports];
    CodeActionClientCapabilities {
        code_action_literal_support: Some(ClientCodeActionLiteralOptions {
            code_action_kind: ClientCodeActionKindOptions { value_set: kinds.into() },
        }),
        disabled_support: Some(true),
        ..Default::default()
    }
}

impl LanguageServer {
    /// Asks for code actions in an open document over `selection` (char
    /// indices into `text`, the editor's text as last synced). Quick fixes
    /// go with the diagnostics this server reported there, or else on the
    /// selection's lines; organize imports covers the whole file. The
    /// answer comes back as [`Output::CodeActions`] with `ticket`. Returns
    /// whether it was sent: not while the server isn't ready, or for a
    /// file it doesn't have.
    pub(crate) fn code_actions(
        &mut self,
        path: &std::path::Path,
        text: &Rope,
        selection: Range<usize>,
        kind: ActionKind,
        ticket: u64,
    ) -> bool {
        let Some(document) = self.documents.get(path) else { return false };
        let encoding = self.encoding;
        let (range, context) = match kind {
            ActionKind::QuickFix => {
                let start = position(text, selection.start, encoding);
                let end = position(text, selection.end, encoding);
                let mut diagnostics: Vec<&Diagnostic> =
                    document.diagnostics.iter().filter(|d| d.range.start <= end && start <= d.range.end).collect();
                if diagnostics.is_empty() {
                    let lines = start.line..=end.line;
                    diagnostics = document
                        .diagnostics
                        .iter()
                        .filter(|d| d.range.start.line <= *lines.end() && *lines.start() <= d.range.end.line)
                        .collect();
                }
                let context = json!({ "diagnostics": diagnostics, "only": ["quickfix"], "triggerKind": 1 });
                (json!({ "start": start, "end": end }), context)
            }
            ActionKind::OrganizeImports => {
                let end = position(text, text.len_chars(), encoding);
                let context = json!({ "diagnostics": [], "only": ["source.organizeImports"], "triggerKind": 1 });
                (json!({ "start": { "line": 0, "character": 0 }, "end": end }), context)
            }
        };
        let params = json!({ "textDocument": { "uri": document.uri }, "range": range, "context": context });
        self.request("textDocument/codeAction", params, Pending::CodeActions(ticket)).is_some()
    }

    pub(super) fn code_actions_answered(&self, ticket: u64, result: Result<Value, ResponseError>) -> Vec<Output> {
        let actions = match result {
            Ok(Value::Array(actions)) => actions,
            // Nothing (`null`), or an error: no actions from this server.
            _ => Vec::new(),
        };
        let fixes = actions.iter().filter_map(|action| self.fix(action)).collect();
        vec![Output::CodeActions { ticket, fixes }]
    }

    /// A code action that carries its edit, and isn't disabled.
    fn fix(&self, action: &Value) -> Option<CodeActionFix> {
        let title = action["title"].as_str()?.to_owned();
        if !action["disabled"].is_null() {
            return None;
        }
        let edit = &action["edit"];
        let mut edits: Vec<(PathBuf, Vec<TextEdit>)> = Vec::new();
        let mut add = |uri: &str, list: &Value| {
            let Some(path) = text::path(uri) else { return };
            let list: Vec<TextEdit> = list.as_array().into_iter().flatten().filter_map(text_edit).collect();
            match edits.iter_mut().find(|(p, _)| *p == path) {
                Some((_, existing)) => existing.extend(list),
                None => edits.push((path, list)),
            }
        };
        if let Some(changes) = edit["changes"].as_object() {
            for (uri, list) in changes {
                add(uri, list);
            }
        }
        for change in edit["documentChanges"].as_array().into_iter().flatten() {
            // Text edits only: creating, renaming or deleting files isn't
            // something a quick fix of tsgo's does.
            if let Some(uri) = change["textDocument"]["uri"].as_str() {
                add(uri, &change["edits"]);
            }
        }
        if edits.iter().all(|(_, list)| list.is_empty()) {
            return None;
        }
        let kind = action["kind"].as_str().unwrap_or_default().to_owned();
        Some(CodeActionFix { title, kind, encoding: self.encoding, edits })
    }
}

/// Remembers the diagnostics a server reported for an open file, which
/// quick fixes are asked with.
impl LanguageServer {
    pub(super) fn remember_diagnostics(&mut self, outputs: &[Output]) {
        for output in outputs {
            if let Output::Diagnostics { path, diagnostics } = output
                && let Some(document) = self.documents.get_mut(path)
            {
                document.diagnostics = diagnostics.clone();
            }
        }
    }
}

fn text_edit(edit: &Value) -> Option<TextEdit> {
    let point = |p: &Value| Some((p["line"].as_u64()? as u32, p["character"].as_u64()? as u32));
    Some(TextEdit {
        start: point(&edit["range"]["start"])?,
        end: point(&edit["range"]["end"])?,
        text: edit["newText"].as_str()?.to_owned(),
    })
}

/// The LSP position of a char index into `text`, in `encoding`.
pub(crate) fn position(text: &Rope, char: usize, encoding: Encoding) -> Position {
    let char = char.min(text.len_chars());
    let line = text.char_to_line(char);
    let start = text.line_to_char(line);
    let character = text
        .slice(start..char)
        .chars()
        .map(|c| match encoding {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        })
        .sum::<usize>();
    Position { line: line as u32, character: character as u32 }
}

/// The char range in `text` of an edit from a server using `encoding`.
pub(crate) fn char_range(text: &Rope, edit: &TextEdit, encoding: Encoding) -> Range<usize> {
    let index = |(line, character): (u32, u32)| {
        // A position past the last line is the end of the text.
        if line as usize >= text.len_lines() {
            return text.len_chars();
        }
        let at = text::text_position(text, line, character, encoding);
        text.line_to_char(at.line) + at.column
    };
    let (start, end) = (index(edit.start), index(edit.end));
    start..end.max(start)
}
