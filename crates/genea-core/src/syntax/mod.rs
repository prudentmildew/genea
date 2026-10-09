//! Syntax: a file's tree-sitter tree and highlight spans (ticket #24).
//!
//! The rule (ADR 0004, spec #19 Threading): the reparse never sits in the
//! keystroke-to-frame path. An edit is applied to the main thread's copy of
//! the tree ([`tree_sitter::Tree::edit`], which only moves positions) and to
//! the highlight spans, which shift with the text they cover. Parsing and
//! highlighting then run in the background ([`ParseJob`]), one parse at a
//! time. Edits made while a parse runs are replayed onto its result when it
//! lands, and another parse starts, so highlighting catches up after typing
//! stops.
//!
//! Later tickets build on this:
//!
//! - structural editing (#25: folding, brackets, indent, expand selection)
//!   reads [`Syntax::tree`], which is always edited to the buffer's current
//!   positions, though its structure may be one parse behind;
//! - semantic highlighting (#46) layers the language server's tokens over
//!   [`Syntax::spans`] in the editor's view.

mod dotenv;
mod highlight;
mod language;
pub(crate) mod structure;

use std::{
    ops::Range,
    sync::atomic::{AtomicU64, Ordering},
};

pub use highlight::Highlight;
pub(crate) use highlight::{Span, Spans};
pub(crate) use language::{Comment, Language};
pub(crate) use structure::{FoldRegion, Indent};
use ropey::Rope;
use tree_sitter::{InputEdit, Parser, Tree};

/// One open buffer's syntax, on the main thread.
pub(crate) struct Syntax {
    /// Tells this buffer's parse results from another's.
    id: u64,
    language: Language,
    /// The last parse, edited to the buffer's current positions. `None`
    /// before the first parse lands, and always for `.env`.
    tree: Option<Tree>,
    /// The last parse's highlights, shifted by the edits since.
    spans: Spans,
    /// Counts the edits: `tree` and `spans` are edited up to this one. Not
    /// the buffer's version, which undo winds back.
    version: u64,
    /// The `version` the last parse was of.
    parsed_version: Option<u64>,
    /// The version the parse in flight is of, if one is.
    in_flight: Option<u64>,
    /// Edits made since the parse in flight started, to replay onto it.
    edits_since_parse: Vec<InputEdit>,
}

/// A parse to run in the background.
pub(crate) struct ParseJob {
    id: u64,
    language: Language,
    old_tree: Option<Tree>,
    text: Rope,
    version: u64,
}

/// A finished parse, to hand back to [`Syntax::parsed`].
pub(crate) struct Parsed {
    id: u64,
    tree: Option<Tree>,
    spans: Spans,
    version: u64,
}

impl Syntax {
    /// The syntax for a file, or `None` if it isn't in a highlighted
    /// language. Large files never get one (the editor decides:
    /// [`crate::editor::LARGE_FILE_BYTES`]).
    pub(crate) fn for_file(path: &std::path::Path) -> Option<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Some(Syntax {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            language: Language::of_path(path)?,
            tree: None,
            spans: Spans::default(),
            version: 0,
            parsed_version: None,
            in_flight: None,
            edits_since_parse: Vec::new(),
        })
    }

    /// Records an edit to the buffer: moves the tree and the spans with the
    /// text.
    pub(crate) fn edit(&mut self, edit: InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(&edit);
        }
        self.spans.edit(&edit);
        self.version += 1;
        if self.in_flight.is_some() {
            self.edits_since_parse.push(edit);
        }
    }

    /// A parse of `text` to run in the background, if the tree is behind
    /// the buffer and no parse is running already.
    pub(crate) fn start_parse(&mut self, text: &Rope) -> Option<ParseJob> {
        if self.in_flight.is_some() || self.parsed_version == Some(self.version) {
            return None;
        }
        self.in_flight = Some(self.version);
        Some(ParseJob {
            id: self.id,
            language: self.language,
            old_tree: self.tree.clone(),
            text: text.clone(),
            version: self.version,
        })
    }

    /// Takes a finished parse, catching it up with the edits made while it
    /// ran. Results for another buffer are ignored.
    pub(crate) fn parsed(&mut self, parsed: Parsed) {
        if parsed.id != self.id || self.in_flight != Some(parsed.version) {
            return;
        }
        let (mut tree, mut spans) = (parsed.tree, parsed.spans);
        for edit in self.edits_since_parse.drain(..) {
            if let Some(tree) = &mut tree {
                tree.edit(&edit);
            }
            spans.edit(&edit);
        }
        self.tree = tree;
        self.spans = spans;
        self.parsed_version = Some(parsed.version);
        self.in_flight = None;
    }

    /// The highlight spans overlapping a byte range of the buffer.
    pub(crate) fn spans(&self, bytes: Range<usize>) -> &[Span] {
        self.spans.overlapping(bytes)
    }

    /// The syntax tree, edited to the buffer's current positions; its
    /// structure is the last parse's.
    pub(crate) fn tree(&self) -> Option<&Tree> {
        self.tree.as_ref()
    }

    /// How a line break at `byte` (on `row`) indents the new line. `before`
    /// is the line's text before the caret, `rest` the byte the text after
    /// it starts at (whitespace skipped) and `next` that text's first char.
    ///
    /// The tree decides, and since it may be a parse behind (a brace typed
    /// just before Return isn't in it yet), an opening bracket that ends
    /// `before` and isn't in a string or comment counts too.
    pub(crate) fn indent(&self, before: &str, byte: usize, row: usize, rest: usize, next: Option<char>) -> Indent {
        let indent = self.tree.as_ref().map(|tree| structure::indent(tree, byte, row, rest)).unwrap_or_default();
        if indent.deeper {
            return indent;
        }
        let trimmed = before.trim_end();
        let Some(last) = trimmed.chars().next_back() else { return indent };
        let last_byte = byte - (before.len() - trimmed.len()) - last.len_utf8();
        let literal = self.spans(last_byte..last_byte + 1).iter().any(|span| {
            matches!(
                span.highlight,
                Highlight::String | Highlight::StringSpecial | Highlight::Comment | Highlight::Escape | Highlight::Literal
            )
        });
        if literal {
            return indent;
        }
        match last {
            '{' | '[' | '(' => {
                let close = structure::closing_bracket(&last.to_string()).and_then(|c| c.chars().next());
                Indent { deeper: true, split: next.is_some() && next == close }
            }
            ':' if self.language == Language::Yaml => Indent { deeper: true, split: false },
            _ => indent,
        }
    }
}

impl ParseJob {
    /// Parses (incrementally, from the previous tree) and highlights. Runs
    /// on a background thread.
    pub(crate) fn run(self) -> Parsed {
        let mut bytes = Vec::with_capacity(self.text.len_bytes());
        for chunk in self.text.chunks() {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        let mut paint = vec![0; bytes.len()];
        let tree = match self.language.config() {
            Some(config) => {
                let mut parser = Parser::new();
                let tree = parser
                    .set_language(&config.language)
                    .ok()
                    .and_then(|()| parser.parse(&bytes, self.old_tree.as_ref()));
                if let Some(tree) = &tree {
                    highlight::paint_tree(config, tree.root_node(), &bytes, &mut paint, 0);
                }
                tree
            }
            None => {
                dotenv::paint(&bytes, &mut paint);
                None
            }
        };
        Parsed { id: self.id, tree, spans: Spans::from_paint(&paint), version: self.version }
    }
}
