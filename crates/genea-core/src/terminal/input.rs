//! What keys send to the program: xterm's encodings, which depend on the
//! modes the program has set (application cursor keys, …).

use alacritty_terminal::term::TermMode;

use crate::command::{Modifiers, TerminalKey};

/// The bytes a key press sends.
pub(super) fn key(key: TerminalKey, m: Modifiers, mode: TermMode) -> Vec<u8> {
    // xterm's modifier parameter: 1 + shift + 2·alt + 4·ctrl.
    let param = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.ctrl);
    let modified = param > 1;
    let cursor = |c: char| -> Vec<u8> {
        if modified {
            format!("\x1b[1;{param}{c}").into_bytes()
        } else if mode.contains(TermMode::APP_CURSOR) {
            format!("\x1bO{c}").into_bytes()
        } else {
            format!("\x1b[{c}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modified { format!("\x1b[{n};{param}~").into_bytes() } else { format!("\x1b[{n}~").into_bytes() }
    };
    let only_alt = m.alt && !m.shift && !m.ctrl;
    match key {
        TerminalKey::Enter => with_alt(m.alt, b"\r".to_vec()),
        TerminalKey::Backspace if m.ctrl => b"\x08".to_vec(),
        TerminalKey::Backspace => with_alt(m.alt, b"\x7f".to_vec()),
        TerminalKey::Tab if m.shift => b"\x1b[Z".to_vec(),
        TerminalKey::Tab => with_alt(m.alt, b"\t".to_vec()),
        TerminalKey::Escape => with_alt(m.alt, b"\x1b".to_vec()),
        // ⌥← and ⌥→ move by word, as in the macOS terminals.
        TerminalKey::Left if only_alt => b"\x1bb".to_vec(),
        TerminalKey::Right if only_alt => b"\x1bf".to_vec(),
        TerminalKey::Up => cursor('A'),
        TerminalKey::Down => cursor('B'),
        TerminalKey::Right => cursor('C'),
        TerminalKey::Left => cursor('D'),
        TerminalKey::Home => cursor('H'),
        TerminalKey::End => cursor('F'),
        TerminalKey::Insert => tilde(2),
        TerminalKey::Delete => tilde(3),
        TerminalKey::PageUp => tilde(5),
        TerminalKey::PageDown => tilde(6),
        TerminalKey::F(n @ 1..=4) => {
            let c = char::from(b'P' + n - 1);
            if modified { format!("\x1b[1;{param}{c}").into_bytes() } else { format!("\x1bO{c}").into_bytes() }
        }
        TerminalKey::F(n) => match n {
            5 => tilde(15),
            6..=10 => tilde(n + 11),
            11 | 12 => tilde(n + 12),
            _ => Vec::new(),
        },
        TerminalKey::Char(c) => {
            let bytes = if m.ctrl { control(c) } else { c.to_string().into_bytes() };
            with_alt(m.alt, bytes)
        }
    }
}

/// ⌥ as Meta: the key's bytes after an ESC.
fn with_alt(alt: bool, bytes: Vec<u8>) -> Vec<u8> {
    if alt { [b"\x1b".as_slice(), &bytes].concat() } else { bytes }
}

/// ⌃ with a character: its control code, where it has one.
fn control(c: char) -> Vec<u8> {
    let code = match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | ' ' | '2' => 0,
        '[' | '3' => 27,
        '\\' | '4' => 28,
        ']' | '5' => 29,
        '^' | '6' => 30,
        '_' | '-' | '7' => 31,
        '?' | '8' => 127,
        _ => return c.to_string().into_bytes(),
    };
    vec![code]
}
