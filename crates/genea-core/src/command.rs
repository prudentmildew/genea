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
    /// Moves the caret, scrolling it into view.
    MoveCaret(CaretMove),
    /// Puts the caret at a grid cell (0-based line and display column), as a
    /// click does. Out-of-range cells clamp to the nearest text position.
    PlaceCaret { line: usize, column: usize },
}

/// Caret movements without a selection.
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
