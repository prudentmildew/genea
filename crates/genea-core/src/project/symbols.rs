//! The project's side of symbols in the finder (ticket #47): asking tsgo
//! for the current file's symbols (⌘F12, `textDocument/documentSymbol`) or
//! the project's (⌥⌘O and ⇧⇧, `workspace/symbol`), and taking its answers.
//! Matching them is in `crate::finder`, like files and actions.
//!
//! A request is *wanted* when the finder opens or its query changes, and
//! sent after the next sync with the server (`sync_language`), so the
//! server has the file's current text, and a server that isn't ready yet
//! is asked once it is. Only the answer to the latest request is taken.

use std::{collections::HashMap, sync::Arc};

use serde_json::json;

use super::Project;
use crate::{
    finder::{FinderSymbols, SymbolRequest},
    lsp::{
        Pending,
        symbols::{Positions, Symbol},
        text,
    },
    view::FinderMode,
};

impl Project {
    /// The finder opened, or its query changed: what to ask the server.
    pub(super) fn want_symbols(&mut self) {
        let current = self.editor.as_ref().map(|editor| editor.path().to_owned());
        let Some(finder) = &mut self.finder else { return };
        let symbols = &mut finder.symbols;
        match finder.mode {
            FinderMode::FileSymbols => {
                // The file's symbols don't depend on the query.
                if symbols.waiting.is_none() && symbols.found.is_empty() {
                    symbols.wanted = current.map(SymbolRequest::Document);
                }
            }
            FinderMode::ProjectSymbols | FinderMode::Everywhere => {
                let query = finder.query.trim();
                if query.is_empty() {
                    *symbols = FinderSymbols::default();
                } else {
                    symbols.wanted = Some(SymbolRequest::Workspace(query.to_owned()));
                }
            }
            FinderMode::Files | FinderMode::RecentFiles | FinderMode::Actions => {}
        }
    }

    /// Sends the finder's wanted symbols request, once the server is ready
    /// and has the file. Runs after every sync with the server.
    pub(super) fn request_symbols(&mut self) {
        let Some(wanted) = self.finder.as_mut().and_then(|finder| finder.symbols.wanted.take()) else { return };
        let (method, params, document) = match &wanted {
            SymbolRequest::Document(path) => {
                let Some(editor) = self.open_editor(path) else { return };
                // Files the server never gets have no symbols.
                if text::language_id(path).is_none() || editor.is_large() {
                    return;
                }
                if !editor.is_loaded() {
                    self.keep_wanted(wanted);
                    return;
                }
                let absolute = self.root.join(path);
                let params = json!({ "textDocument": { "uri": text::uri(&absolute) } });
                ("textDocument/documentSymbol", params, Some(absolute))
            }
            SymbolRequest::Workspace(query) => ("workspace/symbol", json!({ "query": query }), None),
        };
        match self.language_request(method, params, Pending::Symbols { document }) {
            Some(id) => {
                if let Some(finder) = &mut self.finder {
                    finder.symbols.waiting = Some(id);
                }
            }
            None => self.keep_wanted(wanted),
        }
    }

    /// Asks again after the next sync.
    fn keep_wanted(&mut self, wanted: SymbolRequest) {
        if let Some(finder) = &mut self.finder {
            finder.symbols.wanted.get_or_insert(wanted);
        }
    }

    /// The server answered symbols request `id`. The finder matches them at
    /// the end of this Apply (`refresh_finder`).
    pub(super) fn symbols_answered(&mut self, id: i64, symbols: Vec<Symbol>) {
        let Some(finder) = &mut self.finder else { return };
        if finder.symbols.waiting != Some(id) {
            return;
        }
        let document = finder.mode == FinderMode::FileSymbols;
        let (root, files) = (&self.root, &self.files);
        let found = symbols.into_iter().filter_map(|mut symbol| {
            symbol.path = symbol.path.strip_prefix(root).ok()?.to_owned();
            // The project's symbols leave out what the finder hides:
            // `node_modules`, `.git` and `exclude`.
            (document || files.lists(&symbol.path)).then_some(symbol)
        });
        finder.symbols.found = found.collect();
        finder.symbols.waiting = None;
        finder.symbols.changed = true;
    }

    /// The symbols the finder matches, and how to place them.
    pub(super) fn symbol_candidates(&self) -> (Arc<[Symbol]>, Positions) {
        let found = self.finder.as_ref().map(|finder| finder.symbols.found.clone()).unwrap_or_default();
        let mut open = HashMap::new();
        if !found.is_empty() {
            for editor in self.editor.iter().chain(self.panes.parked()) {
                open.insert(editor.path().to_owned(), editor.text().clone());
            }
        }
        (found, Positions::new(self.root.clone(), self.language_encoding(), open))
    }
}
