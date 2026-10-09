//! `.env` highlighting. There is no tree-sitter grammar for `.env` files,
//! and the format is one `KEY=value` per line, so a line scanner does.

use super::highlight::{Highlight, Paint};

/// Paints `# comments`, `export`, keys, `=` and values.
pub(super) fn paint(text: &[u8], paint: &mut [Paint]) {
    let mut line_start = 0;
    for line in text.split_inclusive(|&b| b == b'\n') {
        paint_line(line, &mut paint[line_start..line_start + line.len()]);
        line_start += line.len();
    }
}

fn paint_line(line: &[u8], paint: &mut [Paint]) {
    let end = line.iter().rposition(|b| !matches!(b, b'\n' | b'\r')).map_or(0, |i| i + 1);
    let mut i = line.iter().position(|b| !b.is_ascii_whitespace()).unwrap_or(end).min(end);
    if i == end {
        return;
    }
    if line[i] == b'#' {
        paint[i..end].fill(Highlight::Comment as Paint);
        return;
    }
    if line[i..end].starts_with(b"export") && line.get(i + 6).is_some_and(|b| *b == b' ' || *b == b'\t') {
        paint[i..i + 6].fill(Highlight::Keyword as Paint);
        i += 6;
        while i < end && line[i].is_ascii_whitespace() {
            i += 1;
        }
    }
    let Some(equals) = line[i..end].iter().position(|&b| b == b'=').map(|n| i + n) else { return };
    paint[i..equals].fill(Highlight::Property as Paint);
    paint[equals] = Highlight::Operator as Paint;
    let value = equals + 1;
    // An unquoted value ends at ` #`, which starts a comment.
    let comment = (line[value..end].first() != Some(&b'"') && line[value..end].first() != Some(&b'\''))
        .then(|| line[value..end].windows(2).position(|w| w[0].is_ascii_whitespace() && w[1] == b'#'))
        .flatten()
        .map(|n| value + n + 1);
    let value_end = comment.map_or(end, |comment| {
        line[value..comment].iter().rposition(|b| !b.is_ascii_whitespace()).map_or(value, |n| value + n + 1)
    });
    paint[value..value_end].fill(Highlight::String as Paint);
    if let Some(comment) = comment {
        paint[comment..end].fill(Highlight::Comment as Paint);
    }
}
