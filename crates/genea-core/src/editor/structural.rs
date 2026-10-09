//! Structural editing (ticket #25): auto-indent on Return, matching
//! brackets, expand and shrink selection, line comments and folding. The
//! syntax tree (`syntax::structure`) answers the questions; this turns its
//! answers into edits, carets and view state.

use std::{
    collections::HashMap,
    ops::Range,
    time::Instant,
};

use super::{CaretSelection, Editor};
use crate::{
    command::CaretMove,
    history::{Change, EditKind},
    indentation::Indentation,
    syntax::{Comment, FoldRegion, Language, Syntax, structure},
    view::{Caret, Fold},
};

/// What ⌥↓ goes back through: the selections before each ⌥↑, valid while
/// the text version and the carets are those the last ⌥↑ left.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Expansions {
    version: u64,
    carets: Vec<CaretSelection>,
    before: Vec<Vec<CaretSelection>>,
}

impl Editor {
    /// Return: breaks the line at every caret, indenting the new line like
    /// the caret's line, one level deeper when the caret is just inside a
    /// block (after `{`, `(`, `[` or an element's start tag). When the text
    /// after the caret closes that block, it goes on a line of its own and
    /// the caret stays on the indented line between. Whitespace after the
    /// caret is dropped. A level is one of `indentation` (ticket #26).
    pub(crate) fn new_line(&mut self, indentation: Indentation, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only {
            return;
        }
        let break_text = self.line_ending.normalize("\n");
        let unit = indentation.unit();
        let edits = (0..self.carets.len())
            .map(|i| {
                let range = self.carets[i].range();
                let line = self.text.char_to_line(range.start);
                let line_start = self.text.line_to_char(line);
                let before: String = self.text.slice(line_start..range.start).chars().collect();
                let end_line = self.text.char_to_line(range.end);
                let end_line_end = self.text.line_to_char(end_line) + self.line_len(end_line);
                let after: String = self.text.slice(range.end..end_line_end).chars().collect();
                let skipped = after.chars().take_while(|c| matches!(c, ' ' | '\t')).count();
                let base: String = before.chars().take_while(|c| matches!(c, ' ' | '\t')).collect();

                let byte = self.text.char_to_byte(range.start);
                let rest = self.text.char_to_byte(range.end + skipped);
                let next = after.chars().nth(skipped);
                let indent = self
                    .syntax
                    .as_ref()
                    .map(|syntax| syntax.indent(&before, byte, line, rest, next))
                    .unwrap_or_default();

                let mut text = format!("{break_text}{base}");
                if indent.deeper {
                    text.push_str(&unit);
                }
                let caret = text.chars().count();
                if indent.deeper && indent.split {
                    text.push_str(&break_text);
                    text.push_str(&base);
                }
                (i, range.start..range.end + skipped, text, caret)
            })
            .collect();
        self.replace_placing(edits, EditKind::Typing, now, viewport_rows);
    }

    /// ⌘/: comments the carets' lines out, or back in if every one that
    /// isn't blank is commented already. Line comments go at the shallowest
    /// indentation of the lines; block comments wrap each line's text.
    pub(crate) fn toggle_line_comment(&mut self, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        let Some(comment) = Language::of_path(&self.path).and_then(Language::comment) else { return };
        if self.read_only {
            return;
        }
        let (open, close) = match comment {
            Comment::Line(prefix) => (prefix, None),
            Comment::Block(open, close) => (open, Some(close)),
        };

        let lines: Vec<(usize, String)> = self
            .caret_lines()
            .into_iter().map(|line| (line, self.line_chars(line).into_iter().collect())).collect();
        let filled: Vec<&(usize, String)> = lines.iter().filter(|(_, text)| !text.trim().is_empty()).collect();
        let commented = |text: &str| {
            let text = text.trim();
            text.starts_with(open) && close.is_none_or(|close| text.len() >= open.len() + close.len() && text.ends_with(close))
        };
        let indent_of = |text: &str| text.chars().take_while(|c| matches!(c, ' ' | '\t')).count();

        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        if !filled.is_empty() && filled.iter().all(|(_, text)| commented(text)) {
            for (line, text) in filled {
                let start = self.text.line_to_char(*line);
                let indent = indent_of(text);
                let chars: Vec<char> = text.chars().collect();
                let mut open_end = indent + open.chars().count();
                if chars.get(open_end) == Some(&' ') {
                    open_end += 1;
                }
                edits.push((start + indent..start + open_end, String::new()));
                if let Some(close) = close {
                    let end = text.trim_end().chars().count();
                    let mut from = end - close.chars().count();
                    if from > open_end && chars[from - 1] == ' ' {
                        from -= 1;
                    }
                    edits.push((start + from.max(open_end)..start + end, String::new()));
                }
            }
        } else {
            let targets: Vec<&(usize, String)> = if filled.is_empty() { lines.iter().collect() } else { filled };
            let indent = targets.iter().map(|(_, text)| indent_of(text)).min().unwrap_or(0);
            for (line, text) in targets {
                let start = self.text.line_to_char(*line);
                let at = if text.trim().is_empty() { text.chars().count() } else { indent };
                edits.push((start + at..start + at, format!("{open} ")));
                if let Some(close) = close {
                    let end = start + text.trim_end().chars().count().max(at);
                    edits.push((end..end, format!(" {close}")));
                }
            }
        }
        self.edit_text(edits, EditKind::Other, now, viewport_rows);
    }

    /// The bracket at the primary caret (after it, else before it) and its
    /// match, top to bottom, for the view to highlight.
    pub(super) fn matched_brackets(&self) -> Vec<Caret> {
        let Some(tree) = self.syntax.as_ref().and_then(Syntax::tree) else { return Vec::new() };
        let caret = self.primary().caret;
        let byte = self.text.char_to_byte(caret);
        let after = structure::matching_bracket(tree, byte);
        let before = || (caret > 0).then(|| structure::matching_bracket(tree, self.text.char_to_byte(caret - 1))).flatten();
        let Some((bracket, matched)) = after.or_else(before) else { return Vec::new() };
        let mut cells: Vec<Caret> = [bracket.start, matched.start]
            .into_iter()
            .filter(|&b| b <= self.text.len_bytes())
            .map(|b| self.caret_at(self.text.byte_to_char(b)))
            .collect();
        cells.sort_by_key(|c| (c.line, c.column));
        cells
    }

    /// ⌥↑: grows every selection to the smallest syntax node (or a block's
    /// inside) around it. An empty selection grows to the node at its
    /// caret. Remembers the selections before, for `shrink_selection`.
    pub(crate) fn expand_selection(&mut self, viewport_rows: f64) {
        let Some(tree) = self.syntax.as_ref().and_then(Syntax::tree) else { return };
        let before = self.carets.clone();
        let mut expanded = false;
        for i in 0..self.carets.len() {
            let range = self.carets[i].range();
            let bytes = self.text.char_to_byte(range.start)..self.text.char_to_byte(range.end);
            if let Some(grown) = structure::expansion(tree, bytes) {
                let start = self.text.byte_to_char(grown.start.min(self.text.len_bytes()));
                let end = self.text.byte_to_char(grown.end.min(self.text.len_bytes()));
                self.carets[i] = CaretSelection::selecting(start, end);
                expanded = true;
            }
        }
        if !expanded {
            return;
        }
        self.merge_carets();
        let mut history = match self.expansions.take() {
            Some(expansions) if self.expansions_hold(&expansions, &before) => expansions.before,
            _ => Vec::new(),
        };
        history.push(before);
        self.expansions = Some(Expansions { version: self.version, carets: self.carets.clone(), before: history });
        self.reveal_caret(viewport_rows);
    }

    /// ⌥↓: puts back the selections from before the last ⌥↑, if nothing
    /// else changed the selections or the text since.
    pub(crate) fn shrink_selection(&mut self, viewport_rows: f64) {
        let Some(mut expansions) = self.expansions.take() else { return };
        if !self.expansions_hold(&expansions, &self.carets) {
            return;
        }
        let Some(previous) = expansions.before.pop() else { return };
        self.carets = previous;
        if !expansions.before.is_empty() {
            expansions.carets = self.carets.clone();
            self.expansions = Some(expansions);
        }
        self.reveal_caret(viewport_rows);
    }

    /// Whether the selections are still the ones the last expansion left.
    fn expansions_hold(&self, expansions: &Expansions, carets: &[CaretSelection]) -> bool {
        expansions.version == self.version && expansions.carets == carets
    }

    /// Makes several changes as one edit, keeping every caret and selection
    /// on the text it was on: each entry replaces a char range (sorted, not
    /// overlapping) of the text as it is now. A caret where text is
    /// inserted moves after it.
    pub(super) fn edit_text(&mut self, mut edits: Vec<(Range<usize>, String)>, kind: EditKind, now: Instant, rows: f64) {
        edits.retain(|(range, text)| !(range.is_empty() && text.is_empty()));
        if self.read_only || edits.is_empty() {
            return;
        }
        edits.sort_by_key(|(range, _)| (range.start, range.end));
        let (before, version_before) = (self.selections(), self.version);
        let map = |position: usize| {
            let mut shift = 0isize;
            for (range, text) in &edits {
                let inserted = text.chars().count();
                if range.end <= position {
                    shift += inserted as isize - range.len() as isize;
                } else if range.start < position {
                    return range.start.saturating_add_signed(shift) + inserted;
                } else {
                    break;
                }
            }
            position.saturating_add_signed(shift)
        };
        let carets: Vec<CaretSelection> = self
            .carets
            .iter()
            .map(|c| CaretSelection { caret: map(c.caret), anchor: map(c.anchor), goal: None })
            .collect();

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
        self.new_version();
        self.carets = carets;
        self.merge_carets();
        self.reveal_caret(rows);
        self.record(kind, now, changes, before, version_before);
    }
}

/// A collapsed fold: the lines after the one `start` is on, through the
/// one `last` is on, are hidden. Both are char positions that move with
/// edits like any other position.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Folded {
    /// The region's start (its opening bracket or tag), on its first line.
    start: usize,
    /// The start of its last hidden line.
    last: usize,
}

/// Hidden line ranges, sorted and merged.
type Hidden = Vec<Range<usize>>;

/// A line's row: its index less the hidden lines above it. A hidden line
/// has the row of the line its fold starts on.
fn row_in(hidden: &Hidden, line: usize) -> usize {
    let mut row = line;
    for range in hidden {
        if range.end <= line {
            row -= range.len();
        } else if range.start <= line {
            row -= line - range.start + 1;
        } else {
            break;
        }
    }
    row
}

/// The line shown on a row.
fn line_in(hidden: &Hidden, row: usize) -> usize {
    let mut line = row;
    for range in hidden {
        if range.start <= line {
            line += range.len();
        } else {
            break;
        }
    }
    line
}

impl Editor {
    /// The lines hidden in collapsed folds.
    fn hidden(&self) -> Hidden {
        let mut ranges: Vec<Range<usize>> = self
            .folds
            .iter()
            .map(|fold| self.text.char_to_line(fold.start) + 1..self.text.char_to_line(fold.last) + 1)
            .filter(|range| !range.is_empty())
            .collect();
        ranges.sort_by_key(|range| range.start);
        let mut merged: Hidden = Vec::with_capacity(ranges.len());
        for range in ranges {
            match merged.last_mut() {
                Some(last) if range.start <= last.end => last.end = last.end.max(range.end),
                _ => merged.push(range),
            }
        }
        merged
    }

    /// The row a line is drawn on.
    pub(super) fn row_of(&self, line: usize) -> usize {
        if self.folds.is_empty() { line } else { row_in(&self.hidden(), line) }
    }

    /// The line drawn on a row, clamped to the last one.
    pub(super) fn line_at_row(&self, row: usize) -> usize {
        let line = if self.folds.is_empty() { row } else { line_in(&self.hidden(), row) };
        line.min(self.text.len_lines() - 1)
    }

    /// Each row of `rows` with the line drawn on it.
    pub(super) fn rows_to_lines(&self, rows: Range<usize>) -> Vec<(usize, usize)> {
        let hidden = self.hidden();
        let last = self.text.len_lines() - 1;
        rows.map(|row| (row, line_in(&hidden, row).min(last))).collect()
    }

    /// Rows in the file: lines not hidden in folds.
    pub(super) fn row_count(&self) -> usize {
        self.text.len_lines() - self.hidden().iter().map(|range| range.len()).sum::<usize>()
    }

    /// Moves the folds with an edit that replaced `chars` with `inserted`
    /// chars, dropping folds left with no lines to hide.
    pub(super) fn shift_folds(&mut self, chars: Range<usize>, inserted: usize) {
        if self.folds.is_empty() {
            return;
        }
        let shift = |position: usize| {
            if position < chars.start {
                position
            } else if position >= chars.end {
                position - chars.len() + inserted
            } else {
                chars.start
            }
        };
        for fold in &mut self.folds {
            fold.start = shift(fold.start);
            fold.last = shift(fold.last);
        }
        let text = &self.text;
        self.folds.retain(|fold| text.char_to_line(fold.last) > text.char_to_line(fold.start));
    }

    /// Expands the folds that hide a caret.
    pub(super) fn unfold_carets(&mut self) {
        if self.folds.is_empty() {
            return;
        }
        let lines: Vec<usize> = self.carets.iter().map(|c| self.text.char_to_line(c.caret)).collect();
        let text = &self.text;
        self.folds.retain(|fold| {
            let hidden = text.char_to_line(fold.start) + 1..=text.char_to_line(fold.last);
            !lines.iter().any(|line| hidden.contains(line))
        });
    }

    /// Moves a caret that Left or Right took into folded lines past them:
    /// back to the end of the fold's first line, or on to the line after.
    pub(super) fn skip_folded(&self, cursor: &mut CaretSelection, movement: CaretMove) {
        if self.folds.is_empty() {
            return;
        }
        let line = self.text.char_to_line(cursor.caret);
        let hidden = self.hidden();
        let Some(range) = hidden.iter().find(|range| range.contains(&line)) else { return };
        let header = range.start - 1;
        let header_end = self.text.line_to_char(header) + self.line_len(header);
        cursor.caret = match movement {
            CaretMove::Right | CaretMove::WordRight if range.end < self.text.len_lines() => {
                self.text.line_to_char(range.end)
            }
            CaretMove::Left | CaretMove::WordLeft | CaretMove::Right | CaretMove::WordRight => header_end,
            _ => return,
        };
    }

    /// Moves carets out of folded lines, to the start of the outermost fold
    /// that hides them.
    fn evict_carets(&mut self) {
        let hidden = self.hidden();
        for i in 0..self.carets.len() {
            let line = self.text.char_to_line(self.carets[i].caret);
            let Some(range) = hidden.iter().find(|range| range.contains(&line)) else { continue };
            let header = range.start - 1;
            let start = self.folds.iter().filter(|f| self.text.char_to_line(f.start) == header).map(|f| f.start).min();
            if let Some(start) = start {
                self.carets[i] = CaretSelection::at(start);
            }
        }
        self.merge_carets();
    }

    /// Whether byte `byte` starts its line's text (only whitespace before
    /// it), for a closing bracket that keeps its line visible.
    fn starts_line(&self, byte: usize) -> bool {
        let byte = byte.min(self.text.len_bytes());
        let line = self.text.byte_to_line(byte);
        self.text.byte_slice(self.text.line_to_byte(line)..byte).chars().all(|c| matches!(c, ' ' | '\t'))
    }

    /// The fold regions starting on `lines`, at most one per line.
    fn fold_regions(&self, lines: Range<usize>) -> Vec<FoldRegion> {
        let Some(tree) = self.syntax.as_ref().and_then(Syntax::tree) else { return Vec::new() };
        structure::fold_regions_in(tree, lines, &|byte| self.starts_line(byte))
    }

    /// The fold markers of `lines`: where a region starts, and whether it is
    /// collapsed.
    pub(super) fn fold_markers(&self, lines: Range<usize>) -> HashMap<usize, Fold> {
        let mut markers: HashMap<usize, Fold> =
            self.fold_regions(lines.clone()).into_iter().map(|region| (region.header, Fold::Expanded)).collect();
        for fold in &self.folds {
            let header = self.text.char_to_line(fold.start);
            if lines.contains(&header) {
                markers.insert(header, Fold::Collapsed);
            }
        }
        markers
    }

    /// Whether a region is collapsed already.
    fn is_collapsed(&self, region: &FoldRegion) -> bool {
        let start = self.text.byte_to_char(region.start.min(self.text.len_bytes()));
        self.folds.iter().any(|fold| fold.start == start)
    }

    /// Collapses a region, moving carets out of the lines it hides.
    fn collapse(&mut self, region: FoldRegion) {
        if region.last >= self.text.len_lines() || self.is_collapsed(&region) {
            return;
        }
        let start = self.text.byte_to_char(region.start.min(self.text.len_bytes()));
        self.folds.push(Folded { start, last: self.text.line_to_char(region.last) });
        self.evict_carets();
    }

    /// A gutter click on a fold marker.
    pub(crate) fn toggle_fold(&mut self, line: usize, viewport_rows: f64) {
        let before = self.folds.len();
        let text = &self.text;
        self.folds.retain(|fold| text.char_to_line(fold.start) != line);
        if self.folds.len() == before
            && let Some(region) = self.fold_regions(line..line + 1).into_iter().next()
        {
            self.collapse(region);
        }
        self.scroll_by(0.0, viewport_rows);
    }

    /// ⌥⌘−: collapses the region starting on the primary caret's line, or
    /// else the innermost expanded one around the caret.
    pub(crate) fn collapse_fold(&mut self, viewport_rows: f64) {
        let Some(tree) = self.syntax.as_ref().and_then(Syntax::tree) else { return };
        let caret = self.primary().caret;
        let line = self.text.char_to_line(caret);
        let line_start = |byte| self.starts_line(byte);
        let on_line = structure::fold_regions_in(tree, line..line + 1, &line_start);
        let around = structure::fold_regions_at(tree, self.text.char_to_byte(caret), &line_start);
        let region = on_line.into_iter().chain(around).find(|region| !self.is_collapsed(region));
        if let Some(region) = region {
            self.collapse(region);
            self.reveal_caret(viewport_rows);
        }
    }

    /// ⌥⌘+: expands the collapsed regions on the primary caret's line.
    pub(crate) fn expand_fold(&mut self, viewport_rows: f64) {
        let line = self.text.char_to_line(self.primary().caret);
        let text = &self.text;
        self.folds.retain(|fold| text.char_to_line(fold.start) != line);
        self.scroll_by(0.0, viewport_rows);
    }

    pub(crate) fn collapse_all_folds(&mut self, viewport_rows: f64) {
        for region in self.fold_regions(0..self.text.len_lines()) {
            self.collapse(region);
        }
        self.reveal_caret(viewport_rows);
    }

    pub(crate) fn expand_all_folds(&mut self, viewport_rows: f64) {
        self.folds.clear();
        self.scroll_by(0.0, viewport_rows);
    }
}
