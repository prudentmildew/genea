//! Edits that come from elsewhere than the carets (ticket #45): a language
//! server's quick fix or organize imports, as one undo step.

use std::{ops::Range, time::Instant};

use super::{CaretSelection, Editor};
use crate::history::{Change, EditKind};

impl Editor {
    /// The primary caret's selection, as an ordered char range.
    pub(crate) fn primary_selection(&self) -> Range<usize> {
        self.carets.last().map_or(0..0, |c| c.range())
    }

    /// Replaces char ranges of the current text, as LSP text edits do: the
    /// ranges don't overlap (one that does is dropped), and edits at the
    /// same place apply in their order. Line breaks become the file's line
    /// ending. One undo step; the carets stay with the text around them
    /// (one inside a replaced range ends up after its replacement).
    pub(crate) fn apply_text_edits(&mut self, edits: Vec<(Range<usize>, String)>, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only || self.loading.is_some() {
            return;
        }
        let mut sorted: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
        let mut edits = edits;
        edits.sort_by_key(|(range, _)| range.start);
        for (range, text) in edits {
            let range = range.start.min(self.text.len_chars())..range.end.min(self.text.len_chars());
            if sorted.last().is_some_and(|(last, _)| range.start < last.end) {
                continue;
            }
            sorted.push((range, self.line_ending.normalize(&text)));
        }

        let (before, version_before) = (self.selections(), self.version);
        // From the end backwards, so each change's indices are those of the
        // text as it was before the edit.
        let mut changes = Vec::new();
        for (range, text) in sorted.iter().rev() {
            if !range.is_empty() {
                changes.push(Change::Remove { at: range.start, text: self.text.slice(range.clone()).to_string() });
            }
            if !text.is_empty() {
                changes.push(Change::Insert { at: range.start, text: text.clone() });
            }
        }
        if changes.is_empty() {
            return;
        }
        for change in &changes {
            self.apply(change);
        }
        self.new_version();

        let moved = |at: usize| {
            let mut shift = 0isize;
            for (range, text) in &sorted {
                let inserted = text.chars().count() as isize;
                if range.end <= at {
                    shift += inserted - range.len() as isize;
                } else if range.start < at {
                    return range.start.saturating_add_signed(shift + inserted);
                } else {
                    break;
                }
            }
            at.saturating_add_signed(shift)
        };
        self.carets = self.carets.iter().map(|c| CaretSelection::selecting(moved(c.anchor), moved(c.caret))).collect();
        self.merge_carets();
        self.reveal_caret(viewport_rows);
        self.record(EditKind::Other, now, changes, before, version_before);
    }
}
