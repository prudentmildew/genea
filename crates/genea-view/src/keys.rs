//! The keymap: key presses on the editor surface to core commands.
//!
//! WebStorm's macOS keymap (spec #19). Menu shortcuts (⌘S, ⌘X, ⌘C, ⌘V, ⌘A)
//! never get here: the native menu takes them first.

use std::time::{Duration, Instant};

use genea_core::{CaretMove, Command, FinderMode};
use slint::{SharedString, platform::Key};

/// Modifier state of a key press. `cmd` is ⌘.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub cmd: bool,
    pub alt: bool,
    pub ctrl: bool,
}

/// ⌘+ (or ⌘=, where + needs ⇧), ⌘− and ⌘0: zoom (ticket #59), in case
/// the View menu doesn't take them. From the editor and the terminal alike.
pub fn zoom(text: &str, m: Modifiers) -> Option<Command> {
    if !m.cmd || m.ctrl || m.alt {
        return None;
    }
    match text {
        "+" | "=" => Some(Command::ZoomIn),
        "-" | "−" => Some(Command::ZoomOut),
        "0" if !m.shift => Some(Command::ResetZoom),
        _ => None,
    }
}

pub fn command_for(text: &str, m: Modifiers) -> Option<Command> {
    if let Some(command) = zoom(text, m) {
        return Some(command);
    }
    if let Some(movement) = movement(text, m) {
        return Some(if m.shift { Command::Select(movement) } else { Command::MoveCaret(movement) });
    }
    let is = |key: Key| text == SharedString::from(key).as_str();
    // Multi-caret (ticket #52): ⌃G, ⌃⇧G, ⌃⌘G. With ⌃ held, the key's text
    // may be the letter or its control character.
    if m.ctrl && (text.eq_ignore_ascii_case("g") || text == "\u{7}") {
        return Some(if m.cmd {
            Command::SelectAllOccurrences
        } else if m.shift {
            Command::UnselectLastOccurrence
        } else {
            Command::SelectNextOccurrence
        });
    }
    if let Some(command) = structural(text, m) {
        return Some(command);
    }
    // ⌥⏎ and ⌃⌥O (ticket #45), in case the Code menu doesn't take them.
    // With ⌃⌥ held, O may come as `o`, `ø` or its control character.
    if m.ctrl && m.alt && !m.cmd && matches!(text, "o" | "O" | "ø" | "Ø" | "\u{f}") {
        return Some(Command::OrganizeImports);
    }
    if m.alt && !(m.ctrl || m.cmd || m.shift) && is(Key::Return) {
        return Some(Command::ShowQuickFixes);
    }
    if let Some(command) = assist(text, m) {
        return Some(command);
    }
    if m.ctrl || m.cmd {
        return None;
    }
    if is(Key::Escape) {
        return Some(Command::CollapseCarets);
    }
    // Tab and ⇧Tab (ticket #26). ⇧Tab may come as Tab with ⇧ or as Backtab.
    if is(Key::Backtab) || (is(Key::Tab) && m.shift) {
        return Some(Command::Outdent);
    }
    if is(Key::Tab) {
        return Some(Command::Indent);
    }
    if is(Key::Backspace) {
        Some(Command::Delete(if m.alt { CaretMove::WordLeft } else { CaretMove::Left }))
    } else if is(Key::Delete) {
        Some(Command::Delete(if m.alt { CaretMove::WordRight } else { CaretMove::Right }))
    } else if is(Key::Return) {
        Some(Command::NewLine)
    } else if is_typed_text(text) {
        // ⌥ with a letter types a character on macOS, so it counts as text.
        Some(Command::InsertText(text.to_owned()))
    } else {
        None
    }
}

/// A key the quick-fix popup (ticket #45) takes from the editor while it
/// shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupKey {
    Up,
    Down,
    Accept,
    Close,
}

/// The popup key a key press is, if it is one: ↑, ↓, Return or Esc, with no
/// modifiers.
pub fn popup_key(text: &str, m: Modifiers) -> Option<PopupKey> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    if m.shift || m.cmd || m.alt || m.ctrl {
        return None;
    }
    if is(Key::UpArrow) {
        Some(PopupKey::Up)
    } else if is(Key::DownArrow) {
        Some(PopupKey::Down)
    } else if is(Key::Return) {
        Some(PopupKey::Accept)
    } else if is(Key::Escape) {
        Some(PopupKey::Close)
    } else {
        None
    }
}

/// Structural editing keys (ticket #25) that aren't menu shortcuts: ⌥↑ and
/// ⌥↓ (a menu would take them from the clone-caret gesture), and ⌥⌘+ for
/// Expand on layouts where `+` has its own key (the menu has ⌥⌘=).
fn structural(text: &str, m: Modifiers) -> Option<Command> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    if m.alt && !(m.shift || m.cmd || m.ctrl) {
        if is(Key::UpArrow) {
            return Some(Command::ExpandSelection);
        }
        if is(Key::DownArrow) {
            return Some(Command::ShrinkSelection);
        }
    }
    if m.alt && m.cmd && !m.ctrl && matches!(text, "+" | "±") {
        return Some(Command::ExpandFold);
    }
    None
}

/// Completion, hover and signature help (ticket #43), WebStorm's keys:
/// ⌃Space (Code Completion), F1 or ⌃J (Quick Documentation) and ⌘P
/// (Parameter Info). With ⌃ held, the key's text may be the control
/// character (NUL for Space, LF for J).
fn assist(text: &str, m: Modifiers) -> Option<Command> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    let plain = !(m.shift || m.cmd || m.alt || m.ctrl);
    if m.ctrl && !(m.shift || m.cmd || m.alt) {
        if matches!(text, " " | "\u{0}") {
            return Some(Command::ShowCompletion);
        }
        if matches!(text, "j" | "J" | "\n") {
            return Some(Command::ShowHover);
        }
    }
    if plain && is(Key::F1) {
        return Some(Command::ShowHover);
    }
    if m.cmd && !(m.shift || m.alt || m.ctrl) && text.eq_ignore_ascii_case("p") {
        return Some(Command::ShowSignatureHelp);
    }
    None
}

/// The caret movement a key asks for, with or without ⇧.
fn movement(text: &str, m: Modifiers) -> Option<CaretMove> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    if m.ctrl {
        return None;
    }
    let movement = if m.cmd {
        if is(Key::UpArrow) || is(Key::Home) {
            CaretMove::DocumentStart
        } else if is(Key::DownArrow) || is(Key::End) {
            CaretMove::DocumentEnd
        } else if is(Key::LeftArrow) {
            CaretMove::LineStart
        } else if is(Key::RightArrow) {
            CaretMove::LineEnd
        } else {
            return None;
        }
    } else if m.alt {
        // ⌥↑ and ⌥↓ are expand and shrink selection (`structural`).
        if is(Key::LeftArrow) {
            CaretMove::WordLeft
        } else if is(Key::RightArrow) {
            CaretMove::WordRight
        } else {
            return None;
        }
    } else if is(Key::UpArrow) {
        CaretMove::Up
    } else if is(Key::DownArrow) {
        CaretMove::Down
    } else if is(Key::LeftArrow) {
        CaretMove::Left
    } else if is(Key::RightArrow) {
        CaretMove::Right
    } else if is(Key::PageUp) {
        CaretMove::PageUp
    } else if is(Key::PageDown) {
        CaretMove::PageDown
    } else if is(Key::Home) {
        CaretMove::LineStart
    } else if is(Key::End) {
        CaretMove::LineEnd
    } else {
        return None;
    };
    Some(movement)
}

/// Whether a key's text is something to type: not empty, no control
/// characters, and none of the private-use code points Slint gives
/// function and navigation keys.
fn is_typed_text(text: &str) -> bool {
    !text.is_empty() && text.chars().all(|c| !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(&c))
}

/// Two ⌥ presses at most this far apart start the clone-caret gesture, and
/// two ⇧ presses open Search Everywhere.
const DOUBLE_PRESS: Duration = Duration::from_millis(400);

/// ⇧⇧: Search Everywhere. Two presses of ⇧ alone, with no other key
/// between them. Watches every key press, since it needs the ⇧ presses
/// themselves.
#[derive(Default)]
pub struct DoubleShift {
    last_shift: Option<Instant>,
}

impl DoubleShift {
    /// The command for this key press, if it completes a ⇧⇧.
    pub fn command_for(&mut self, text: &str) -> Option<Command> {
        if text != SharedString::from(Key::Shift).as_str() && text != SharedString::from(Key::ShiftR).as_str() {
            self.last_shift = None;
            return None;
        }
        let now = Instant::now();
        if self.last_shift.take().is_some_and(|at| now.duration_since(at) < DOUBLE_PRESS) {
            return Some(Command::OpenFinder(FinderMode::Everywhere));
        }
        self.last_shift = Some(now);
        None
    }
}

/// WebStorm's Clone Caret Above and Below: press ⌥ twice and keep it held,
/// then ↑ or ↓ (each press adds a caret). Watches every key press, since it
/// needs the ⌥ presses themselves.
#[derive(Default)]
pub struct CloneCaretGesture {
    last_option: Option<Instant>,
    armed: bool,
}

impl CloneCaretGesture {
    /// The clone command for this key press, if the gesture is on.
    pub fn command_for(&mut self, text: &str, m: Modifiers) -> Option<Command> {
        let is = |key: Key| text == SharedString::from(key).as_str();
        if is(Key::Alt) || is(Key::AltGr) {
            let now = Instant::now();
            self.armed = self.last_option.is_some_and(|at| now.duration_since(at) < DOUBLE_PRESS);
            self.last_option = Some(now);
            return None;
        }
        if self.armed && m.alt && !(m.shift || m.cmd || m.ctrl) {
            if is(Key::UpArrow) {
                return Some(Command::CloneCaretAbove);
            }
            if is(Key::DownArrow) {
                return Some(Command::CloneCaretBelow);
            }
        }
        self.armed = false;
        self.last_option = None;
        None
    }
}
