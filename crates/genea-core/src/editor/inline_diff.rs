//! An inline diff on the editor surface (ticket #55): the lines a base has
//! and the buffer doesn't are drawn as rows of their own, just above the
//! line they were removed before, and the buffer's lines that the base
//! doesn't have are marked as added.
//!
//! The editor only lays the diff out; the project works out the hunks in
//! the background (`crate::diff`) and hands them over with
//! [`Editor::set_inline_diff`]. Removed rows count as rows for scrolling
//! and for the view (`VisibleLine::row`), but carets never land on them:
//! Up and Down move by the file's own rows, as before.

use std::ops::Range;

use ropey::Rope;
use unicode_width::UnicodeWidthChar;

use super::{Editor, MAX_VISIBLE_COLUMNS, TAB_WIDTH};
use crate::{
    diff::Hunk,
    view::{DiffAgainst, InlineDiffView, RemovedLine, VisibleLine},
};

/// A diff for the editor to show: the base and the hunks against it,
/// which may be a moment behind the buffer.
pub(crate) struct InlineDiff {
    pub(crate) against: DiffAgainst,
    pub(crate) base: Rope,
    pub(crate) hunks: Vec<Hunk>,
}

/// What a row shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum RowContent {
    /// A line of the buffer.
    Line(usize),
    /// A line of the base that was removed just above line `before`.
    Removed { before: usize, base_line: usize },
}

/// Removed lines drawn together, above one line of the buffer.
struct Block {
    before: usize,
    base: Range<usize>,
    /// The row the first of them is drawn on.
    row: usize,
}

impl Editor {
    /// Shows `diff` inline, or the file plainly for `None`.
    pub(crate) fn set_inline_diff(&mut self, diff: Option<InlineDiff>, viewport_rows: f64) {
        self.inline_diff = diff;
        self.scroll_by(0.0, viewport_rows);
    }

    /// The blocks of removed lines that show (not hidden in a fold), top
    /// to bottom. Hunks behind the buffer are clamped to it.
    fn removed_blocks(&self) -> Vec<Block> {
        let Some(diff) = &self.inline_diff else { return Vec::new() };
        let last = self.text.len_lines() - 1;
        let base_lines = diff.base.len_lines();
        let mut blocks = Vec::new();
        let mut above = 0;
        for hunk in &diff.hunks {
            let base = hunk.base.start.min(base_lines)..hunk.base.end.min(base_lines);
            if base.is_empty() {
                continue;
            }
            let before = hunk.lines.start.min(last);
            let row = self.row_of(before);
            if self.line_at_row(row) != before {
                continue;
            }
            above += base.len();
            blocks.push(Block { before, row: row + above - base.len(), base });
        }
        blocks
    }

    /// The row a line is drawn on, counting removed rows.
    pub(super) fn display_row_of(&self, line: usize) -> usize {
        let removed: usize = self.removed_blocks().iter().filter(|b| b.before <= line).map(|b| b.base.len()).sum();
        self.row_of(line) + removed
    }

    /// Rows in the file, counting removed rows.
    pub(super) fn display_row_count(&self) -> usize {
        self.row_count() + self.removed_blocks().iter().map(|b| b.base.len()).sum::<usize>()
    }

    /// Each row of `rows` with what it shows.
    pub(super) fn display_rows(&self, rows: Range<usize>) -> Vec<(usize, RowContent)> {
        if self.inline_diff.is_none() {
            return self.rows_to_lines(rows).into_iter().map(|(row, line)| (row, RowContent::Line(line))).collect();
        }
        let blocks = self.removed_blocks();
        rows.map(|row| {
            let mut above = 0;
            for block in &blocks {
                if row < block.row {
                    break;
                }
                if row < block.row + block.base.len() {
                    let base_line = block.base.start + (row - block.row);
                    return (row, RowContent::Removed { before: block.before, base_line });
                }
                above += block.base.len();
            }
            (row, RowContent::Line(self.line_at_row(row - above)))
        })
        .collect()
    }

    /// The diff on the shown rows, for the view.
    pub(super) fn inline_diff_view(&self, lines: &[VisibleLine], shown: &[(usize, RowContent)]) -> Option<InlineDiffView> {
        let diff = self.inline_diff.as_ref()?;
        let added = lines.iter().map(|line| line.index).filter(|&index| is_added(&diff.hunks, index)).collect();
        let removed = shown
            .iter()
            .filter_map(|&(row, content)| match content {
                RowContent::Removed { before, base_line } => {
                    Some(RemovedLine { row, before, text: grid_text(&diff.base, base_line) })
                }
                RowContent::Line(_) => None,
            })
            .collect();
        Some(InlineDiffView { against: diff.against, added, removed, change: None })
    }
}

/// Whether a buffer line is in a hunk's lines.
fn is_added(hunks: &[Hunk], line: usize) -> bool {
    let after = hunks.partition_point(|h| h.lines.end <= line);
    hunks.get(after).is_some_and(|h| h.lines.contains(&line))
}

/// A base line as laid out on the grid: tabs expanded, no line ending, cut
/// at [`MAX_VISIBLE_COLUMNS`].
fn grid_text(base: &Rope, line: usize) -> String {
    let mut text = String::new();
    let mut column = 0;
    for c in base.line(line).chars() {
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
