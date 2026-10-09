//! Commands: everything the view (or a test) can ask a project to do.
//!
//! Commands are plain data so that tests, the Slint view, menus and keymaps
//! all speak the same language. Add a variant per user-facing action; the
//! workbench routes it in `Project::dispatch`.

use std::path::PathBuf;

use crate::{
    problems::TextPosition,
    templates::{PackageManagerPin, RuntimePin},
    view::{LeftColumnView, ToolchainPickerKind},
};

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
    /// "Set runtime…", "Set package manager…" or "Update toolchain…":
    /// opens the toolchain picker, which lists versions in the background
    /// (`ProjectView::toolchain_picker`). Nothing is written until an
    /// option's command is dispatched.
    OpenToolchainPicker(ToolchainPickerKind),
    /// Typing in the toolchain picker: shows only the options whose tool,
    /// version or detail contain the text (ignoring case).
    FilterToolchainPicker(String),
    /// Closes the toolchain picker without changing anything.
    CloseToolchainPicker,
    /// Pins the runtime: writes `devEngines.runtime` to the root
    /// `package.json` as an exact version, then downloads that version. A
    /// toolchain picker option's command; closes the picker.
    SetRuntime(RuntimePin),
    /// Pins the package manager: writes `packageManager` to the root
    /// `package.json` as an exact version, then downloads that version. A
    /// toolchain picker option's command; closes the picker.
    SetPackageManager(PackageManagerPin),
    /// "Remove unused toolchains": deletes every version in the shared
    /// toolchain store that no recently opened (or open) project uses. A
    /// notice says what was removed.
    RemoveUnusedToolchains,
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
    /// A click on a folder in the Files view (a path relative to the
    /// project root): expands it, or collapses it if it is expanded. A
    /// folder keeps what was expanded inside it while it is collapsed.
    ToggleFolder(PathBuf),

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

    // Structural editing (ticket #25).
    /// ⌘/: comments out the lines the carets and selections are on, or
    /// uncomments them if every one that isn't blank is commented. Uses the
    /// language's line comment (`//`, `#`), or wraps each line in its block
    /// comment (`/* */` in CSS, `<!-- -->` in HTML and Markdown). A
    /// selection that ends at the start of a line leaves that line out.
    ToggleLineComment,
    /// ⌥↑: grows each selection to the smallest syntax node around it (the
    /// inside of a block comes before the block). An empty selection grows
    /// to the node at its caret.
    ExpandSelection,
    /// ⌥↓: undoes the last `ExpandSelection`, step by step, as long as
    /// nothing else changed the selection or the text in between.
    ShrinkSelection,
    /// A click on a fold marker in the gutter: collapses the fold region
    /// that starts on `line` (0-based, in the file), or expands it if it is
    /// collapsed. A collapsed region's lines are hidden, keeping its first
    /// line and the line its closing bracket or tag starts.
    ToggleFold { line: usize },
    /// ⌥⌘−: collapses the region that starts on the primary caret's line,
    /// or else the innermost expanded region around the caret. Carets in
    /// the hidden lines move to the region's start.
    CollapseFold,
    /// ⌥⌘+: expands the collapsed regions on the primary caret's line.
    ExpandFold,
    /// Collapses every fold region in the file.
    CollapseAllFolds,
    /// Expands every collapsed region.
    ExpandAllFolds,

    // Git (ticket #56).
    /// A click on a git gutter marker of the focused file: shows the change
    /// on that line (`EditorView::hunk`) with its lines at HEAD. A line
    /// without a marker shows nothing.
    ShowHunk { line: usize },
    /// Closes the shown change (Esc, a click outside it).
    HideHunk,
    /// The shown change's Rollback: puts its lines at HEAD back in the
    /// buffer, as an edit that Undo reverts, and closes it. Does nothing
    /// without a shown change.
    RollbackHunk,
    /// Answers an open file's conflict bar (`EditorView::conflict`): its
    /// file changed on disk while it had unsaved edits. The path is as in
    /// `EditorView::path`.
    ResolveConflict { path: PathBuf, choice: ConflictChoice },
}

/// What to do when an open file with unsaved edits changed on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Replace the buffer with the file on disk, as one undo step, so Undo
    /// brings the unsaved edits back.
    Reload,
    /// Keep the buffer as it is; the next save overwrites the file on disk.
    KeepMyEdits,
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
