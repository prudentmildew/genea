//! Translating between Genea's files and text and LSP's: `file:` URIs,
//! language ids, and positions in the encoding the server chose.

use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::problems::TextPosition;

/// How a server counts columns: it picks one of the encodings the client
/// offers in `general.positionEncodings` (Genea offers UTF-8 first, which
/// tsgo takes), or UTF-16, the default every server supports.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Encoding {
    Utf8,
    #[default]
    Utf16,
    Utf32,
}

impl Encoding {
    /// From `capabilities.positionEncoding` in the initialize result.
    pub(crate) fn from_lsp(kind: Option<&str>) -> Self {
        match kind {
            Some("utf-8") => Encoding::Utf8,
            Some("utf-32") => Encoding::Utf32,
            _ => Encoding::Utf16,
        }
    }

    fn units(self, c: char) -> usize {
        match self {
            Encoding::Utf8 => c.len_utf8(),
            Encoding::Utf16 => c.len_utf16(),
            Encoding::Utf32 => 1,
        }
    }
}

/// The LSP language id of a first-class-language file, or `None` for any
/// other file (those are never sent to tsgo).
pub(crate) fn language_id(path: &Path) -> Option<&'static str> {
    match path.extension()?.to_str()? {
        "ts" | "mts" | "cts" => Some("typescript"),
        "tsx" => Some("typescriptreact"),
        "js" | "mjs" | "cjs" => Some("javascript"),
        "jsx" => Some("javascriptreact"),
        _ => None,
    }
}

/// The `file:` URI of an absolute path, percent-encoding everything but
/// unreserved characters and `/`.
pub(crate) fn uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for byte in path.as_os_str().as_encoded_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => uri.push(*byte as char),
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

/// The absolute path of a `file:` URI, or `None` for another scheme.
pub(crate) fn path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    // `file://localhost/…` and `file:///…` both name a local file.
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(byte) = rest.get(i + 1..i + 3).and_then(|hex| u8::from_str_radix(hex, 16).ok())
        {
            decoded.push(byte);
            i += 3;
        } else {
            decoded.push(bytes[i]);
            i += 1;
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(decoded)))
}

/// A position from the server (line, column in `encoding` units) as a
/// [`TextPosition`] in `text`. A line past the end, or a column past the
/// end of its line, is clamped.
pub(crate) fn text_position(text: &Rope, line: u32, character: u32, encoding: Encoding) -> TextPosition {
    let last = text.len_lines().saturating_sub(1);
    let line = (line as usize).min(last);
    let mut units = 0;
    let mut column = 0;
    for c in text.line(line).chars() {
        if c == '\n' || c == '\r' || units >= character as usize {
            break;
        }
        units += encoding.units(c);
        column += 1;
    }
    TextPosition { line, column }
}

/// A char index into `text` as an LSP position (line, column in
/// `encoding` units). An index past the end is clamped.
pub(crate) fn lsp_position(text: &Rope, char: usize, encoding: Encoding) -> (u32, u32) {
    let char = char.min(text.len_chars());
    let line = text.char_to_line(char);
    let start = text.line_to_char(line);
    let units: usize = text.slice(start..char).chars().map(|c| encoding.units(c)).sum();
    (line as u32, units as u32)
}

/// A position from the server as a char index into `text`, clamped like
/// [`text_position`].
pub(crate) fn char_index(text: &Rope, line: u32, character: u32, encoding: Encoding) -> usize {
    let position = text_position(text, line, character, encoding);
    text.line_to_char(position.line) + position.column
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_with_spaces_and_unicode() {
        let path = Path::new("/Users/me/My Projects/ærlig/src/a b.ts");
        let uri = uri(path);
        assert_eq!(uri, "file:///Users/me/My%20Projects/%C3%A6rlig/src/a%20b.ts");
        assert_eq!(super::path(&uri).unwrap(), path);
        assert_eq!(super::path("file://localhost/tmp/x").unwrap(), Path::new("/tmp/x"));
        assert_eq!(super::path("untitled:1"), None);
    }

    #[test]
    fn columns_count_in_the_servers_encoding() {
        let text = Rope::from_str("é😀x\r\nnext");
        // `x` is at UTF-8 byte 6, UTF-16 unit 3, char 2.
        assert_eq!(text_position(&text, 0, 6, Encoding::Utf8), TextPosition { line: 0, column: 2 });
        assert_eq!(text_position(&text, 0, 3, Encoding::Utf16), TextPosition { line: 0, column: 2 });
        assert_eq!(text_position(&text, 0, 2, Encoding::Utf32), TextPosition { line: 0, column: 2 });
        // Past the end of the line, and past the last line.
        assert_eq!(text_position(&text, 0, 99, Encoding::Utf16), TextPosition { line: 0, column: 3 });
        assert_eq!(text_position(&text, 7, 1, Encoding::Utf16), TextPosition { line: 1, column: 1 });
    }

    #[test]
    fn char_indices_go_to_the_servers_positions_and_back() {
        let text = Rope::from_str("ab\né😀x\n");
        // `x` is char 6: line 1, after `é` (2 bytes, 1 unit) and `😀` (4 bytes, 2 units).
        assert_eq!(lsp_position(&text, 6, Encoding::Utf8), (1, 6));
        assert_eq!(lsp_position(&text, 6, Encoding::Utf16), (1, 3));
        assert_eq!(lsp_position(&text, 6, Encoding::Utf32), (1, 2));
        assert_eq!(char_index(&text, 1, 6, Encoding::Utf8), 6);
        assert_eq!(char_index(&text, 1, 3, Encoding::Utf16), 6);
        assert_eq!(lsp_position(&text, 99, Encoding::Utf16), (2, 0));
    }

    #[test]
    fn only_first_class_languages_have_a_language_id() {
        assert_eq!(language_id(Path::new("a.tsx")), Some("typescriptreact"));
        assert_eq!(language_id(Path::new("types.d.ts")), Some("typescript"));
        assert_eq!(language_id(Path::new("a.mjs")), Some("javascript"));
        assert_eq!(language_id(Path::new("package.json")), None);
        assert_eq!(language_id(Path::new("Makefile")), None);
    }
}
