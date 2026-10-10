//! What the language server draws on an open file (ticket #46): semantic
//! highlights over the tree-sitter ones.
//!
//! They arrive for a version of the text, and are kept in byte positions
//! that move with every edit (`Editor::splice`), so they stay on the text
//! they belong to until fresh ones arrive.

use std::ops::Range;

use tree_sitter::InputEdit;

use super::Editor;
use crate::syntax::{Highlight, Span, Spans};

/// An editor's decorations.
#[derive(Clone, Debug, Default)]
pub(crate) struct Decorations {
    /// Semantic highlights; they win over the tree-sitter ones.
    semantic: Spans,
}

impl Decorations {
    /// Moves everything with an edit.
    pub(super) fn edit(&mut self, edit: &InputEdit) {
        self.semantic.edit(edit);
    }

    /// The semantic highlights overlapping a byte range.
    pub(super) fn semantic(&self, bytes: Range<usize>) -> &[Span] {
        self.semantic.overlapping(bytes)
    }
}

impl Editor {
    /// Replaces the semantic highlights (byte ranges, in order, not
    /// overlapping), if the text is still at `version`; otherwise they are
    /// for older text and are dropped.
    pub(crate) fn set_semantic_highlights(&mut self, version: u64, highlights: Vec<(Range<usize>, Highlight)>) {
        if version != self.version || self.syntax.is_none() {
            return;
        }
        let spans = highlights
            .into_iter()
            .map(|(bytes, highlight)| Span { start: bytes.start as u32, end: bytes.end as u32, highlight });
        self.decorations.semantic = Spans::from_spans(spans);
    }

    /// Drops everything the language server drew.
    pub(crate) fn clear_decorations(&mut self) {
        self.decorations = Decorations::default();
    }
}
