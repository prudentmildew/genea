//! The keymap: key presses on the editor surface to core commands.
//!
//! WebStorm's macOS keymap (spec #19). Menu shortcuts never get here: the
//! native menu takes them first.

use genea_core::{CaretMove, Command};
use slint::{SharedString, platform::Key};

/// Modifier state of a key press. `cmd` is ⌘.
#[derive(Clone, Copy, Debug, Default)]
pub struct Modifiers {
    pub shift: bool,
    pub cmd: bool,
    pub alt: bool,
    pub ctrl: bool,
}

pub fn command_for(text: &str, m: Modifiers) -> Option<Command> {
    let is = |key: Key| text == SharedString::from(key).as_str();
    let caret = |movement| Some(Command::MoveCaret(movement));
    // Selections come with the editing tickets; shift+move isn't bound yet.
    if m.shift || m.alt || m.ctrl {
        return None;
    }
    if m.cmd {
        return if is(Key::UpArrow) {
            caret(CaretMove::DocumentStart)
        } else if is(Key::DownArrow) {
            caret(CaretMove::DocumentEnd)
        } else if is(Key::LeftArrow) {
            caret(CaretMove::LineStart)
        } else if is(Key::RightArrow) {
            caret(CaretMove::LineEnd)
        } else {
            None
        };
    }
    if is(Key::UpArrow) {
        caret(CaretMove::Up)
    } else if is(Key::DownArrow) {
        caret(CaretMove::Down)
    } else if is(Key::LeftArrow) {
        caret(CaretMove::Left)
    } else if is(Key::RightArrow) {
        caret(CaretMove::Right)
    } else if is(Key::PageUp) {
        caret(CaretMove::PageUp)
    } else if is(Key::PageDown) {
        caret(CaretMove::PageDown)
    } else if is(Key::Home) {
        caret(CaretMove::LineStart)
    } else if is(Key::End) {
        caret(CaretMove::LineEnd)
    } else {
        None
    }
}
