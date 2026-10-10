//! Symbols from a language server (ticket #47): the answers to
//! `textDocument/documentSymbol` and `workspace/symbol` as one flat list,
//! and their positions as the editor counts them.

use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

use ropey::Rope;
use serde_json::Value;

use super::text::{self, Encoding};
use crate::problems::TextPosition;

/// A symbol the server found.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Symbol {
    pub(crate) name: String,
    /// The class, interface, namespace, … it is declared in.
    pub(crate) container: Option<String>,
    /// Its file: absolute as the server names it; the project makes it
    /// relative to its root.
    pub(crate) path: PathBuf,
    /// Where its name starts, as the server counts (its encoding).
    pub(crate) line: u32,
    pub(crate) character: u32,
}

/// The symbols in a `documentSymbol` or `workspace/symbol` answer, in the
/// server's order, members right after what contains them. Both answer
/// shapes are read: `DocumentSymbol` trees (of `document`, which they
/// don't name), and flat `SymbolInformation` or `WorkspaceSymbol` lists.
/// Anything garbled is left out.
pub(crate) fn parse(answer: &Value, document: Option<&Path>) -> Vec<Symbol> {
    let mut symbols = Vec::new();
    for item in answer.as_array().into_iter().flatten() {
        push(item, None, document, &mut symbols);
    }
    symbols
}

fn push(item: &Value, container: Option<&str>, document: Option<&Path>, symbols: &mut Vec<Symbol>) {
    let Some(name) = item["name"].as_str() else { return };
    if let Some(location) = item.get("location") {
        // `SymbolInformation`, or a `WorkspaceSymbol` (whose location may
        // be the file alone).
        let Some(path) = location["uri"].as_str().and_then(text::path) else { return };
        let start = &location["range"]["start"];
        let container = item["containerName"].as_str().filter(|c| !c.is_empty()).map(str::to_owned);
        symbols.push(symbol(name, container, path, start));
    } else if let Some(path) = document {
        // A `DocumentSymbol`, located by its name.
        let start = &item["selectionRange"]["start"];
        symbols.push(symbol(name, container.map(str::to_owned), path.to_owned(), start));
        for child in item["children"].as_array().into_iter().flatten() {
            push(child, Some(name), document, symbols);
        }
    }
}

fn symbol(name: &str, container: Option<String>, path: PathBuf, start: &Value) -> Symbol {
    let number = |key| start[key].as_u64().and_then(|n| u32::try_from(n).ok()).unwrap_or(0);
    Symbol { name: name.to_owned(), container, path, line: number("line"), character: number("character") }
}

/// Turns the server's positions into the editor's (line, char column),
/// using an open file's text, or else reading the file once. For a match
/// job, which converts only the results it shows.
pub(crate) struct Positions {
    root: PathBuf,
    encoding: Encoding,
    /// The open files' texts, by relative path.
    open: HashMap<PathBuf, Rope>,
    /// Files read from disk; `None` if they couldn't be.
    read: RefCell<HashMap<PathBuf, Option<Rope>>>,
}

impl Positions {
    pub(crate) fn new(root: PathBuf, encoding: Encoding, open: HashMap<PathBuf, Rope>) -> Self {
        Positions { root, encoding, open, read: RefCell::default() }
    }

    /// Where `symbol` (with a path relative to the root) is in its file.
    pub(crate) fn of(&self, symbol: &Symbol) -> TextPosition {
        let at = |text: &Rope| text::text_position(text, symbol.line, symbol.character, self.encoding);
        if let Some(text) = self.open.get(&symbol.path) {
            return at(text);
        }
        let mut read = self.read.borrow_mut();
        let text = read
            .entry(symbol.path.clone())
            .or_insert_with(|| fs::read_to_string(self.root.join(&symbol.path)).ok().map(|text| Rope::from_str(&text)));
        match text {
            Some(text) => at(text),
            // Gone: the server's columns are the best guess.
            None => TextPosition { line: symbol.line as usize, column: symbol.character as usize },
        }
    }
}
