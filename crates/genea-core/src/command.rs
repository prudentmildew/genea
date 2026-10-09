//! Commands: everything the view (or a test) can ask a project to do.
//!
//! Commands are plain data so that tests, the Slint view, menus and keymaps
//! all speak the same language. Add a variant per user-facing action; the
//! workbench routes it in `Project::dispatch`.

use std::path::PathBuf;

use crate::{problems::TextPosition, view::LeftColumnView};

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
    /// Retries the toolchain downloads that failed (a notice's Retry).
    RetryToolchain,
    /// Writes Genea's default versions as exact pins to `package.json` for
    /// the roles the project doesn't pin (the unpinned notice's action).
    PinToolchainDefaults,
    /// "Reload environment": runs the login shell again and gives processes
    /// started from then on its variables. Until it answers, they get the
    /// environment from before.
    ReloadEnvironment,
    /// Moves the caret to a grid cell like [`PlaceCaret`](Self::PlaceCaret)
    /// but keeps the selection's anchor: a drag, or a ⇧-click.
    ExtendSelection { line: usize, column: usize },
    /// ⌥-click: adds a caret at a grid cell (placed like
    /// [`PlaceCaret`](Self::PlaceCaret)) and makes it the primary one. On a
    /// caret that is already there, removes it instead, unless it is the
    /// only one. Typing, deleting and pasting then apply at every caret.
    AddCaret { line: usize, column: usize },
    /// ⌃G: with nothing selected, selects the word at the caret; then adds
    /// a caret selecting the next occurrence of the selection (wrapping
    /// around to the top), as the primary. A word selected by ⌃G only
    /// matches whole words.
    SelectNextOccurrence,
    /// ⌃⇧G: removes the caret added last, making the one before it primary.
    UnselectLastOccurrence,
    /// ⌃⌘G: selects every occurrence of the selection, or of the word at
    /// the caret (whole words), with a caret at each.
    SelectAllOccurrences,
    /// Adds a caret on the line above the primary caret, at the same
    /// column (or the line's end), as the new primary. If the caret before
    /// the primary is on that line, so the primary was cloned below it,
    /// removes the primary instead.
    CloneCaretAbove,
    /// Like [`CloneCaretAbove`](Self::CloneCaretAbove), on the line below.
    CloneCaretBelow,
    /// Esc: drops every caret but the primary, which keeps its selection.
    CollapseCarets,
    /// Double-click: selects the word (or punctuation or space run) at a
    /// grid cell.
    SelectWord { line: usize, column: usize },
    /// Triple-click: selects a whole line with its line break.
    SelectLine { line: usize },
    /// Types text at the caret, replacing the selection: a key press or an
    /// IME commit.
    InsertText(String),
    /// The IME's marked text while a dead key composes (`´` before `e`),
    /// drawn at the caret but not in the file. An empty string ends the
    /// composition; the composed text then arrives as `InsertText`.
    SetPreedit(String),
    /// Deletes the selection or, with nothing selected, the text between the
    /// caret and where the movement would put it: `Delete(Left)` is
    /// Backspace, `Delete(Right)` is Delete, `Delete(WordLeft)` is ⌥⌫.
    Delete(CaretMove),
    /// Return: breaks the line at the caret with the file's line ending.
    NewLine,
    /// ⌘C: puts the selection on the system clipboard. Does nothing with
    /// nothing selected.
    Copy,
    /// ⌘X: moves the selection to the system clipboard.
    Cut,
    /// ⌘V: types the system clipboard's text, replacing the selection.
    Paste,
    /// ⌘Z: reverts the last undo step and restores the selections from
    /// before it. Consecutive typing is one step until a pause of about a
    /// second on the host clock, a caret jump, or a switch between typing
    /// and deleting. Paste and Cut are steps of their own.
    Undo,
    /// ⌘⇧Z: makes the last undone step again and restores the selections
    /// from after it. Any edit after an undo drops what could be redone.
    Redo,
    /// ⌘S: writes the open file to disk in the background. `settle` (tests)
    /// or the change notification (the app) says when it is written.
    Save,
    /// Shows a file with the caret at a place in it: a click on a Problems
    /// item (and later a search result or a terminal link). A file that is
    /// already open keeps its buffer; otherwise it is read like `OpenFile`.
    OpenFileAt { path: PathBuf, at: TextPosition },
    /// "Open config": opens the root `genea.jsonc`, creating it as `{}` if
    /// it is missing.
    OpenConfig,
    /// A left-column view's shortcut (⌘6 for Problems): shows that view, or
    /// collapses the column if it is already showing.
    ToggleLeftColumn(LeftColumnView),

    // Tabs and the split (ticket #31). Panes are indexed left to right and
    // tabs left to right within a pane, as `ProjectView::panes` lists them.
    // `OpenFile` opens a tab in the focused pane, or focuses the file's tab
    // if it is already open on either side.
    /// Shows a tab and focuses its pane.
    SelectTab { pane: usize, tab: usize },
    /// Focuses a pane (a click in it), keeping its active tab.
    FocusPane(usize),
    /// Closes a tab. If it is the file's last tab and the file has unsaved
    /// edits, nothing closes yet: `ProjectView::close_prompt` asks first.
    CloseTab { pane: usize, tab: usize },
    /// Answers the close prompt.
    ResolveClose(CloseChoice),
    /// Opens the focused tab's file again in a new right-hand pane and
    /// focuses it. Only one split exists: with two panes this does nothing.
    SplitRight,
    /// Moves a tab to the other pane, splitting if there is only one. A pane
    /// left without tabs closes.
    MoveTabToOtherSide { pane: usize, tab: usize },
    /// Ends the split: the right pane's tabs join the left one.
    CloseSplit,
    /// Scrolls a pane that may not have the focus (the trackpad over it).
    ScrollPane { pane: usize, rows: f64 },

    // The terminal pane (ticket #38).
    /// Tells the core how many rows and columns of cells fit in the
    /// terminal pane. The shell is told too (SIGWINCH), and the grid
    /// reflows.
    SetTerminalSize { rows: usize, columns: usize },
    /// Types text into the terminal: a key press or an IME commit.
    TerminalText(String),
    /// A key that isn't plain text, or one with ⌃ or ⌥ held: sent to the
    /// program the way xterm sends it.
    TerminalKey(TerminalKey, Modifiers),
    /// The scroll wheel over the terminal, in rows (negative is up, towards
    /// older output), over the cell at `line` and `column`. It scrolls the
    /// scrollback, unless the program takes it: a program that reports the
    /// mouse gets wheel events, and a full-screen one gets ↑ and ↓.
    ScrollTerminal { rows: i32, line: usize, column: usize },
}

/// A key for the terminal that [`Command::TerminalText`] can't carry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalKey {
    Enter,
    Backspace,
    Tab,
    Escape,
    Up,
    Down,
    Left,
    Right,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    /// F1 to F12.
    F(u8),
    /// A character key with ⌃ or ⌥ held (⌃C is `Char('c')` with `ctrl`).
    Char(char),
}

/// Modifier keys held with a terminal key or mouse event.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    /// ⌥, which the terminal treats as Meta.
    pub alt: bool,
    /// ⌃.
    pub ctrl: bool,
}

/// What to do with unsaved edits in a closing tab.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CloseChoice {
    /// Write the file, then close the tab once it is written.
    Save,
    /// Close the tab and lose the edits.
    Discard,
    /// Keep the tab open.
    Cancel,
}

/// Caret movements, for moving, selecting and deleting.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaretMove {
    Left,
    Right,
    /// ⌥←: to the start of the word (or punctuation run) before the caret.
    WordLeft,
    /// ⌥→: to the end of the word (or punctuation run) after the caret.
    WordRight,
    Up,
    Down,
    PageUp,
    PageDown,
    LineStart,
    LineEnd,
    DocumentStart,
    DocumentEnd,
}
