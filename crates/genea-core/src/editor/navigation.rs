//! What code navigation and rename (ticket #44) need from an editor: where
//! the primary caret is, the word there, and a language server's edits
//! applied as one undo step.

use std::{ops::Range, time::Instant};

use super::{CaretSelection, Editor};
use crate::{
    history::{Change, EditKind},
    problems::TextPosition,
    text::CharClass,
};

impl Editor {
    /// Where the primary caret is (0-based line, char column).
    pub(crate) fn caret_position(&self) -> TextPosition {
        let caret = self.primary().caret;
        let line = self.text.char_to_line(caret);
        TextPosition { line, column: caret - self.text.line_to_char(line) }
    }

    /// The word the primary caret is in or touches, if any.
    pub(crate) fn word_at_caret(&self) -> Option<String> {
        let caret = self.primary().caret;
        let is_word = |i: usize| CharClass::of(self.text.char(i)) == CharClass::Word;
        let start = (0..caret).rev().take_while(|&i| is_word(i)).last().unwrap_or(caret);
        let end = (caret..self.text.len_chars()).take_while(|&i| is_word(i)).last().map_or(caret, |i| i + 1);
        (start < end).then(|| self.text.slice(start..end).to_string())
    }

    /// Replaces char ranges of the text as it is (a rename's edits) as one
    /// undo step. Ranges that overlap an earlier one are left out. Carets
    /// stay with the text around them; one inside a replaced range stays as
    /// far into the new text as it can. Returns whether anything changed.
    pub(crate) fn apply_edits(&mut self, mut edits: Vec<(Range<usize>, String)>, now: Instant, rows: f64) -> bool {
        if self.read_only {
            return false;
        }
        let len = self.text.len_chars();
        edits.retain(|(range, _)| range.start <= range.end && range.end <= len);
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut end = 0;
        edits.retain(|(range, _)| {
            let keep = range.start >= end;
            if keep {
                end = range.end;
            }
            keep
        });
        let (before, version_before) = (self.selections(), self.version);
        let mut changes = Vec::new();
        for (range, text) in edits.iter().rev() {
            if !range.is_empty() {
                changes.push(Change::Remove { at: range.start, text: self.text.slice(range.clone()).to_string() });
            }
            if !text.is_empty() {
                changes.push(Change::Insert { at: range.start, text: text.clone() });
            }
        }
        if changes.is_empty() {
            return false;
        }
        for change in &changes {
            self.apply(change);
        }
        self.new_version();
        let map = |p: usize| {
            let mut shift = 0isize;
            for (range, text) in &edits {
                let inserted = text.chars().count();
                if p >= range.end && !(range.is_empty() && p == range.start) {
                    shift += inserted as isize - range.len() as isize;
                } else if p > range.start {
                    return range.start.saturating_add_signed(shift) + (p - range.start).min(inserted);
                } else {
                    break;
                }
            }
            p.saturating_add_signed(shift)
        };
        for cursor in &mut self.carets {
            *cursor = CaretSelection::selecting(map(cursor.anchor), map(cursor.caret));
        }
        self.merge_carets();
        self.scroll_by(0.0, rows);
        self.record(EditKind::Other, now, changes, before, version_before);
        true
    }
}
