//! Structural editing (ticket #25): auto-indent on Return, matching
//! brackets, expand and shrink selection, line comments and folding. The
//! syntax tree (`syntax::structure`) answers the questions; this turns its
//! answers into edits, carets and view state.

use std::{collections::BTreeSet, ops::Range, time::Instant};

use super::{CaretSelection, Editor};
use crate::{
    history::{Change, EditKind},
    syntax::{Comment, Language},
};

/// One level of indentation. Ticket #26 resolves it from `.oxfmtrc.json`
/// and `.editorconfig`; until then it is Oxfmt's default.
const INDENT_UNIT: &str = "  ";

impl Editor {
    /// Return: breaks the line at every caret, indenting the new line like
    /// the caret's line, one level deeper when the caret is just inside a
    /// block (after `{`, `(`, `[` or an element's start tag). When the text
    /// after the caret closes that block, it goes on a line of its own and
    /// the caret stays on the indented line between. Whitespace after the
    /// caret is dropped.
    pub(crate) fn new_line(&mut self, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only {
            return;
        }
        let break_text = self.line_ending.normalize("\n");
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
                    text.push_str(INDENT_UNIT);
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

        let mut rows = BTreeSet::new();
        for caret in &self.carets {
            let range = caret.range();
            let first = self.text.char_to_line(range.start);
            let mut last = self.text.char_to_line(range.end);
            if last > first && range.end == self.text.line_to_char(last) {
                last -= 1;
            }
            rows.extend(first..=last);
        }
        let lines: Vec<(usize, String)> =
            rows.into_iter().map(|line| (line, self.line_chars(line).into_iter().collect())).collect();
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

    /// Makes several changes as one edit, keeping every caret and selection
    /// on the text it was on: each entry replaces a char range (sorted, not
    /// overlapping) of the text as it is now. A caret where text is
    /// inserted moves after it.
    fn edit_text(&mut self, mut edits: Vec<(Range<usize>, String)>, kind: EditKind, now: Instant, rows: f64) {
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
