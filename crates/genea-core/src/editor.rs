//! One open file on the editor surface: its text, caret and scroll position.
//!
//! Read-only for now (ticket #20). The editing tickets add the edit path,
//! undo, selections and syntax here or in crates next to it.

use std::path::PathBuf;

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use crate::view::{Caret, EditorView, VisibleLine};

/// Grid columns a tab advances to (the next multiple of this).
const TAB_WIDTH: usize = 4;

/// Columns of a line that make it into view state. Longer lines are cut, so
/// a minified file can't make every sync copy and shape megabytes.
pub const MAX_VISIBLE_COLUMNS: usize = 1000;

pub(crate) struct Editor {
    path: PathBuf,
    text: Rope,
    /// Char index into `text`.
    caret: usize,
    /// First visible row; fractional while scrolling smoothly.
    scroll_top: f64,
}

impl Editor {
    pub(crate) fn new(path: PathBuf, text: Rope) -> Self {
        Editor { path, text, caret: 0, scroll_top: 0.0 }
    }

    pub(crate) fn view(&self, viewport_rows: f64) -> EditorView {
        let line_count = self.text.len_lines();
        let first = (self.scroll_top.floor() as usize).min(line_count);
        let end = ((self.scroll_top + viewport_rows).ceil() as usize).min(line_count);
        let lines = (first..end).map(|index| VisibleLine { index, text: self.grid_text(index) }).collect();
        let line = self.text.char_to_line(self.caret);
        let column = display_columns(self.text.slice(self.text.line_to_char(line)..self.caret).chars());
        EditorView {
            title: self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: self.path.clone(),
            read_only: true,
            line_count,
            scroll_top: self.scroll_top,
            lines,
            caret: Caret { line, column },
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
    chars.fold(0, |column, c| {
        if c == '\t' { (column / TAB_WIDTH + 1) * TAB_WIDTH } else { column + c.width().unwrap_or(0) }
    })
}
