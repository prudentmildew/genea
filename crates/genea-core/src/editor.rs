//! One open file on the editor surface: its text, selection, scroll position
//! and saved state.
//!
//! Every edit goes through `insert` or `delete`, which give the buffer a
//! fresh version (compared with the saved one for `modified`) and record
//! the change in the undo history. Multi-caret and syntax build on those
//! two (tickets #52, #24).

use std::{ops::Range, path::PathBuf, time::Instant};

use ropey::Rope;
use tree_sitter::{InputEdit, Point};
use unicode_width::UnicodeWidthChar;

use crate::{
    command::CaretMove,
    history::{Change, Edit, EditKind, History, Selection},
    syntax::{Highlight, ParseJob, Parsed, Syntax},
    text::{self, LineEnding},
    view::{Caret, EditorView, HighlightSpan, Preedit, VisibleLine},
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
    /// Every line break typed or pasted is converted to this.
    line_ending: LineEnding,
    /// The IME's marked text, drawn at the caret; never in `text`.
    preedit: String,
    /// Identifies the text: every edit gives it a fresh one, and undo and
    /// redo restore the one the text had then.
    version: u64,
    /// The newest version handed out, so a fresh one is never reused.
    last_version: u64,
    /// The version last written to disk (or read from it).
    saved_version: u64,
    /// Edits and saves are refused: the file isn't valid UTF-8, so writing
    /// the buffer back would change bytes the user never touched.
    read_only: bool,
    history: History,
    /// The tree and highlights, for files in a highlighted language.
    syntax: Option<Syntax>,
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
        let syntax = Syntax::for_file(&path, &text);
        Editor {
            path,
            text,
            caret: 0,
            anchor: 0,
            goal_column: None,
            scroll_top: 0.0,
            line_ending,
            preedit: String::new(),
            version: 0,
            last_version: 0,
            saved_version: 0,
            read_only: false,
            history: History::default(),
            syntax,
        }
    }

    /// The same editor, refusing edits and saves.
    pub(crate) fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// Moves the caret. With `extend`, the selection's anchor stays put, so
    /// the selection grows or shrinks; without, the selection collapses.
    pub(crate) fn move_caret(&mut self, movement: CaretMove, extend: bool, viewport_rows: f64) {
        let (start, end) = self.selection();
        match movement {
            CaretMove::Left if !extend && start != end => (self.caret, self.goal_column) = (start, None),
            CaretMove::Right if !extend && start != end => (self.caret, self.goal_column) = (end, None),
            _ => self.move_head(movement, viewport_rows),
        }
        if !extend {
            self.anchor = self.caret;
        }
        self.reveal_caret(viewport_rows);
    }

    pub(crate) fn select_all(&mut self, viewport_rows: f64) {
        self.anchor = 0;
        self.caret = self.text.len_chars();
        self.goal_column = None;
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
            CaretMove::WordLeft if column > 0 => self.set_caret(line, text::word_start(&self.line_chars(line), column)),
            CaretMove::WordRight if column < self.line_len(line) => {
                self.set_caret(line, text::word_end(&self.line_chars(line), column))
            }
            CaretMove::WordLeft => self.move_head(CaretMove::Left, viewport_rows),
            CaretMove::WordRight => self.move_head(CaretMove::Right, viewport_rows),
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
    /// With `extend`, the selection's anchor stays put.
    pub(crate) fn place_caret(&mut self, line: usize, column: usize, extend: bool, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        let char_column = self.char_column_at(line, column);
        self.set_caret(line, char_column);
        if !extend {
            self.anchor = self.caret;
        }
        self.reveal_caret(viewport_rows);
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
        self.anchor = line_start + start;
        self.caret = line_start + end;
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
    }

    /// Selects a whole line and its line break, with the caret at the start
    /// of the next line.
    pub(crate) fn select_line(&mut self, line: usize, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        self.anchor = self.text.line_to_char(line);
        self.caret = self.anchor + self.text.line(line).len_chars();
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
    }

    /// The file's line ending, kept for every line break typed.
    pub(crate) fn line_ending(&self) -> LineEnding {
        self.line_ending
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

    /// Inserts `text` at the caret, replacing the selection, and puts the
    /// caret after it. Line breaks become the file's line ending. `now` is
    /// the host clock's time, for grouping undo steps.
    pub(crate) fn insert(&mut self, text: &str, kind: EditKind, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only || (text.is_empty() && self.anchor == self.caret) {
            return;
        }
        let text = self.line_ending.normalize(text);
        let (before, version_before) = (self.selection_state(), self.version);
        let mut changes: Vec<Change> = self.delete_selection().into_iter().collect();
        if !text.is_empty() {
            self.splice(self.caret..self.caret, &text);
            self.new_version();
            changes.push(Change::Insert { at: self.caret, text: text.clone() });
        }
        self.caret += text.chars().count();
        self.anchor = self.caret;
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
        self.record(kind, now, changes, before, version_before);
    }

    pub(crate) fn set_preedit(&mut self, text: String) {
        if self.read_only {
            return;
        }
        self.preedit = text;
    }

    /// Deletes the selection, or the text the movement would pass over.
    pub(crate) fn delete(&mut self, movement: CaretMove, kind: EditKind, now: Instant, viewport_rows: f64) {
        if self.read_only {
            return;
        }
        let (before, version_before) = (self.selection_state(), self.version);
        if self.anchor == self.caret {
            self.move_head(movement, viewport_rows);
        }
        let changes = self.delete_selection().into_iter().collect();
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
        self.record(kind, now, changes, before, version_before);
    }

    /// Removes the selected text and collapses the selection where it was.
    fn delete_selection(&mut self) -> Option<Change> {
        let (start, end) = self.selection();
        let removed = (start != end).then(|| {
            let text = self.text.slice(start..end).to_string();
            self.splice(start..end, "");
            self.new_version();
            Change::Remove { at: start, text }
        });
        self.caret = start;
        self.anchor = start;
        removed
    }

    /// ⌘Z: reverts the last undo step and restores the selection from
    /// before it.
    pub(crate) fn undo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        let mut history = std::mem::take(&mut self.history);
        if let Some(restored) = history.undo(|change| self.apply(change)) {
            self.restore(restored.selection, restored.version, viewport_rows);
        }
        self.history = history;
    }

    /// ⌘⇧Z: makes the last undone step again and restores the selection
    /// from after it.
    pub(crate) fn redo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        let mut history = std::mem::take(&mut self.history);
        if let Some(restored) = history.redo(|change| self.apply(change)) {
            self.restore(restored.selection, restored.version, viewport_rows);
        }
        self.history = history;
    }

    fn restore(&mut self, selection: Selection, version: u64, viewport_rows: f64) {
        self.anchor = selection.anchor;
        self.caret = selection.caret;
        self.version = version;
        self.goal_column = None;
        self.reveal_caret(viewport_rows);
    }

    fn new_version(&mut self) {
        self.last_version += 1;
        self.version = self.last_version;
    }

    fn selection_state(&self) -> Selection {
        Selection { anchor: self.anchor, caret: self.caret }
    }

    /// Adds an edit's changes to the undo history.
    fn record(&mut self, kind: EditKind, at: Instant, changes: Vec<Change>, before: Selection, version_before: u64) {
        self.history.record(Edit {
            kind,
            at,
            changes,
            before,
            after: self.selection_state(),
            version_before,
            version_after: self.version,
        });
    }

    /// Replaces a char range of the text. Every change to the text goes
    /// through here (typing, deleting, undo and redo): it moves the syntax
    /// tree and highlights with the text.
    fn splice(&mut self, chars: Range<usize>, text: &str) {
        let start_byte = self.text.char_to_byte(chars.start);
        let old_end_byte = self.text.char_to_byte(chars.end);
        let start_position = self.point(start_byte);
        let old_end_position = self.point(old_end_byte);
        self.text.remove(chars.clone());
        self.text.insert(chars.start, text);
        let new_end_byte = start_byte + text.len();
        let new_end_position = self.point(new_end_byte);
        if let Some(syntax) = &mut self.syntax {
            let edit =
                InputEdit { start_byte, old_end_byte, new_end_byte, start_position, old_end_position, new_end_position };
            syntax.edit(edit);
        }
    }

    /// A byte offset as a tree-sitter point (row, byte column).
    fn point(&self, byte: usize) -> Point {
        let row = self.text.byte_to_line(byte);
        Point { row, column: byte - self.text.line_to_byte(row) }
    }

    /// A background parse to start, if the syntax tree is behind the text
    /// and none is running.
    pub(crate) fn start_parse(&mut self) -> Option<ParseJob> {
        self.syntax.as_mut()?.start_parse(&self.text)
    }

    /// Takes a finished background parse.
    pub(crate) fn parsed(&mut self, parsed: Parsed) {
        if let Some(syntax) = &mut self.syntax {
            syntax.parsed(parsed);
        }
    }

    /// Applies an undo history change to the text.
    fn apply(&mut self, change: &Change) {
        match change {
            Change::Insert { at, text } => self.splice(*at..*at, text),
            Change::Remove { at, text } => self.splice(*at..*at + text.chars().count(), ""),
        }
    }

    /// The selected text, or `None` with nothing selected.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let (start, end) = self.selection();
        (start != end).then(|| self.text.slice(start..end).to_string())
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
        let lines = (first..end)
            .map(|index| {
                let (text, highlights) = self.grid_line(index);
                VisibleLine { index, text, selections: self.selected_columns(index).into_iter().collect(), highlights }
            })
            .collect();
        let caret = Caret { line: self.text.char_to_line(self.caret), column: self.caret_display_column() };
        let preedit = (!self.preedit.is_empty()).then(|| Preedit {
            line: caret.line,
            column: caret.column,
            width: self.preedit.chars().fold(caret.column, display_columns_from) - caret.column,
        });
        EditorView {
            title: self.path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            path: self.path.clone(),
            read_only: self.read_only,
            modified: self.version != self.saved_version,
            line_count,
            scroll_top: self.scroll_top,
            lines,
            caret,
            preedit,
        }
    }

    /// The display columns of `line` the selection covers, if any.
    fn selected_columns(&self, line: usize) -> Option<Range<usize>> {
        let (start, end) = self.selection();
        let line_start = self.text.line_to_char(line);
        let text_end = line_start + self.line_len(line);
        let next_line = line_start + self.text.line(line).len_chars();
        if start == end || end <= line_start || start >= next_line {
            return None;
        }
        let columns = |to: usize| display_columns(self.text.slice(line_start..to).chars());
        let from = columns(start.max(line_start));
        let to = columns(end.min(text_end)) + usize::from(end > text_end);
        Some(from..to)
    }

    /// A line as laid out on the grid: tabs expanded, no line ending, cut
    /// at [`MAX_VISIBLE_COLUMNS`], with the preedit spliced in at the caret
    /// (and left plain). Also its highlight spans, in display columns.
    fn grid_line(&self, line: usize) -> (String, Vec<HighlightSpan>) {
        let line_start = self.text.line_to_byte(line);
        let slice = self.text.line(line);
        let spans = self.syntax.as_ref().map_or(&[][..], |s| s.spans(line_start..line_start + slice.len_bytes()));
        // Each char with its byte offset in the file; the preedit has none.
        let chars = slice.chars().scan(line_start, |byte, c| {
            let at = *byte;
            *byte += c.len_utf8();
            Some((Some(at), c))
        });
        let (caret_line, caret_column) = self.caret_line_column();
        let chars: Box<dyn Iterator<Item = (Option<usize>, char)>> = if line == caret_line && !self.preedit.is_empty() {
            let preedit = self.preedit.chars().map(|c| (None, c));
            Box::new(chars.clone().take(caret_column).chain(preedit).chain(chars.skip(caret_column)))
        } else {
            Box::new(chars)
        };

        let mut text = String::new();
        let mut highlights: Vec<HighlightSpan> = Vec::new();
        let mut spans = spans.iter().peekable();
        let mut column = 0;
        for (byte, c) in chars {
            if matches!(c, '\n' | '\r') || column >= MAX_VISIBLE_COLUMNS {
                break;
            }
            let start = column;
            if c == '\t' {
                let next = (column / TAB_WIDTH + 1) * TAB_WIDTH;
                text.extend(std::iter::repeat_n(' ', next - column));
                column = next;
            } else {
                text.push(c);
                column += c.width().unwrap_or(0);
            }
            let highlight = byte.and_then(|byte| {
                while spans.next_if(|s| (s.end as usize) <= byte).is_some() {}
                spans.peek().filter(|s| (s.start as usize) <= byte).map(|s| s.highlight)
            });
            push_highlight(&mut highlights, highlight, start..column);
        }
        (text, highlights)
    }
}

/// Adds a char's columns to the line's highlight spans, extending the last
/// span when it continues it.
fn push_highlight(spans: &mut Vec<HighlightSpan>, highlight: Option<Highlight>, columns: Range<usize>) {
    let Some(highlight) = highlight else { return };
    match spans.last_mut() {
        Some(last) if last.highlight == highlight && last.columns.end == columns.start => last.columns.end = columns.end,
        _ => spans.push(HighlightSpan { columns, highlight }),
    }
}

/// One tab's place in a file: where its caret and selection are and how far
/// it is scrolled (ticket #31). An open file has one `Editor` however many
/// tabs show it, so edits show in every tab; each tab keeps its own cursor,
/// and the project swaps it in while that tab is focused or drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cursor {
    caret: usize,
    anchor: usize,
    goal_column: Option<usize>,
    scroll_top: f64,
    preedit: String,
}

impl Editor {
    /// The file, relative to the project root when it is inside it.
    pub(crate) fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// The buffer has edits that aren't on disk yet.
    pub(crate) fn is_modified(&self) -> bool {
        self.version != self.saved_version
    }

    pub(crate) fn cursor(&self) -> Cursor {
        Cursor {
            caret: self.caret,
            anchor: self.anchor,
            goal_column: self.goal_column,
            scroll_top: self.scroll_top,
            preedit: self.preedit.clone(),
        }
    }

    /// Puts a tab's cursor back. The text may have changed under it in
    /// another tab, so positions are clamped to the text.
    pub(crate) fn set_cursor(&mut self, cursor: &Cursor, viewport_rows: f64) {
        let len = self.text.len_chars();
        self.caret = cursor.caret.min(len);
        self.anchor = cursor.anchor.min(len);
        self.goal_column = cursor.goal_column;
        self.scroll_top = cursor.scroll_top;
        self.preedit.clone_from(&cursor.preedit);
        self.scroll_by(0.0, viewport_rows);
    }
}

impl Cursor {
    /// The same cursor without the IME's marked text, for a tab that loses
    /// focus.
    pub(crate) fn parked(mut self) -> Self {
        self.preedit.clear();
        self
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
