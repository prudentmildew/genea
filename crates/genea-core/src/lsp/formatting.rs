//! Format and fix on save (ticket #50), the protocol side: asking Oxfmt to
//! format a document (`textDocument/formatting`), and asking Oxlint for
//! its safe fixes (the `source.fixAll.oxc` code action). tsgo's formatter
//! is never asked.

use std::path::Path;

use ropey::Rope;
use serde_json::{Value, json};

use super::{
    LanguageServer, Output, Pending,
    actions,
    connection::ResponseError,
};

/// The code action kind of Oxlint's safe fixes for a whole file.
pub(crate) const FIX_ALL: &str = "source.fixAll.oxc";

impl LanguageServer {
    /// Asks the server to format an open document (relative path), indented
    /// as given (LSP requires it; Oxfmt reads its own config too). The
    /// answer comes back as [`Output::Formatted`] with `ticket`. Returns
    /// the request's id, or `None` if the server isn't ready or doesn't
    /// have the file.
    pub(crate) fn format(&mut self, path: &Path, tab_size: usize, insert_spaces: bool, ticket: u64) -> Option<i64> {
        let uri = self.documents.get(path)?.uri.clone();
        let options = json!({ "tabSize": tab_size, "insertSpaces": insert_spaces });
        let params = json!({ "textDocument": { "uri": uri }, "options": options });
        self.request("textDocument/formatting", params, Pending::Formatting(ticket))
    }

    /// Asks Oxlint for the safe fixes of a whole open document (relative
    /// path), whose text is `text` (as last synced). The answer comes back as [`Output::CodeActions`] with
    /// `ticket`. Returns the request's id, or `None` if the server isn't
    /// ready or doesn't have the file.
    pub(crate) fn fix_all(&mut self, path: &Path, text: &Rope, ticket: u64) -> Option<i64> {
        let document = self.documents.get(path)?;
        let end = actions::position(text, text.len_chars(), self.encoding);
        let context = json!({ "diagnostics": document.diagnostics, "only": [FIX_ALL], "triggerKind": 1 });
        let range = json!({ "start": { "line": 0, "character": 0 }, "end": end });
        let params = json!({ "textDocument": { "uri": document.uri }, "range": range, "context": context });
        self.request("textDocument/codeAction", params, Pending::CodeActions(ticket))
    }
}

/// The answer to `textDocument/formatting`: its edits, or none for `null`
/// or an error.
pub(super) fn answered(ticket: u64, result: Result<Value, ResponseError>) -> Vec<Output> {
    let edits = match result {
        Ok(Value::Array(edits)) => edits.iter().filter_map(actions::text_edit).collect(),
        _ => Vec::new(),
    };
    vec![Output::Formatted { ticket, edits }]
}
