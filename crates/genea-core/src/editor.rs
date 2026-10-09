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

/// Files over this size are *large files* (spec #19, Large files; ticket
/// #27): they open with no syntax tree, no highlighting and no language
/// intelligence, and the status bar says so. Decided once, at open, from the
/// size read; edits don't change it. Anything that starts per-file language
/// work (language servers, semantic tokens, …) checks [`Editor::is_large`].
pub const LARGE_FILE_BYTES: usize = 5 * 1024 * 1024;

/// One caret and its selection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CaretSelection {
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

impl CaretSelection {
    fn at(position: usize) -> Self {
        CaretSelection { caret: position, anchor: position, goal: None }
    }

    fn selecting(anchor: usize, caret: usize) -> Self {
        CaretSelection { caret, anchor, goal: None }
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
    /// primary. No two overlap (see `merge_carets`).
    carets: Vec<CaretSelection>,
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
    /// Edits and saves are refused: the file isn't valid UTF-8, so writing
    /// the buffer back would change bytes the user never touched.
    read_only: bool,
    history: History,
    /// Set when ⌃G or ⌃⌘G selects the word at the caret: the version and
    /// carets it left. While both still hold, occurrences match whole words
    /// only; any other caret change or edit ends that.
    whole_words: Option<(u64, Vec<CaretSelection>)>,
    /// The tree and highlights, for files in a highlighted language that
    /// aren't large.
    syntax: Option<Syntax>,
    /// Over [`LARGE_FILE_BYTES`] when opened: no syntax, no language
    /// intelligence.
    large: bool,
    /// Only the file's first screen is in (ticket #27): the OpenFile with
    /// this generation is still reading the rest. Read-only meanwhile.
    loading: Option<u64>,
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
        let large = text.len_bytes() > LARGE_FILE_BYTES;
        let syntax = if large { None } else { Syntax::for_file(&path) };
        Editor {
            path,
            text,
            carets: vec![CaretSelection::at(0)],
            scroll_top: 0.0,
            line_ending,
            preedit: String::new(),
            version: 0,
            last_version: 0,
            saved_version: 0,
            read_only: false,
            history: History::default(),
            whole_words: None,
            syntax,
            large,
            loading: None,
        }
    }

    /// An editor showing the first screen of a file that is still being
    /// read by the OpenFile with this `generation`; `size` is the file's
    /// size on disk, if known. Read-only, without syntax, until
    /// [`Editor::finish_loading`].
    pub(crate) fn loading(path: PathBuf, first_screen: Rope, size: Option<u64>, generation: u64) -> Self {
        let mut editor = Editor::new(path, first_screen).read_only();
        editor.syntax = None;
        editor.large = size.is_some_and(|size| size > LARGE_FILE_BYTES as u64);
        editor.loading = Some(generation);
        editor
    }

    /// Whether this editor is waiting for the rest of its file from the
    /// OpenFile with this generation.
    pub(crate) fn is_loading(&self, generation: u64) -> bool {
        self.loading == Some(generation)
    }

    /// Reading the rest of the file failed: stays as it is, read-only.
    pub(crate) fn stop_loading(&mut self) {
        self.loading = None;
    }

    /// The whole file is in: becomes `loaded` (an editor of the whole
    /// file), keeping this editor's carets and scroll position, which the
    /// first screen's text leaves valid (it is a prefix of the whole).
    pub(crate) fn finish_loading(&mut self, mut loaded: Editor, viewport_rows: f64) {
        loaded.set_cursor(&self.cursor(), viewport_rows);
        *self = loaded;
    }

    /// The same editor, refusing edits and saves.
    pub(crate) fn read_only(mut self) -> Self {
        self.read_only = true;
        self
    }

    pub(crate) fn is_read_only(&self) -> bool {
        self.read_only
    }

    /// A large file: no syntax, and no language intelligence may start for
    /// it (see [`LARGE_FILE_BYTES`]).
    pub(crate) fn is_large(&self) -> bool {
        self.large
    }

    fn primary(&self) -> CaretSelection {
        *self.carets.last().expect("an editor always has a caret")
    }

    /// Keeps only the primary caret.
    fn keep_only(&mut self, caret: CaretSelection) {
        self.carets = vec![caret];
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
        for i in 0..self.carets.len() {
            let mut cursor = self.carets[i];
            let range = cursor.range();
            match movement {
                CaretMove::Left if !extend && !range.is_empty() => (cursor.caret, cursor.goal) = (range.start, None),
                CaretMove::Right if !extend && !range.is_empty() => (cursor.caret, cursor.goal) = (range.end, None),
                _ => self.move_head(&mut cursor, movement, viewport_rows),
            }
            if !extend {
                cursor.collapse();
            }
            self.carets[i] = cursor;
        }
        self.merge_carets();
        self.reveal_caret(viewport_rows);
    }

    pub(crate) fn select_all(&mut self, viewport_rows: f64) {
        self.keep_only(CaretSelection::selecting(0, self.text.len_chars()));
        self.reveal_caret(viewport_rows);
    }

    /// Moves a caret alone, leaving its anchor where it was.
    fn move_head(&self, cursor: &mut CaretSelection, movement: CaretMove, viewport_rows: f64) {
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
        self.keep_only(cursor);
        self.reveal_caret(viewport_rows);
    }

    /// ⌥-click: adds a caret at a grid cell as the new primary, or removes
    /// the caret that is already there (unless it is the only one).
    pub(crate) fn add_caret(&mut self, line: usize, column: usize, viewport_rows: f64) {
        let position = self.position_at(line, column);
        let existing = self.carets.iter().position(|c| c.is_empty() && c.caret == position);
        match existing {
            Some(i) if self.carets.len() > 1 => {
                self.carets.remove(i);
            }
            Some(_) => {}
            None => self.carets.push(CaretSelection::at(position)),
        }
        self.merge_carets();
        self.reveal_caret(viewport_rows);
    }

    /// ⌃G: with nothing selected, selects the word at the primary caret;
    /// otherwise adds a caret selecting the next occurrence of the primary
    /// selection after it (wrapping around), as the new primary. A word
    /// selected this way only matches whole words.
    pub(crate) fn select_next_occurrence(&mut self, viewport_rows: f64) {
        if self.primary().is_empty() {
            self.select_word_at_primary();
        } else {
            let whole_words = self.word_search_on();
            let from = self.primary().range().end;
            let matches = self.occurrences(whole_words);
            let selected =
                |m: &&Range<usize>| self.carets.iter().any(|c| c.range().start < m.end && m.start < c.range().end);
            let next = matches.iter().filter(|m| m.start >= from).chain(&matches).find(|m| !selected(m)).cloned();
            if let Some(next) = next {
                self.carets.push(CaretSelection::selecting(next.start, next.end));
                self.merge_carets();
            }
            self.keep_word_search(whole_words);
        }
        self.reveal_caret(viewport_rows);
    }

    /// ⌃⇧G: removes the primary caret (the last one added), making the one
    /// before it primary. Does nothing with a single caret.
    pub(crate) fn unselect_last_occurrence(&mut self, viewport_rows: f64) {
        if self.carets.len() > 1 {
            let whole_words = self.word_search_on();
            self.carets.pop();
            self.keep_word_search(whole_words);
        }
        self.reveal_caret(viewport_rows);
    }

    /// ⌃⌘G: selects every occurrence of the primary selection (or of the
    /// word at the primary caret, as whole words), with a caret at each.
    /// The primary stays where it was.
    pub(crate) fn select_all_occurrences(&mut self, viewport_rows: f64) {
        if self.primary().is_empty() {
            self.select_word_at_primary();
        }
        let primary = self.primary();
        if !primary.is_empty() {
            let whole_words = self.word_search_on();
            let own = primary.range();
            self.carets = self
                .occurrences(whole_words)
                .into_iter()
                .filter(|m| m.end <= own.start || m.start >= own.end)
                .map(|m| CaretSelection::selecting(m.start, m.end))
                .chain([primary])
                .collect();
            self.merge_carets();
            self.keep_word_search(whole_words);
        }
        self.reveal_caret(viewport_rows);
    }

    /// Adds a caret on the line above (`up`) or below the primary one, at
    /// its goal column, as the new primary; or removes the primary if the
    /// caret before it is on that line (it was cloned the other way).
    pub(crate) fn clone_caret(&mut self, up: bool, viewport_rows: f64) {
        let primary = self.primary();
        let line = self.text.char_to_line(primary.caret);
        let target = if up { line.checked_sub(1) } else { Some(line + 1).filter(|&l| l < self.text.len_lines()) };
        let Some(target) = target else { return };
        let previous = self.carets.len().checked_sub(2).map(|i| self.carets[i]);
        if primary.goal.is_some() && previous.is_some_and(|p| self.text.char_to_line(p.caret) == target) {
            self.carets.pop();
        } else {
            let mut clone = CaretSelection::at(primary.caret);
            clone.goal = primary.goal;
            self.move_vertically(&mut clone, target);
            clone.collapse();
            self.carets.push(clone);
            self.merge_carets();
        }
        self.reveal_caret(viewport_rows);
    }

    /// Esc: keeps only the primary caret, with its selection.
    pub(crate) fn collapse_carets(&mut self, viewport_rows: f64) {
        self.keep_only(self.primary());
        self.reveal_caret(viewport_rows);
    }

    /// Selects the word touching the primary caret, if any, and starts a
    /// whole-word occurrence search.
    fn select_word_at_primary(&mut self) {
        let caret = self.primary().caret;
        let is_word = |i: usize| text::CharClass::of(self.text.char(i)) == text::CharClass::Word;
        let mut start = caret;
        while start > 0 && is_word(start - 1) {
            start -= 1;
        }
        let mut end = caret;
        while end < self.text.len_chars() && is_word(end) {
            end += 1;
        }
        if start < end {
            *self.carets.last_mut().expect("an editor always has a caret") = CaretSelection::selecting(start, end);
            self.keep_word_search(true);
        }
    }

    /// Whether the carets are still those a whole-word occurrence search
    /// left, with the text unchanged.
    fn word_search_on(&self) -> bool {
        self.whole_words.as_ref().is_some_and(|(version, cursors)| *version == self.version && *cursors == self.carets)
    }

    /// Remembers the carets an occurrence command left, so the next one
    /// knows whether a whole-word search is still going.
    fn keep_word_search(&mut self, whole_words: bool) {
        self.whole_words = whole_words.then(|| (self.version, self.carets.clone()));
    }

    /// Every occurrence of the primary selection's text, in order.
    fn occurrences(&self, whole_words: bool) -> Vec<Range<usize>> {
        let needle = self.text.slice(self.primary().range()).to_string();
        if needle.is_empty() {
            return Vec::new();
        }
        let len = needle.chars().count();
        let is_word = |i: usize| text::CharClass::of(self.text.char(i)) == text::CharClass::Word;
        let bounded = |m: &Range<usize>| {
            (m.start == 0 || !is_word(m.start - 1)) && (m.end == self.text.len_chars() || !is_word(m.end))
        };
        self.text
            .to_string()
            .match_indices(&needle)
            .map(|(byte, _)| {
                let start = self.text.byte_to_char(byte);
                start..start + len
            })
            .filter(|m| !whole_words || bounded(m))
            .collect()
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
        self.keep_only(CaretSelection::selecting(line_start + start, line_start + end));
        self.reveal_caret(viewport_rows);
    }

    /// Selects a whole line and its line break, with the caret at the start
    /// of the next line.
    pub(crate) fn select_line(&mut self, line: usize, viewport_rows: f64) {
        let line = line.min(self.text.len_lines() - 1);
        let start = self.text.line_to_char(line);
        self.keep_only(CaretSelection::selecting(start, start + self.text.line(line).len_chars()));
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

    /// Inserts `text` at every caret, replacing the selections, and puts
    /// each caret after it. Line breaks become the file's line ending. `now`
    /// is the host clock's time, for grouping undo steps.
    pub(crate) fn insert(&mut self, text: &str, kind: EditKind, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only || (text.is_empty() && self.carets.iter().all(|c| c.is_empty())) {
            return;
        }
        let text = self.line_ending.normalize(text);
        let edits = (0..self.carets.len()).map(|i| (i, self.carets[i].range(), text.clone())).collect();
        self.replace(edits, kind, now, viewport_rows);
    }

    /// ⌘V: like `insert`, except that text with as many lines as there are
    /// carets (two or more) puts one line at each caret, top to bottom.
    pub(crate) fn paste(&mut self, text: &str, now: Instant, viewport_rows: f64) {
        let text = LineEnding::Lf.normalize(text);
        let lines: Vec<&str> = text.strip_suffix('\n').unwrap_or(&text).split('\n').collect();
        if self.carets.len() < 2 || lines.len() != self.carets.len() {
            return self.insert(&text, EditKind::Other, now, viewport_rows);
        }
        self.preedit.clear();
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| self.carets[i].range().start);
        let edits = order.into_iter().zip(lines).map(|(i, line)| (i, self.carets[i].range(), line.to_owned())).collect();
        self.replace(edits, EditKind::Other, now, viewport_rows);
    }

    pub(crate) fn set_preedit(&mut self, text: String) {
        if self.read_only {
            return;
        }
        self.preedit = text;
    }

    /// Deletes each caret's selection, or the text its movement would pass
    /// over.
    pub(crate) fn delete(&mut self, movement: CaretMove, kind: EditKind, now: Instant, viewport_rows: f64) {
        if self.read_only {
            return;
        }
        let edits = (0..self.carets.len())
            .map(|i| {
                let mut cursor = self.carets[i];
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
        if self.read_only {
            return;
        }
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
        self.carets = carets.into_iter().map(|(_, caret)| CaretSelection::at(caret)).collect();
        self.merge_carets();
        self.reveal_caret(rows);
        self.record(kind, now, changes, before, version_before);
    }

    /// Merges carets whose selections overlap, or that sit at the same
    /// place, keeping the later-added one's direction and order.
    fn merge_carets(&mut self) {
        if self.carets.len() < 2 {
            return;
        }
        let mut order: Vec<usize> = (0..self.carets.len()).collect();
        order.sort_by_key(|&i| (self.carets[i].range().start, self.carets[i].range().end));
        let mut groups: Vec<(Range<usize>, usize)> = Vec::with_capacity(order.len());
        for i in order {
            let range = self.carets[i].range();
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
        if groups.len() == self.carets.len() {
            return;
        }
        groups.sort_by_key(|(_, kept)| *kept);
        self.carets = groups
            .into_iter()
            .map(|(range, kept)| {
                let cursor = self.carets[kept];
                if cursor.range() == range {
                    cursor
                } else if cursor.caret < cursor.anchor {
                    CaretSelection::selecting(range.end, range.start)
                } else {
                    CaretSelection::selecting(range.start, range.end)
                }
            })
            .collect();
    }

    /// ⌘Z: reverts the last undo step and restores the carets from before
    /// it.
    pub(crate) fn undo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        let mut history = std::mem::take(&mut self.history);
        if let Some(restored) = history.undo(|change| self.apply(change)) {
            self.restore(restored.selections, restored.version, viewport_rows);
        }
        self.history = history;
    }

    /// ⌘⇧Z: makes the last undone step again and restores the carets from
    /// after it.
    pub(crate) fn redo(&mut self, viewport_rows: f64) {
        self.preedit.clear();
        let mut history = std::mem::take(&mut self.history);
        if let Some(restored) = history.redo(|change| self.apply(change)) {
            self.restore(restored.selections, restored.version, viewport_rows);
        }
        self.history = history;
    }

    fn restore(&mut self, selections: Vec<Selection>, version: u64, viewport_rows: f64) {
        self.carets = selections.into_iter().map(|s| CaretSelection::selecting(s.anchor, s.caret)).collect();
        self.version = version;
        self.reveal_caret(viewport_rows);
    }

    fn new_version(&mut self) {
        self.last_version += 1;
        self.version = self.last_version;
    }

    fn selections(&self) -> Vec<Selection> {
        self.carets.iter().map(|c| c.selection()).collect()
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

    /// Replaces a char range of the text. Every change to the text goes
    /// through here (`replace`, undo and redo, via `apply`): it moves the
    /// syntax tree and highlights with the text.
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

    /// Applies a change to the text: an edit's, or an undo or redo's.
    fn apply(&mut self, change: &Change) {
        match change {
            Change::Insert { at, text } => self.splice(*at..*at, text),
            Change::Remove { at, text } => self.splice(*at..*at + text.chars().count(), ""),
        }
    }

    /// The selected text, or `None` with nothing selected. Several
    /// selections are joined top to bottom, one per line.
    pub(crate) fn selected_text(&self) -> Option<String> {
        let mut ranges: Vec<Range<usize>> =
            self.carets.iter().map(|c| c.range()).filter(|r| !r.is_empty()).collect();
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

    fn move_vertically(&self, cursor: &mut CaretSelection, target_line: usize) {
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

    /// The display column of a char column in `line`, both clamped to the
    /// text (problems and other positions from outside the editor).
    pub(crate) fn grid_column(&self, line: usize, char_column: usize) -> usize {
        let line = line.min(self.text.len_lines() - 1);
        display_columns(self.text.line(line).chars().take(char_column.min(self.line_len(line))))
    }

    pub(crate) fn view(&self, viewport_rows: f64) -> EditorView {
        let line_count = self.text.len_lines();
        let first = (self.scroll_top.floor() as usize).min(line_count);
        let end = ((self.scroll_top + viewport_rows).ceil() as usize).min(line_count);
        let mut ranges: Vec<Range<usize>> =
            self.carets.iter().map(|c| c.range()).filter(|r| !r.is_empty()).collect();
        ranges.sort_by_key(|r| r.start);
        let lines = (first..end)
            .map(|index| {
                let (text, highlights) = self.grid_line(index);
                VisibleLine { index, text, selections: self.selected_columns(index, &ranges), highlights }
            })
            .collect();
        let caret = self.caret_at(self.primary().caret);
        let mut positions: Vec<usize> = self.carets.iter().map(|c| c.caret).collect();
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
            read_only: self.read_only,
            loading: self.loading.is_some(),
            modified: self.version != self.saved_version,
            line_count,
            scroll_top: self.scroll_top,
            lines,
            caret,
            carets,
            preedit,
            problems: Vec::new(),
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
    /// primary caret (and left plain). Also its highlight spans, in display columns.
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
        let (caret_line, caret_column) = self.line_column(self.primary().caret);
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

/// One tab's place in a file: where its carets and selections are and how
/// far it is scrolled (ticket #31). An open file has one `Editor` however
/// many tabs show it, so edits show in every tab; each tab keeps its own
/// cursor, and the project swaps it in while that tab is focused or drawn.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Cursor {
    /// As in `Editor::carets`; empty means one caret at the start.
    carets: Vec<CaretSelection>,
    scroll_top: f64,
    preedit: String,
    whole_words: Option<(u64, Vec<CaretSelection>)>,
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
            carets: self.carets.clone(),
            scroll_top: self.scroll_top,
            preedit: self.preedit.clone(),
            whole_words: self.whole_words.clone(),
        }
    }

    /// Puts a tab's cursor back. The text may have changed under it in
    /// another tab, so positions are clamped to the text.
    pub(crate) fn set_cursor(&mut self, cursor: &Cursor, viewport_rows: f64) {
        let len = self.text.len_chars();
        self.carets = cursor
            .carets
            .iter()
            .map(|c| CaretSelection { caret: c.caret.min(len), anchor: c.anchor.min(len), goal: c.goal })
            .collect();
        if self.carets.is_empty() {
            self.carets.push(CaretSelection::at(0));
        }
        self.merge_carets();
        self.scroll_top = cursor.scroll_top;
        self.preedit.clone_from(&cursor.preedit);
        self.whole_words.clone_from(&cursor.whole_words);
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
