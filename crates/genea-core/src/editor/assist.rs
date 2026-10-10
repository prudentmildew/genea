//! What completion, hover and signature help (ticket #43) need from an
//! editor: the primary caret as a char index, grid cells and chars, and
//! applying a server's text edits as one undo step.

use std::{ops::Range, time::Instant};

use super::{CaretSelection, Editor};
use crate::{
    history::{Change, EditKind},
    view::Caret,
};

impl Editor {
    /// The primary caret, as a char index into the text.
    pub(crate) fn caret_index(&self) -> usize {
        self.primary().caret
    }

    /// How many carets there are.
    pub(crate) fn caret_count(&self) -> usize {
        self.carets.len()
    }

    /// The grid cell of a char index.
    pub(crate) fn cell_of(&self, position: usize) -> Caret {
        self.caret_at(position.min(self.text.len_chars()))
    }

    /// The char shown at a grid cell (0-based line and display column), or
    /// `None` past the end of its line or below the last line.
    pub(crate) fn char_at_cell(&self, line: usize, column: usize) -> Option<usize> {
        if line >= self.text.len_lines() {
            return None;
        }
        let position = self.position_at(line, column);
        let line_end = self.text.line_to_char(line) + self.line_len(line);
        (position < line_end && self.grid_column(line, line_end - self.text.line_to_char(line)) > column)
            .then_some(position)
    }

    /// Replaces char ranges of the text (not overlapping, any order) as one
    /// undo step, leaving one caret after the text that replaced
    /// `edits[main]`. A completion and its auto-import go in together this
    /// way.
    pub(crate) fn apply_completion_edits(
        &mut self,
        mut edits: Vec<(Range<usize>, String)>,
        main: usize,
        now: Instant,
        viewport_rows: f64,
    ) {
        self.preedit.clear();
        if self.read_only || main >= edits.len() {
            return;
        }
        let (before, version_before) = (self.selections(), self.version);
        let len = self.text.len_chars();
        for (range, _) in &mut edits {
            range.end = range.end.min(len);
            range.start = range.start.min(range.end);
        }
        // Where the main edit's text ends once the edits before it in the
        // text have shifted it.
        let main_range = edits[main].0.clone();
        let shift: isize = edits
            .iter()
            .filter(|(range, _)| range.end <= main_range.start && *range != main_range)
            .map(|(range, text)| text.chars().count() as isize - range.len() as isize)
            .sum();
        let caret = (main_range.start as isize + shift) as usize + edits[main].1.chars().count();
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let mut changes = Vec::new();
        for (range, text) in edits.iter().rev() {
            if !range.is_empty() {
                changes.push(Change::Remove { at: range.start, text: self.text.slice(range.clone()).to_string() });
            }
            if !text.is_empty() {
                changes.push(Change::Insert { at: range.start, text: text.clone() });
            }
        }
        for change in &changes {
            self.apply(change);
        }
        if !changes.is_empty() {
            self.new_version();
        }
        self.carets = vec![CaretSelection::at(caret)];
        self.reveal_caret(viewport_rows);
        self.record(EditKind::Other, now, changes, before, version_before);
    }
}
