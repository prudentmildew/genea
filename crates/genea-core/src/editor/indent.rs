//! Tab and ⇧Tab (ticket #26), with the file's resolved [`Indentation`].

use std::{collections::BTreeSet, time::Instant};

use super::Editor;
use crate::{history::EditKind, indentation::Indentation};

impl Editor {
    /// Tab. With nothing selected, types indentation at every caret: a tab,
    /// or spaces up to the next multiple of the indentation's width. With a
    /// selection, indents every line the carets and selections are on (but
    /// empty ones) by one level, keeping the selections on their text.
    pub(crate) fn indent(&mut self, indentation: Indentation, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        if self.read_only {
            return;
        }
        if self.carets.iter().all(|c| c.is_empty()) {
            let edits = (0..self.carets.len())
                .map(|i| {
                    let caret = self.carets[i].caret;
                    let text = if indentation.use_tabs {
                        "\t".to_owned()
                    } else {
                        let width = indentation.tab_width;
                        " ".repeat(width - self.display_column(caret) % width)
                    };
                    (i, caret..caret, text)
                })
                .collect();
            self.replace(edits, EditKind::Typing, now, viewport_rows);
        } else {
            let unit = indentation.unit();
            let edits = self
                .caret_lines()
                .into_iter()
                .filter(|&line| self.line_len(line) > 0)
                .map(|line| {
                    let start = self.text.line_to_char(line);
                    (start..start, unit.clone())
                })
                .collect();
            self.edit_text(edits, EditKind::Other, now, viewport_rows);
        }
    }

    /// ⇧Tab: takes one level of indentation off every line the carets and
    /// selections are on: a leading tab, or leading spaces back to the
    /// previous multiple of the indentation's width. Carets and selections
    /// stay on their text.
    pub(crate) fn outdent(&mut self, indentation: Indentation, now: Instant, viewport_rows: f64) {
        self.preedit.clear();
        let edits = self
            .caret_lines()
            .into_iter()
            .filter_map(|line| {
                let chars = self.line_chars(line);
                let removed = match chars.first() {
                    Some('\t') => 1,
                    _ => match chars.iter().take_while(|&&c| c == ' ').count() {
                        0 => return None,
                        spaces => (spaces - 1) % indentation.tab_width + 1,
                    },
                };
                let start = self.text.line_to_char(line);
                Some((start..start + removed, String::new()))
            })
            .collect();
        self.edit_text(edits, EditKind::Other, now, viewport_rows);
    }

    /// The lines the carets and selections are on. A selection that ends at
    /// the start of a line leaves that line out.
    pub(super) fn caret_lines(&self) -> BTreeSet<usize> {
        let mut lines = BTreeSet::new();
        for caret in &self.carets {
            let range = caret.range();
            let first = self.text.char_to_line(range.start);
            let mut last = self.text.char_to_line(range.end);
            if last > first && range.end == self.text.line_to_char(last) {
                last -= 1;
            }
            lines.extend(first..=last);
        }
        lines
    }
}
