//! What the language server draws on an open file (ticket #46): semantic
//! highlights over the tree-sitter ones, inlay hints, and code lenses.
//!
//! They arrive for a version of the text, and are kept in byte positions
//! that move with every edit (`Editor::splice`), so they stay on the text
//! they belong to until fresh ones arrive.
//!
//! Inlay hints are drawn in the line, so they move the text after them to
//! the right. View state is in *grid* columns, which count them; the
//! editor's own columns (display columns of the text: carets, Up and Down,
//! tab stops) don't. [`Editor::to_grid`] and [`Editor::from_grid`] convert
//! at the edge: what the view shows, and the cells it sends back (clicks).
//! A caret at a hint's position is drawn before the hint.
//!
//! Code lenses are drawn after the end of their line (not on a line of
//! their own above it, which would take rows that aren't the file's
//! lines), so they move nothing.

use std::ops::Range;

use tree_sitter::InputEdit;
use unicode_width::UnicodeWidthStr;

use super::{Editor, display_columns_from};
use crate::syntax::{Highlight, Span, Spans};

/// An editor's decorations.
#[derive(Clone, Debug, Default)]
pub(crate) struct Decorations {
    /// Semantic highlights; they win over the tree-sitter ones.
    semantic: Spans,
    /// Inlay hints, by position.
    inlays: Vec<Inlay>,
    /// Code lenses, by position.
    lenses: Vec<Lens>,
}

/// A code lens: its title, drawn at the end of the line `byte` is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Lens {
    pub(super) byte: usize,
    pub(super) title: String,
}

/// What goes between a line's text and its code lenses, and between
/// lenses.
pub(super) const LENS_GAP: &str = "  ";
const LENS_SEPARATOR: &str = " · ";

/// An inlay hint: text drawn before the char at `byte`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Inlay {
    pub(super) byte: usize,
    pub(super) text: String,
}

impl Decorations {
    /// Moves everything with an edit.
    pub(super) fn edit(&mut self, edit: &InputEdit) {
        self.semantic.edit(edit);
        let (start, old_end, new_end) = (edit.start_byte, edit.old_end_byte, edit.new_end_byte);
        // A hint where text is inserted moves with the text after it (a
        // name typed on keeps its type hint at its end; an argument keeps
        // its parameter hint); one in deleted text goes. Lenses too.
        let shift = |byte: &mut usize| {
            if *byte < start || (*byte == start && start != old_end) {
                true
            } else if *byte >= old_end {
                *byte = *byte - old_end + new_end;
                true
            } else {
                false
            }
        };
        self.inlays.retain_mut(|inlay| shift(&mut inlay.byte));
        self.lenses.retain_mut(|lens| shift(&mut lens.byte));
    }

    /// The code lenses at bytes `from..=to`, joined, as drawn after the
    /// line's text; `None` without any.
    pub(super) fn lenses(&self, from: usize, to: usize) -> Option<String> {
        let first = self.lenses.partition_point(|l| l.byte < from);
        let end = first + self.lenses[first..].partition_point(|l| l.byte <= to);
        let titles: Vec<&str> = self.lenses[first..end].iter().map(|l| l.title.as_str()).collect();
        (!titles.is_empty()).then(|| titles.join(LENS_SEPARATOR))
    }

    /// The semantic highlights overlapping a byte range.
    pub(super) fn semantic(&self, bytes: Range<usize>) -> &[Span] {
        self.semantic.overlapping(bytes)
    }

    /// The inlay hints at bytes `from..=to`, in order.
    pub(super) fn inlays(&self, from: usize, to: usize) -> &[Inlay] {
        let first = self.inlays.partition_point(|i| i.byte < from);
        let end = first + self.inlays[first..].partition_point(|i| i.byte <= to);
        &self.inlays[first..end]
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

    /// Replaces the inlay hints (byte position, text), if the text is still
    /// at `version`.
    pub(crate) fn set_inlay_hints(&mut self, version: u64, hints: Vec<(usize, String)>) {
        if version != self.version {
            return;
        }
        let mut inlays: Vec<Inlay> = hints.into_iter().map(|(byte, text)| Inlay { byte, text }).collect();
        inlays.sort_by_key(|i| i.byte);
        self.decorations.inlays = inlays;
    }

    /// Replaces the code lenses (byte position, title), if the text is
    /// still at `version`.
    pub(crate) fn set_code_lenses(&mut self, version: u64, lenses: Vec<(usize, String)>) {
        if version != self.version {
            return;
        }
        let mut lenses: Vec<Lens> = lenses.into_iter().map(|(byte, title)| Lens { byte, title }).collect();
        lenses.sort_by_key(|l| l.byte);
        self.decorations.lenses = lenses;
    }

    pub(crate) fn has_code_lenses(&self) -> bool {
        !self.decorations.lenses.is_empty()
    }

    pub(crate) fn clear_code_lenses(&mut self) {
        self.decorations.lenses.clear();
    }

    pub(crate) fn has_inlay_hints(&self) -> bool {
        !self.decorations.inlays.is_empty()
    }

    pub(crate) fn clear_inlay_hints(&mut self) {
        self.decorations.inlays.clear();
    }

    /// Drops everything the language server drew.
    pub(crate) fn clear_decorations(&mut self) {
        self.decorations = Decorations::default();
    }

    /// The primary caret as the status bar shows it: its line and the
    /// display column in the file's text (hints don't count), 1-based.
    pub(crate) fn caret_label(&self) -> String {
        let caret = self.primary().caret;
        format!("{}:{}", self.text.char_to_line(caret) + 1, self.display_column(caret) + 1)
    }

    /// The bytes of a line's text, without its line ending.
    pub(super) fn line_bytes(&self, line: usize) -> Range<usize> {
        let start = self.text.line_to_byte(line);
        let slice = self.text.line(line);
        let ending = slice.len_chars() - self.line_len(line);
        start..start + slice.len_bytes() - ending
    }

    /// The inlay hints on a line as (display column of the text where it
    /// goes, its width), in order.
    fn line_inlays(&self, line: usize) -> Vec<(usize, usize)> {
        if self.decorations.inlays.is_empty() {
            return Vec::new();
        }
        let bytes = self.line_bytes(line);
        let inlays = self.decorations.inlays(bytes.start, bytes.end);
        if inlays.is_empty() {
            return Vec::new();
        }
        let mut found = Vec::with_capacity(inlays.len());
        let mut inlays = inlays.iter().peekable();
        let (mut byte, mut column) = (bytes.start, 0);
        for c in self.text.byte_slice(bytes.clone()).chars() {
            while let Some(inlay) = inlays.next_if(|i| i.byte <= byte) {
                found.push((column, inlay.text.width()));
            }
            byte += c.len_utf8();
            column = display_columns_from(column, c);
        }
        found.extend(inlays.map(|inlay| (column, inlay.text.width())));
        found
    }

    /// A display column of a line's text as a grid column: moved right by
    /// the hints before it.
    pub(super) fn to_grid(&self, line: usize, column: usize) -> usize {
        column + self.line_inlays(line).iter().filter(|(at, _)| *at < column).map(|(_, width)| width).sum::<usize>()
    }

    /// A grid column (a cell the view sent) as a display column of the
    /// line's text; a cell in a hint is the hint's position.
    pub(super) fn from_grid(&self, line: usize, grid: usize) -> usize {
        let mut shift = 0;
        for (at, width) in self.line_inlays(line) {
            if grid < at + shift {
                break;
            }
            if grid < at + shift + width {
                return at;
            }
            shift += width;
        }
        grid - shift
    }
}
