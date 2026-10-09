//! What keys and the mouse send to the program: xterm's encodings, which
//! depend on the modes the program has set (application cursor keys, mouse
//! reporting, …).

use alacritty_terminal::term::TermMode;

use crate::command::{Modifiers, MouseAction, MouseButton, TerminalKey};

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

/// The report of a mouse action, if the program's mouse mode reports it.
pub(super) fn mouse(action: MouseAction, line: usize, column: usize, m: Modifiers, mode: TermMode) -> Vec<u8> {
    let reported = match action {
        MouseAction::Press(_) | MouseAction::Release(_) => mode.intersects(TermMode::MOUSE_MODE),
        MouseAction::Drag(_) => mode.intersects(TermMode::MOUSE_DRAG | TermMode::MOUSE_MOTION),
        MouseAction::Move => mode.contains(TermMode::MOUSE_MOTION),
    };
    if !reported {
        return Vec::new();
    }
    let button = |button: MouseButton| match button {
        MouseButton::Left => 0,
        MouseButton::Middle => 1,
        MouseButton::Right => 2,
    };
    let sgr = mode.contains(TermMode::SGR_MOUSE);
    let code = match action {
        MouseAction::Press(b) => button(b),
        // The original encoding can't say which button was released.
        MouseAction::Release(b) => if sgr { button(b) } else { 3 },
        MouseAction::Drag(b) => 32 + button(b),
        MouseAction::Move => 32 + 3,
    };
    let code = code + 4 * u32::from(m.shift) + 8 * u32::from(m.alt) + 16 * u32::from(m.ctrl);
    report(code, line, column, matches!(action, MouseAction::Release(_)), mode)
}

/// The report of one step of the scroll wheel, if the program takes the
/// mouse.
pub(super) fn wheel(up: bool, line: usize, column: usize, mode: TermMode) -> Vec<u8> {
    if !mode.intersects(TermMode::MOUSE_MODE) {
        return Vec::new();
    }
    report(if up { 64 } else { 65 }, line, column, false, mode)
}

/// A mouse report in the program's encoding: SGR, or xterm's original
/// (optionally with UTF-8 positions).
fn report(code: u32, line: usize, column: usize, release: bool, mode: TermMode) -> Vec<u8> {
    let (x, y) = (column + 1, line + 1);
    if mode.contains(TermMode::SGR_MOUSE) {
        let end = if release { 'm' } else { 'M' };
        return format!("\x1b[<{code};{x};{y}{end}").into_bytes();
    }
    let mut report = b"\x1b[M".to_vec();
    for value in [code as usize, x, y] {
        let value = 32 + value;
        if mode.contains(TermMode::UTF8_MOUSE) && value < 2048 {
            let mut buffer = [0; 4];
            report.extend_from_slice(char::from_u32(value as u32).unwrap_or(' ').encode_utf8(&mut buffer).as_bytes());
        } else if value <= 255 {
            report.push(value as u8);
        } else {
            // Past column 223 the original encoding has no way to say it.
            return Vec::new();
        }
    }
    report
}
