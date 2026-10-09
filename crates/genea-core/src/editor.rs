//! One open file on the editor surface: its text, caret and scroll position.
//!
//! Read-only for now (ticket #20). The editing tickets add the edit path,
//! undo, selections and syntax here or in crates next to it.

use std::path::PathBuf;

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::{
    command::CaretMove,
    view::{Caret, EditorView, VisibleLine},
};

/// Grid columns a tab advances to (the next multiple of this).
const TAB_WIDTH: usize = 4;

/// Columns of a line that make it into view state. Longer lines are cut, so
/// a minified file can't make every sync copy and shape megabytes.
pub const MAX_VISIBLE_COLUMNS: usize = 1000;

pub(crate) struct Editor {
    path: PathBuf,
    text: Rope,
    /// Char index into `text`: where the caret is, the moving end of the
    /// selection.
    caret: usize,
    /// Char index of the selection's fixed end; equal to `caret` when
    /// nothing is selected.
    anchor: usize,
    /// The display column Up and Down aim for, kept across short lines.
    /// Set by the first vertical move; cleared by every other move.
    goal_column: Option<usize>,
    /// First visible row; fractional while scrolling smoothly.
    scroll_top: f64,
}

impl Editor {
    pub(crate) fn new(path: PathBuf, text: Rope) -> Self {
        Editor { path, text, caret: 0, anchor: 0, goal_column: None, scroll_top: 0.0 }
    }

    /// Moves the caret. With `extend`, the selection's anchor stays put, so
    /// the selection grows or shrinks; without, the selection collapses.
    pub(crate) fn move_caret(&mut self, movement: CaretMove, extend: bool, viewport_rows: f64) {
        self.move_head(movement, viewport_rows);
        if !extend {
            self.anchor = self.caret;
        }
        self.reveal_caret(viewport_rows);
    }

    /// Moves the caret alone, leaving the anchor where it was.
    fn move_head(&mut self, movement: CaretMove, viewport_rows: f64) {
        let (line, column) = self.caret_line_column();
        let last_line = self.text.len_lines() - 1;
        let page = (viewport_rows.floor() as usize).max(1);
        match movement {
            CaretMove::Left if column > 0 => self.set_caret(line, column - 1),
            CaretMove::Left if line > 0 => self.set_caret(line - 1, self.line_len(line - 1)),
            CaretMove::Left => {}
            CaretMove::Right if column < self.line_len(line) => self.set_caret(line, column + 1),
            CaretMove::Right if line < last_line => self.set_caret(line + 1, 0),
            CaretMove::Right => {}
            CaretMove::Up => self.move_vertically(line.saturating_sub(1)),
            CaretMove::Down => self.move_vertically((line + 1).min(last_line)),
            CaretMove::PageUp => {
                self.scroll_by(-(page as f64), viewport_rows);
                self.move_vertically(line.saturating_sub(page));
            }
            CaretMove::PageDown => {
                self.scroll_by(page as f64, viewport_rows);
                self.move_vertically((line + page).min(last_line));
            }
            CaretMove::LineStart => self.set_caret(line, 0),
            CaretMove::LineEnd => self.set_caret(line, self.line_len(line)),
            CaretMove::DocumentStart => self.set_caret(0, 0),
            CaretMove::DocumentEnd => self.set_caret(last_line, self.line_len(last_line)),
        }
    }

    /// Puts the caret at the last text position at or before a grid cell.
    pub(crate) fn place_caret(&mut self, line: usize, column: usize, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        let char_column = self.char_column_at(line, column);
        self.set_caret(line, char_column);
        self.anchor = self.caret;
        self.reveal_caret(viewport_rows);
    }

    /// Inserts `text` at the caret, replacing the selection, and puts the
    /// caret after it.
    pub(crate) fn insert(&mut self, text: &str, viewport_rows: f64) {
        self.delete_selection();
        self.text.insert(self.caret, text);
        self.caret += text.chars().count();
        self.anchor = self.caret;
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
    }

    /// Deletes the selection, or the text the movement would pass over.
    pub(crate) fn delete(&mut self, movement: CaretMove, viewport_rows: f64) {
        if self.anchor == self.caret {
            self.move_head(movement, viewport_rows);
        }
        self.delete_selection();
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
    }

    /// Removes the selected text and collapses the selection where it was.
    fn delete_selection(&mut self) {
        let (start, end) = self.selection();
        if start != end {
            self.text.remove(start..end);
        }
        self.caret = start;
        self.anchor = start;
    }

    /// The selection as an ordered char range.
    fn selection(&self) -> (usize, usize) {
        (self.anchor.min(self.caret), self.anchor.max(self.caret))
    }

    /// Scrolls by `rows`, keeping the last line at the bottom of the
    /// viewport at most.
    pub(crate) fn scroll_by(&mut self, rows: f64, viewport_rows: f64) {
        let max = (self.text.len_lines() as f64 - viewport_rows).max(0.0);
        self.scroll_top = (self.scroll_top + rows).clamp(0.0, max);
    }

    /// Scrolls just enough to show the caret's whole row.
    fn reveal_caret(&mut self, viewport_rows: f64) {
        let line = self.text.char_to_line(self.caret) as f64;
        if line < self.scroll_top {
            self.scroll_top = line;
        } else if line + 1.0 > self.scroll_top + viewport_rows {
            self.scroll_top = line + 1.0 - viewport_rows;
        }
        self.scroll_by(0.0, viewport_rows);
    }

    fn move_vertically(&mut self, target_line: usize) {
        let goal = self.goal_column.unwrap_or_else(|| self.caret_display_column());
        let char_column = self.char_column_at(target_line, goal);
        self.caret = self.text.line_to_char(target_line) + char_column;
        self.goal_column = Some(goal);
    }

    /// Moves the caret to a (line, char column) and forgets the goal column.
    fn set_caret(&mut self, line: usize, char_column: usize) {
        self.caret = self.text.line_to_char(line) + char_column;
        self.goal_column = None;
    }

    /// The caret as (line, char column).
    fn caret_line_column(&self) -> (usize, usize) {
        let line = self.text.char_to_line(self.caret);
        (line, self.caret - self.text.line_to_char(line))
    }

    fn caret_display_column(&self) -> usize {
        let (line, column) = self.caret_line_column();
        display_columns(self.text.line(line).chars().take(column))
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
        let lines = (first..end).map(|index| VisibleLine { index, text: self.grid_text(index) }).collect();
        let caret = Caret { line: self.text.char_to_line(self.caret), column: self.caret_display_column() };
        EditorView {
            title: self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: self.path.clone(),
            read_only: true,
            line_count,
            scroll_top: self.scroll_top,
            lines,
            caret,
        }
    }

    /// A line as laid out on the grid: tabs expanded, no line ending, cut
    /// at [`MAX_VISIBLE_COLUMNS`].
    fn grid_text(&self, line: usize) -> String {
        let mut text = String::new();
        let mut column = 0;
        for c in self.text.line(line).chars() {
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
