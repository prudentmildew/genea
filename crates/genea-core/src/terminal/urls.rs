//! A script's dev-server link (ticket #40): the first `http(s)://` URL on
//! `localhost`, `127.0.0.1` or `[::1]` in its output.
//!
//! The output is read on the tab's reader thread, through a parser of its
//! own that keeps only the printed text (colours and other escapes split
//! nothing: Vite prints its port in bold), a line at a time, so a URL split
//! across reads is still found once its line ends.

use alacritty_terminal::vte::{Params, Parser, Perform};

/// The hosts a dev server is reached on.
const LOCAL_HOSTS: [&str; 3] = ["localhost", "127.0.0.1", "[::1]"];

/// Longest line kept; anything beyond is dropped.
const MAX_LINE: usize = 4096;

/// Finds the first local URL in a program's output.
pub(super) struct UrlFinder {
    parser: Parser,
    lines: Lines,
}

#[derive(Default)]
struct Lines {
    /// The text printed since the last line end.
    line: String,
    found: Option<String>,
}

impl UrlFinder {
    pub(super) fn new() -> Self {
        UrlFinder { parser: Parser::new(), lines: Lines::default() }
    }

    /// Reads more output; returns the first local URL once a line with one
    /// has ended.
    pub(super) fn advance(&mut self, bytes: &[u8]) -> Option<String> {
        self.parser.advance(&mut self.lines, bytes);
        self.lines.found.take()
    }
}

impl Lines {
    fn end_line(&mut self) {
        if self.found.is_none() {
            self.found = local_url(&self.line);
        }
        self.line.clear();
    }
}

impl Perform for Lines {
    fn print(&mut self, c: char) {
        if self.line.len() < MAX_LINE {
            self.line.push(c);
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            b'\n' | b'\r' => self.end_line(),
            b'\t' => self.print(' '),
            _ => {}
        }
    }

    /// Colours (SGR) leave the line as it is; anything else (cursor moves,
    /// erasing) ends it.
    fn csi_dispatch(&mut self, _: &Params, _: &[u8], _: bool, action: char) {
        if action != 'm' {
            self.end_line();
        }
    }
}

/// The first local `http(s)://` URL in `text`, without trailing
/// punctuation.
fn local_url(text: &str) -> Option<String> {
    let mut from = 0;
    while let Some(found) = text[from..].find("http") {
        let start = from + found;
        from = start + 4;
        let rest = &text[start..];
        let Some(after_scheme) = rest.strip_prefix("http://").or_else(|| rest.strip_prefix("https://")) else {
            continue;
        };
        // Not part of a longer word (`xhttp://`).
        if text[..start].chars().next_back().is_some_and(|c| c.is_alphanumeric()) {
            continue;
        }
        let Some(host) = LOCAL_HOSTS.iter().find(|host| {
            after_scheme.strip_prefix(**host).is_some_and(|after| after.chars().next().is_none_or(ends_host))
        }) else {
            continue;
        };
        let scheme = rest.len() - after_scheme.len();
        let length = rest.find(|c: char| c.is_whitespace() || "\"'<>`".contains(c)).unwrap_or(rest.len());
        let mut url = &rest[..length];
        let minimum = scheme + host.len();
        while url.len() > minimum && url.ends_with(|c: char| ".,;:!?)]}'\"".contains(c)) {
            url = &url[..url.len() - 1];
        }
        return Some(url.to_owned());
    }
    None
}

/// Whether `c` may follow a host name in a URL.
fn ends_host(c: char) -> bool {
    matches!(c, ':' | '/' | '?' | '#') || !(c.is_alphanumeric() || c == '-' || c == '.' || c == '_')
}
