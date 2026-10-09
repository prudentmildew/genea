//! `path:line:col` references in terminal output (ticket #39), as
//! compilers, linters and test runners print them: `src/app.ts:12:5`,
//! `./src/app.ts:12`, `/abs/app.ts:3:1`, `file:///abs/app.ts:3:1`.
//!
//! A reference is a path whose last part has an extension (`app.ts`,
//! `.env`), then `:line` and optionally `:column`, both 1-based. That
//! leaves out URLs (`http://localhost:3000`), addresses (`127.0.0.1:80`)
//! and times (`12:30:45`). Paths are found as printed; the tab resolves
//! them against its directory when one is clicked.

use std::{ops::Range, path::PathBuf};

use crate::problems::TextPosition;

/// A reference found in a row's text.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct Found {
    /// Its bytes in the text, path to the last number.
    pub(super) bytes: Range<usize>,
    pub(super) path: PathBuf,
    /// 0-based, from the printed 1-based line and column (column 1 when
    /// none is printed).
    pub(super) at: TextPosition,
}

const FILE_URL: &str = "file:";

/// Every reference in `text`, left to right.
pub(super) fn find(text: &str) -> Vec<Found> {
    let mut found = Vec::new();
    let mut rest = 0;
    while let Some(offset) = text[rest..].find(is_path_char) {
        let start = rest + offset;
        let end = text[start..].find(|c| !is_path_char(c)).map_or(text.len(), |n| start + n);
        rest = end;
        let path = &text[start..end];
        let before = &text[..start];
        // `file:///abs/app.ts:3` (the slashes are path characters), but no
        // other URL's path.
        let (start, path) = match before.strip_suffix(FILE_URL) {
            Some(prefix) if path.starts_with("///") => (prefix.len(), &path[2..]),
            _ if before.ends_with(':') => continue,
            _ => (start, path),
        };
        if !has_extension(path) {
            continue;
        }
        let Some((line, after_line)) = number_after_colon(text, end) else { continue };
        let (column, end) = number_after_colon(text, after_line).map_or((1, after_line), |(c, end)| (c, end));
        if line == 0 || column == 0 {
            continue;
        }
        rest = end;
        found.push(Found {
            bytes: start..end,
            path: PathBuf::from(path),
            at: TextPosition { line: line - 1, column: column - 1 },
        });
    }
    found
}

fn is_path_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-' | '@' | '+' | '~')
}

/// Whether the path's last part has an extension: a dot, then letters or
/// digits starting with a letter.
fn has_extension(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path);
    let Some((_, extension)) = name.rsplit_once('.') else { return false };
    extension.starts_with(|c: char| c.is_ascii_alphabetic()) && extension.chars().all(|c| c.is_ascii_alphanumeric())
}

/// `:N` at `at`: the number and the byte after it.
fn number_after_colon(text: &str, at: usize) -> Option<(usize, usize)> {
    let digits = text[at..].strip_prefix(':')?;
    let length = digits.find(|c: char| !c.is_ascii_digit()).unwrap_or(digits.len());
    let number = digits[..length].parse().ok()?;
    Some((number, at + 1 + length))
}
