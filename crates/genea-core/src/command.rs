//! Commands: everything the view (or a test) can ask a project to do.
//!
//! Commands are plain data so that tests, the Slint view, menus and keymaps
//! all speak the same language. Add a variant per user-facing action; the
//! workbench routes it in `Project::dispatch`.

use std::path::PathBuf;

/// Something the user does in a project's window.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Opens a file in the editor. The path is relative to the project root,
    /// or absolute. The file is read in the background: `settle` (tests) or
    /// the change notification (the app) says when it is shown.
    OpenFile(PathBuf),
    /// Tells the core how many text rows fit in the editor viewport. Fractional:
    /// a partly visible last row counts.
    SetViewport { rows: f64 },
    /// Scrolls the editor by a number of rows (fractional for smooth
    /// trackpad scrolling; positive is down). The caret doesn't move.
    ScrollBy { rows: f64 },
    /// Moves the caret, scrolling it into view. A selection collapses:
    /// `Left` and `Right` go to its start and end.
    MoveCaret(CaretMove),
    /// Moves the caret and extends the selection to it (the movement with
    /// ⇧ held).
    Select(CaretMove),
    /// ⌘A: selects the whole file, with the caret at its end.
    SelectAll,
    /// Puts the caret at a grid cell (0-based line and display column), as a
    /// click does: at the last text position at or before the cell, so the
    /// view rounds a click to the nearest cell boundary first. Cells past the
    /// end of a line or below the last line clamp to the text.
    PlaceCaret { line: usize, column: usize },
    /// Types text at the caret, replacing the selection: a key press or an
    /// IME commit.
    InsertText(String),
    /// Deletes the selection or, with nothing selected, the text between the
    /// caret and where the movement would put it: `Delete(Left)` is
    /// Backspace, `Delete(Right)` is Delete, `Delete(WordLeft)` is ⌥⌫.
    Delete(CaretMove),
    /// Return: breaks the line at the caret with the file's line ending.
    NewLine,
    /// ⌘S: writes the open file to disk in the background. `settle` (tests)
    /// or the change notification (the app) says when it is written.
    Save,
}

/// Caret movements, for moving, selecting and deleting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    Up,
    Down,
    PageUp,
    PageDown,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}
