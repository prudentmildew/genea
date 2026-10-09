//! Plain-text helpers for the editor: line endings and word boundaries.

use ropey::Rope;

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
