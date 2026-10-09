//! Structural editing (ticket #25): auto-indent on Return, matching
//! brackets, expand and shrink selection, line comments and folding. The
//! syntax tree (`syntax::structure`) answers the questions; this turns its
//! answers into edits, carets and view state.

use std::time::Instant;

use super::Editor;
use crate::history::EditKind;

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
}
