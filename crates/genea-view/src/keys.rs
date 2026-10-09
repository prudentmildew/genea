//! The keymap: key presses on the editor surface to core commands.
//!
//! WebStorm's macOS keymap (spec #19). Menu shortcuts (⌘S, ⌘X, ⌘C, ⌘V, ⌘A)
//! never get here: the native menu takes them first.

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
    if let Some(movement) = movement(text, m) {
        return Some(if m.shift { Command::Select(movement) } else { Command::MoveCaret(movement) });
    }
    let is = |key: Key| text == SharedString::from(key).as_str();
    if m.ctrl || m.cmd {
        return None;
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
