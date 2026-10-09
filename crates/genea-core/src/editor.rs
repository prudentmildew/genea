//! One open file on the editor surface: its text, carets, scroll position
//! and saved state.
//!
//! Every edit goes through `replace`, which applies it at every caret, gives
//! the buffer a fresh version (compared with the saved one for `modified`)
//! and records it in the undo history as one edit. Syntax builds on that
//! (ticket #24).
//!
//! The editor has one or more carets (ticket #52), each with its own
//! selection. They are kept in the order they were added; the last one is
//! the primary, which the view scrolls to and the status bar reports.

use std::{ops::Range, path::PathBuf, time::Instant};

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::{
    command::CaretMove,
    history::{Change, Edit, EditKind, History, Selection},
    text::{self, LineEnding},
    view::{Caret, EditorView, Preedit, VisibleLine},
};

/// Grid columns a tab advances to (the next multiple of this).
const TAB_WIDTH: usize = 4;

/// Columns of a line that make it into view state. Longer lines are cut, so
/// a minified file can't make every sync copy and shape megabytes.
pub const MAX_VISIBLE_COLUMNS: usize = 1000;

/// One caret and its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Cursor {
    /// Char index into the text: where the caret is, the moving end of the
    /// selection.
    caret: usize,
    /// Char index of the selection's fixed end; equal to `caret` when
    /// nothing is selected.
    anchor: usize,
    /// The display column Up and Down aim for, kept across short lines.
    /// Set by the first vertical move; cleared by every other move.
    goal: Option<usize>,
}

impl Cursor {
    fn at(position: usize) -> Self {
        Cursor { caret: position, anchor: position, goal: None }
    }

    fn selecting(anchor: usize, caret: usize) -> Self {
        Cursor { caret, anchor, goal: None }
    }

    /// The selection as an ordered char range.
    fn range(self) -> Range<usize> {
        self.anchor.min(self.caret)..self.anchor.max(self.caret)
    }

    fn is_empty(self) -> bool {
        self.anchor == self.caret
    }

    fn collapse(&mut self) {
        self.anchor = self.caret;
    }

    fn selection(self) -> Selection {
        Selection { anchor: self.anchor, caret: self.caret }
    }
}

pub(crate) struct Editor {
    path: PathBuf,
    text: Rope,
    /// Never empty. In the order the carets were added; the last is the
    /// primary. No two overlap (see `merge_cursors`).
    cursors: Vec<Cursor>,
    /// First visible row; fractional while scrolling smoothly.
    scroll_top: f64,
    /// Every line break typed or pasted is converted to this.
    line_ending: LineEnding,
    /// The IME's marked text, drawn at the primary caret; never in `text`.
    preedit: String,
    /// Identifies the text: every edit gives it a fresh one, and undo and
    /// redo restore the one the text had then.
    version: u64,
    /// The newest version handed out, so a fresh one is never reused.
    last_version: u64,
    /// The version last written to disk (or read from it).
    saved_version: u64,
    history: History,
}

/// The buffer as it was when a save started.
pub(crate) struct Snapshot {
    pub(crate) path: PathBuf,
    /// A cheap clone of the rope.
    pub(crate) text: Rope,
    version: u64,
}

impl Editor {
    pub(crate) fn new(path: PathBuf, text: Rope) -> Self {
        let line_ending = LineEnding::detect(&text);
        Editor {
            path,
            text,
            cursors: vec![Cursor::at(0)],
            scroll_top: 0.0,
            line_ending,
            preedit: String::new(),
            version: 0,
            last_version: 0,
            saved_version: 0,
            history: History::default(),
        }
    }

    fn primary(&self) -> Cursor {
        *self.cursors.last().expect("an editor always has a caret")
    }

    /// Keeps only the primary caret.
    fn set_cursor(&mut self, cursor: Cursor) {
        self.cursors = vec![cursor];
    }

    /// Moves every caret. With `extend`, each selection's anchor stays put,
    /// so the selection grows or shrinks; without, the selections collapse.
    pub(crate) fn move_caret(&mut self, movement: CaretMove, extend: bool, viewport_rows: f64) {
        let page = (viewport_rows.floor() as usize).max(1) as f64;
        match movement {
            CaretMove::PageUp => self.scroll_by(-page, viewport_rows),
            CaretMove::PageDown => self.scroll_by(page, viewport_rows),
            _ => {}
        }
        for i in 0..self.cursors.len() {
            let mut cursor = self.cursors[i];
            let range = cursor.range();
            match movement {
                CaretMove::Left if !extend && !range.is_empty() => (cursor.caret, cursor.goal) = (range.start, None),
                CaretMove::Right if !extend && !range.is_empty() => (cursor.caret, cursor.goal) = (range.end, None),
                _ => self.move_head(&mut cursor, movement, viewport_rows),
            }
            if !extend {
                cursor.collapse();
            }
            self.cursors[i] = cursor;
        }
        self.merge_cursors();
        self.reveal_caret(viewport_rows);
    }

    pub(crate) fn select_all(&mut self, viewport_rows: f64) {
        self.set_cursor(Cursor::selecting(0, self.text.len_chars()));
        self.reveal_caret(viewport_rows);
    }

    /// Moves a caret alone, leaving its anchor where it was.
    fn move_head(&self, cursor: &mut Cursor, movement: CaretMove, viewport_rows: f64) {
        let (line, column) = self.line_column(cursor.caret);
        let last_line = self.text.len_lines() - 1;
        let page = (viewport_rows.floor() as usize).max(1);
        let mut set = |line: usize, char_column: usize| {
            cursor.caret = self.text.line_to_char(line) + char_column;
            cursor.goal = None;
        };
        match movement {
            CaretMove::Left if column > 0 => set(line, column - 1),
            CaretMove::Left if line > 0 => set(line - 1, self.line_len(line - 1)),
            CaretMove::Left => {}
            CaretMove::Right if column < self.line_len(line) => set(line, column + 1),
            CaretMove::Right if line < last_line => set(line + 1, 0),
            CaretMove::Right => {}
            CaretMove::WordLeft if column > 0 => set(line, text::word_start(&self.line_chars(line), column)),
            CaretMove::WordRight if column < self.line_len(line) => {
                set(line, text::word_end(&self.line_chars(line), column))
            }
            CaretMove::WordLeft => self.move_head(cursor, CaretMove::Left, viewport_rows),
            CaretMove::WordRight => self.move_head(cursor, CaretMove::Right, viewport_rows),
            CaretMove::Up => self.move_vertically(cursor, line.saturating_sub(1)),
            CaretMove::Down => self.move_vertically(cursor, (line + 1).min(last_line)),
            CaretMove::PageUp => self.move_vertically(cursor, line.saturating_sub(page)),
            CaretMove::PageDown => self.move_vertically(cursor, (line + page).min(last_line)),
            CaretMove::LineStart => set(line, 0),
            CaretMove::LineEnd => set(line, self.line_len(line)),
            CaretMove::DocumentStart => set(0, 0),
            CaretMove::DocumentEnd => set(last_line, self.line_len(last_line)),
        }
    }

    /// Puts the caret at the last text position at or before a grid cell,
    /// dropping any other carets. With `extend`, the primary selection's
    /// anchor stays put.
    pub(crate) fn place_caret(&mut self, line: usize, column: usize, extend: bool, viewport_rows: f64) {
        let position = self.position_at(line, column);
        let mut cursor = self.primary();
        cursor.caret = position;
        cursor.goal = None;
        if !extend {
            cursor.collapse();
        }
        self.set_cursor(cursor);
        self.reveal_caret(viewport_rows);
    }

    /// ⌥-click: adds a caret at a grid cell as the new primary, or removes
    /// the caret that is already there (unless it is the only one).
    pub(crate) fn add_caret(&mut self, line: usize, column: usize, viewport_rows: f64) {
        let position = self.position_at(line, column);
        let existing = self.cursors.iter().position(|c| c.is_empty() && c.caret == position);
        match existing {
            Some(i) if self.cursors.len() > 1 => {
                self.cursors.remove(i);
            }
            Some(_) => {}
            None => self.cursors.push(Cursor::at(position)),
        }
        self.merge_cursors();
        self.reveal_caret(viewport_rows);
    }

    /// The text position at or before a grid cell, clamped to the text.
    fn position_at(&self, line: usize, column: usize) -> usize {
        let line = line.min(self.text.len_lines() - 1);
        self.text.line_to_char(line) + self.char_column_at(line, column)
    }

    /// Selects the run of one character class at a grid cell, with the
    /// caret at its end.
    pub(crate) fn select_word(&mut self, line: usize, column: usize, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        let chars = self.line_chars(line);
        let at = self.char_column_at(line, column).min(chars.len().saturating_sub(1));
        let (start, end) = match chars.get(at) {
            Some(&c) => {
                let class = text::CharClass::of(c);
                let same = |i: &usize| text::CharClass::of(chars[*i]) == class;
                let start = (0..at).rev().take_while(same).last().unwrap_or(at);
                let end = (at..chars.len()).take_while(same).last().map_or(at, |i| i + 1);
                (start, end)
            }
            None => (0, 0),
        };
        let line_start = self.text.line_to_char(line);
        self.set_cursor(Cursor::selecting(line_start + start, line_start + end));
        self.reveal_caret(viewport_rows);
    }

    /// Selects a whole line and its line break, with the caret at the start
    /// of the next line.
    pub(crate) fn select_line(&mut self, line: usize, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        let start = self.text.line_to_char(line);
        self.set_cursor(Cursor::selecting(start, start + self.text.line(line).len_chars()));
        self.reveal_caret(viewport_rows);
    }

    /// The buffer as it is now, for writing in the background.
    pub(crate) fn snapshot(&self) -> Snapshot {
        Snapshot { path: self.path.clone(), text: self.text.clone(), version: self.version }
    }

    /// Records that `snapshot` is on disk, if it is of this buffer.
    pub(crate) fn saved(&mut self, snapshot: &Snapshot) {
        if snapshot.path == self.path {
            self.saved_version = snapshot.version;
        }
    }

    /// Inserts `text` at every caret, replacing the selections, and puts
    /// each caret after it. Line breaks become the file's line ending. `now`
    /// is the host clock's time, for grouping undo steps.
    pub(crate) fn insert(&mut self, text: &str, kind: EditKind, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if text.is_empty() && self.cursors.iter().all(|c| c.is_empty()) {
            return;
        }
        let text = self.line_ending.normalize(text);
        let edits = (0..self.cursors.len()).map(|i| (i, self.cursors[i].range(), text.clone())).collect();
        self.replace(edits, kind, now, viewport_rows);
    }

    /// ⌘V: like `insert`, except that text with as many lines as there are
    /// carets (two or more) puts one line at each caret, top to bottom.
    pub(crate) fn paste(&mut self, text: &str, now: Instant, viewport_rows: f64) {
        let text = LineEnding::Lf.normalize(text);
        let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(&text).split('\n').collect();
        if self.cursors.len() < 2 || lines.len() != self.cursors.len() {
            return self.insert(&text, EditKind::Other, now, viewport_rows);
        }
        self.preedit.clear();
        let mut order: Vec<usize> = (0..self.cursors.len()).collect();
        order.sort_by_key(|&i| self.cursors[i].range().start);
        let edits = order.into_iter().zip(lines).map(|(i, line)| (i, self.cursors[i].range(), line.to_owned())).collect();
        self.replace(edits, EditKind::Other, now, viewport_rows);
    }

    pub(crate) fn set_preedit(&mut self, text: String) {
        self.preedit = text;
    }

    /// Deletes each caret's selection, or the text its movement would pass
    /// over.
    pub(crate) fn delete(&mut self, movement: CaretMove, kind: EditKind, now: Instant, viewport_rows: f64) {
        let edits = (0..self.cursors.len())
            .map(|i| {
                let mut cursor = self.cursors[i];
                if cursor.is_empty() {
                    self.move_head(&mut cursor, movement, viewport_rows);
                }
                (i, cursor.range(), String::new())
            })
            .collect();
        self.replace(edits, kind, now, viewport_rows);
    }

    /// Replaces text at the carets as one edit: each entry is a caret (by
    /// index), the range it replaces and what goes there. Overlapping ranges
    /// merge. Each caret ends up collapsed after its replacement.
    fn replace(&mut self, mut edits: Vec<(usize, Range<usize>, String)>, kind: EditKind, now: Instant, rows: f64) {
        let (before, version_before) = (self.selections(), self.version);
        edits.sort_by_key(|(_, range, _)| (range.start, range.end));
        let mut merged: Vec<(usize, Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (i, range, text) in edits {
            match merged.last_mut() {
                Some((last_i, last, last_text)) if range.start < last.end => {
                    last.end = last.end.max(range.end);
                    *last_i = (*last_i).max(i);
                    if last_text.is_empty() {
                        *last_text = text;
                    }
                }
                _ => merged.push((i, range, text)),
            }
        }

        // From the end backwards, so each change's indices are those of the
        // text as it was before the edit.
        let mut changes = Vec::new();
        for (_, range, text) in merged.iter().rev() {
            if !range.is_empty() {
                changes.push(Change::Remove { at: range.start, text: self.text.slice(range.clone()).to_string() });
                self.text.remove(range.clone());
            }
            if !text.is_empty() {
                self.text.insert(range.start, text);
                changes.push(Change::Insert { at: range.start, text: text.clone() });
            }
        }
        if !changes.is_empty() {
            self.new_version();
        }

        let mut shift = 0isize;
        let mut carets: Vec<(usize, usize)> = merged
            .iter()
            .map(|(i, range, text)| {
                let inserted = text.chars().count();
                let start = range.start.saturating_add_signed(shift);
                shift += inserted as isize - range.len() as isize;
                (*i, start + inserted)
            })
            .collect();
        carets.sort_by_key(|(i, _)| *i);
        self.cursors = carets.into_iter().map(|(_, caret)| Cursor::at(caret)).collect();
        self.merge_cursors();
        self.reveal_caret(rows);
        self.record(kind, now, changes, before, version_before);
    }

    /// Merges carets whose selections overlap, or that sit at the same
    /// place, keeping the later-added one's direction and order.
    fn merge_cursors(&mut self) {
        if self.cursors.len() < 2 {
            return;
        }
        let mut order: Vec<usize> = (0..self.cursors.len()).collect();
        order.sort_by_key(|&i| (self.cursors[i].range().start, self.cursors[i].range().end));
        let mut groups: Vec<(Range<usize>, usize)> = Vec::with_capacity(order.len());
        for i in order {
            let range = self.cursors[i].range();
            match groups.last_mut() {
                Some((group, kept))
                    if range.start < group.end
                        || range.start == group.start
                        || (range.is_empty() && range.start == group.end) =>
                {
                    group.end = group.end.max(range.end);
                    *kept = (*kept).max(i);
                }
                _ => groups.push((range, i)),
            }
        }
        if groups.len() == self.cursors.len() {
            return;
        }
        groups.sort_by_key(|(_, kept)| *kept);
        self.cursors = groups
            .into_iter()
            .map(|(range, kept)| {
                let cursor = self.cursors[kept];
                if cursor.range() == range {
                    cursor
                } else if cursor.caret < cursor.anchor {
                    Cursor::selecting(range.end, range.start)
                } else {
                    Cursor::selecting(range.start, range.end)
                }
            })
            .collect();
    }

    /// ⌘Z: reverts the last undo step and restores the carets from before
    /// it.
    pub(crate) fn undo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        if let Some(restored) = self.history.undo(&mut self.text) {
            self.restore(restored.selections, restored.version, viewport_rows);
        }
    }

    /// ⌘⇧Z: makes the last undone step again and restores the carets from
    /// after it.
    pub(crate) fn redo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        if let Some(restored) = self.history.redo(&mut self.text) {
            self.restore(restored.selections, restored.version, viewport_rows);
        }
    }

    fn restore(&mut self, selections: Vec<Selection>, version: u64, viewport_rows: f64) {
        self.cursors = selections.into_iter().map(|s| Cursor::selecting(s.anchor, s.caret)).collect();
        self.version = version;
        self.reveal_caret(viewport_rows);
    }

    fn new_version(&mut self) {
        self.last_version += 1;
        self.version = self.last_version;
    }

    fn selections(&self) -> Vec<Selection> {
        self.cursors.iter().map(|c| c.selection()).collect()
    }

    /// Adds an edit's changes to the undo history.
    fn record(&mut self, kind: EditKind, at: Instant, changes: Vec<Change>, before: Vec<Selection>, version_before: u64) {
        self.history.record(Edit {
            kind,
            at,
            changes,
            before,
            after: self.selections(),
            version_before,
            version_after: self.version,
        });
    }

    /// The selected text, or `None` with nothing selected. Several
    /// selections are joined top to bottom, one per line.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let mut ranges: Vec<Range<usize>> =
            self.cursors.iter().map(|c| c.range()).filter(|r| !r.is_empty()).collect();
        ranges.sort_by_key(|r| r.start);
        let texts: Vec<String> = ranges.into_iter().map(|r| self.text.slice(r).to_string()).collect();
        (!texts.is_empty()).then(|| texts.join("\n"))
    }

    /// ⌘X's edit: deletes every selection, leaving carets without one
    /// where they are.
    pub(crate) fn delete_selections(&mut self, now: Instant, viewport_rows: f64) {
        self.insert("", EditKind::Other, now, viewport_rows);
    }

    /// Scrolls by `rows`, keeping the last line at the bottom of the
    /// viewport at most.
    pub(crate) fn scroll_by(&mut self, rows: f64, viewport_rows: f64) {
        let max = (self.text.len_lines() as f64 - viewport_rows).max(0.0);
        self.scroll_top = (self.scroll_top + rows).clamp(0.0, max);
    }

    /// Scrolls just enough to show the primary caret's whole row.
    fn reveal_caret(&mut self, viewport_rows: f64) {
        let line = self.text.char_to_line(self.primary().caret) as f64;
        if line < self.scroll_top {
            self.scroll_top = line;
        } else if line + 1.0 > self.scroll_top + viewport_rows {
            self.scroll_top = line + 1.0 - viewport_rows;
        }
        self.scroll_by(0.0, viewport_rows);
    }

    fn move_vertically(&self, cursor: &mut Cursor, target_line: usize) {
        let goal = cursor.goal.unwrap_or_else(|| self.display_column(cursor.caret));
        cursor.caret = self.text.line_to_char(target_line) + self.char_column_at(target_line, goal);
        cursor.goal = Some(goal);
    }

    /// A position as (line, char column).
    fn line_column(&self, position: usize) -> (usize, usize) {
        let line = self.text.char_to_line(position);
        (line, position - self.text.line_to_char(line))
    }

    /// A position's display column.
    fn display_column(&self, position: usize) -> usize {
        let (line, column) = self.line_column(position);
        display_columns(self.text.line(line).chars().take(column))
    }

    fn caret_at(&self, position: usize) -> Caret {
        Caret { line: self.text.char_to_line(position), column: self.display_column(position) }
    }

    /// A line's chars, without its line ending.
    fn line_chars(&self, line: usize) -> Vec<char> {
        self.text.line(line).chars().take(self.line_len(line)).collect()
    }

    /// Chars in a line, without its line ending.
    fn line_len(&self, line: usize) -> usize {
        let slice = self.text.line(line);
        let mut len = slice.len_chars();
        while len > 0 && matches!(slice.char(len - 1), '\n' | '\r') {
            len -= 1;
        }
        len
    }

    /// The char column of the last position at or before display `column`.
    fn char_column_at(&self, line: usize, column: usize) -> usize {
        let mut cells = 0;
        for (i, c) in self.text.line(line).chars().take(self.line_len(line)).enumerate() {
            cells = display_columns_from(cells, c);
            if cells > column {
                return i;
            }
        }
        self.line_len(line)
    }

    pub(crate) fn view(&self, viewport_rows: f64) -> EditorView {
        let line_count = self.text.len_lines();
        let first = (self.scroll_top.floor() as usize).min(line_count);
        let end = ((self.scroll_top + viewport_rows).ceil() as usize).min(line_count);
        let mut ranges: Vec<Range<usize>> =
            self.cursors.iter().map(|c| c.range()).filter(|r| !r.is_empty()).collect();
        ranges.sort_by_key(|r| r.start);
        let lines = (first..end)
            .map(|index| VisibleLine {
                index,
                text: self.grid_text(index),
                selections: self.selected_columns(index, &ranges),
            })
            .collect();
        let caret = self.caret_at(self.primary().caret);
        let mut positions: Vec<usize> = self.cursors.iter().map(|c| c.caret).collect();
        positions.sort_unstable();
        let visible_from = self.text.line_to_char(first);
        let visible_to = if end < line_count { self.text.line_to_char(end) } else { self.text.len_chars() + 1 };
        let carets = positions
            .into_iter()
            .filter(|p| (visible_from..visible_to).contains(p))
            .map(|p| self.caret_at(p))
            .collect();
        let preedit = (!self.preedit.is_empty()).then(|| Preedit {
            line: caret.line,
            column: caret.column,
            width: self.preedit.chars().fold(caret.column, display_columns_from) - caret.column,
        });
        EditorView {
            title: self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: self.path.clone(),
            read_only: false,
            modified: self.version != self.saved_version,
            line_count,
            scroll_top: self.scroll_top,
            lines,
            caret,
            carets,
            preedit,
        }
    }

    /// The display columns of `line` that the selections (sorted, not
    /// overlapping) cover, left to right.
    fn selected_columns(&self, line: usize, ranges: &[Range<usize>]) -> Vec<Range<usize>> {
        let line_start = self.text.line_to_char(line);
        let text_end = line_start + self.line_len(line);
        let next_line = line_start + self.text.line(line).len_chars();
        let first = ranges.partition_point(|r| r.end <= line_start);
        let columns = |to: usize| display_columns(self.text.slice(line_start..to).chars());
        ranges[first..]
            .iter()
            .take_while(|r| r.start < next_line)
            .map(|r| {
                let from = columns(r.start.max(line_start));
                let to = columns(r.end.min(text_end)) + usize::from(r.end > text_end);
                from..to
            })
            .collect()
    }

    /// A line as laid out on the grid: tabs expanded, no line ending, cut
    /// at [`MAX_VISIBLE_COLUMNS`], with the preedit spliced in at the
    /// primary caret.
    fn grid_text(&self, line: usize) -> String {
        let chars = self.text.line(line).chars();
        let (caret_line, caret_column) = self.line_column(self.primary().caret);
        let chars: Box<dyn Iterator<Item = char>> = if line == caret_line && !self.preedit.is_empty() {
            Box::new(chars.clone().take(caret_column).chain(self.preedit.chars()).chain(chars.skip(caret_column)))
        } else {
            Box::new(chars)
        };
        let mut text = String::new();
        let mut column = 0;
        for c in chars {
            if matches!(c, '\n' | '\r') || column >= MAX_VISIBLE_COLUMNS {
                break;
            }
            if c == '\t' {
                let next = (column / TAB_WIDTH + 1) * TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', next - column));
                column = next;
            } else {
                text.push(c);
                column += c.width().unwrap_or(0);
            }
        }
        text
    }
}

/// Grid columns taken by `chars` starting at column 0.
fn display_columns(chars: impl Iterator<Item = char>) -> usize {
    chars.fold(0, display_columns_from)
}

/// The column after `c` when it starts at `column`.
fn display_columns_from(column: usize, c: char) -> usize {
    if c == '\t' { (column / TAB_WIDTH + 1) * TAB_WIDTH } else { column + c.width().unwrap_or(0) }
}
