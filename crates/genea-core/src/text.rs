//! Plain-text helpers for the editor: decoding files, line endings and word
//! boundaries.

use ropey::Rope;

/// A file's contents as the editor gets them.
pub(crate) enum Decoded {
    /// Valid UTF-8: editable.
    Text(String),
    /// Not valid UTF-8 (spec #19: UTF-8 only). Each invalid sequence is a
    /// U+FFFD replacement character, so the file can be read but not saved.
    Invalid(String),
    /// Not text at all: the editor doesn't show it.
    Binary,
}

/// How far into a file to look for a NUL byte, the sign of a binary file
/// (git's heuristic). Text in UTF-8 never has one.
const BINARY_SNIFF_BYTES: usize = 8000;

impl Decoded {
    pub(crate) fn from_bytes(bytes: Vec<u8>) -> Self {
        if bytes[..bytes.len().min(BINARY_SNIFF_BYTES)].contains(&0) {
            return Decoded::Binary;
        }
        match String::from_utf8(bytes) {
            Ok(text) => Decoded::Text(text),
            Err(error) => Decoded::Invalid(String::from_utf8_lossy(error.as_bytes()).into_owned()),
        }
    }
}

/// A file's line ending. Detected when the file is read and used for every
/// line break Genea inserts, so a file keeps its line endings when saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LineEnding {
    Lf,
    CrLf,
}

impl LineEnding {
    /// The file's first line break decides; a file with none is LF.
    pub(crate) fn detect(text: &Rope) -> Self {
        let mut previous = None;
        for c in text.chars() {
            match c {
                '\n' if previous == Some('\r') => return LineEnding::CrLf,
                '\n' => return LineEnding::Lf,
                _ => previous = Some(c),
            }
        }
        LineEnding::Lf
    }

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
        }
    }

    /// The status bar's name for it.
    pub(crate) fn label(self) -> &'static str {
        match self {
            LineEnding::Lf => "LF",
            LineEnding::CrLf => "CRLF",
        }
    }

    /// `text` with every line break (LF, CRLF or a lone CR) made this one.
    pub(crate) fn normalize(self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '\r' => {
                    chars.next_if_eq(&'\n');
                    out.push_str(self.as_str());
                }
                '\n' => out.push_str(self.as_str()),
                c => out.push(c),
            }
        }
        out
    }
}

/// What kind of character, for word movement: a word is a run of one class.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CharClass {
    Space,
    /// Letters, numbers, `_` and `$` (an identifier in TypeScript).
    Word,
    Punctuation,
}

impl CharClass {
    pub(crate) fn of(c: char) -> Self {
        if c.is_whitespace() {
            CharClass::Space
        } else if c.is_alphanumeric() || c == '_' || c == '$' {
            CharClass::Word
        } else {
            CharClass::Punctuation
        }
    }
}

/// Where ⌥→ stops from `column` in a line's `chars` (no line ending): past
/// any spaces, then to the end of the run that follows.
pub(crate) fn word_end(chars: &[char], column: usize) -> usize {
    let mut i = column;
    while i < chars.len() && CharClass::of(chars[i]) == CharClass::Space {
        i += 1;
    }
    let Some(&first) = chars.get(i) else { return i };
    let class = CharClass::of(first);
    while i < chars.len() && CharClass::of(chars[i]) == class {
        i += 1;
    }
    i
}

/// Where ⌥← stops from `column`: back past any spaces, then to the start of
/// the run before them.
pub(crate) fn word_start(chars: &[char], column: usize) -> usize {
    let mut i = column;
    while i > 0 && CharClass::of(chars[i - 1]) == CharClass::Space {
        i -= 1;
    }
    if i == 0 {
        return 0;
    }
    let class = CharClass::of(chars[i - 1]);
    while i > 0 && CharClass::of(chars[i - 1]) == class {
        i -= 1;
    }
    i
}
