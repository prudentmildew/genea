//! Commands: everything the view (or a test) can ask a project to do.
//!
//! Commands are plain data so that tests, the Slint view, menus and keymaps
//! all speak the same language. Add a variant per user-facing action; the
//! workbench routes it in `Project::dispatch`.

use std::path::PathBuf;

use crate::{
    problems::TextPosition,
    templates::{PackageManagerPin, RuntimePin},
    view::{FinderMode, LeftColumnView, ToolchainPickerKind, WindowLayout},
};

/// Something the user does in a project's window.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Opens a file in the editor. The path is relative to the project root,
    /// or absolute. The file is read in the background: `settle` (tests) or
    /// the change notification (the app) says when it is shown.
    OpenFile(PathBuf),
    /// ⌘+: makes the editor's and the terminal's font a point larger
    /// (ticket #59), up to `MAX_FONT_SIZE`. Kept per project.
    ZoomIn,
    /// ⌘−: a point smaller, down to `MIN_FONT_SIZE`.
    ZoomOut,
    /// ⌘0: back to `DEFAULT_FONT_SIZE`.
    ResetZoom,
    /// The view reports where the window is and how it is laid out, so the
    /// session keeps it (ticket #59).
    SetWindowLayout(WindowLayout),
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
    /// New Project… (ticket #61): opens the New Project dialog, which
    /// belongs to the workbench, not this project
    /// ([`Workbench::new_project_dialog`](crate::Workbench::new_project_dialog)).
    NewProject,
    /// "Reload environment": runs the login shell again and gives processes
    /// started from then on its variables. Until it answers, they get the
    /// environment from before.
    ReloadEnvironment,
    /// "Install dependencies" (ticket #41): runs the pinned package
    /// manager's `install` in the project root, in a terminal tab of its
    /// own, once the toolchain has settled. Install scripts run project
    /// code (ADR 0005), so Genea dispatches this only on the user's click,
    /// and the new-project flow dispatches it once for a project it just
    /// created. While an install runs, it shows that tab instead.
    InstallDependencies,
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
    /// Tab: with nothing selected, types the file's indentation at each
    /// caret (a tab, or spaces up to the next multiple of its width); with a
    /// selection, indents the selected lines by one level. The indentation
    /// is what `.oxfmtrc.json` and `.editorconfig` resolve to (ticket #26),
    /// shown in `StatusBar::indentation`.
    Indent,
    /// ⇧Tab: takes one level of indentation off each line the carets and
    /// selections are on (a leading tab, or leading spaces back to the
    /// previous multiple of the indentation's width).
    Outdent,
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
    /// or the change notification (the app) says when it is written. Oxfmt
    /// formats it first, and Oxlint applies its safe fixes, as undo steps,
    /// unless `formatOnSave` or `fixOnSave` turns that off (ticket #50); if
    /// they haven't answered within [`crate::FORMAT_TIMEOUT`] on the host
    /// clock, it is written as it is, with a notice.
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

    /// "Restart language server": stops the project's language server and
    /// starts it again, also after it failed (crashed too often) and while
    /// it isn't responding. Its crash count starts over.
    RestartLanguageServer,
    /// "Add TypeScript 7" (the notice of a project without it): sets
    /// `typescript` to Genea's TypeScript 7 range in the root
    /// `package.json`, where the project lists it, else in
    /// `devDependencies`. Installing it is up to the user; language
    /// intelligence starts once it is in `node_modules`.
    AddTypeScript,
    /// "Add Oxlint and Oxfmt" (the notice of a project without them, ticket
    /// #49): adds Genea's Oxlint and Oxfmt ranges to `devDependencies` in
    /// the root `package.json`, for whichever of the two it doesn't list.
    /// Installing them is up to the user; lint starts once Oxlint is in
    /// `node_modules`.
    AddOxlintAndOxfmt,
    /// "Run project check" (ticket #48): type-checks the whole project in
    /// the background with `tsc -b --noEmit` from its TypeScript 7.
    /// `StatusBar::project_check` shows it running. Its results replace the
    /// last check's in Problems (`ProblemSource::ProjectCheck`); a check
    /// started while one runs replaces that one.
    RunProjectCheck,

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

    // Review (ticket #53). Paths are as in `ChangeItem::path`; a path that
    // isn't listed in Changes is ignored. Both work in the background.
    /// Keep: makes the file's content on disk its review baseline, so it
    /// leaves Changes.
    KeepChange(PathBuf),
    /// Revert: writes the file's review baseline back to disk (deleting a
    /// created file, restoring a deleted one), and open editors reload.
    RevertChange(PathBuf),
    /// Keep for every file in Changes.
    KeepAllChanges,
    /// Revert for every file in Changes that can be reverted.
    RevertAllChanges,
    /// Opens a file listed in Changes as an inline diff against its review
    /// baseline (ticket #55; `EditorView::inline_diff`): removed lines
    /// between its lines, added lines marked. A created file is all added;
    /// a deleted one opens empty and read-only, all removed. A file that
    /// can't be diffed (binary, large) or isn't listed opens plainly.
    OpenChange(PathBuf),
    /// Shows the focused file plainly again, closing its inline diff. A
    /// deleted file's tab closes.
    CloseInlineDiff,

    // The terminal pane (ticket #38).
    /// ⌥F12: shows the terminal pane and focuses it; if it is showing and
    /// focused, collapses it and gives the editor the focus back. The
    /// shell keeps running while the pane is collapsed.
    ToggleTerminal,
    /// Gives the terminal the keyboard focus (a click in it). The editor
    /// gets it back with `FocusPane` or `SelectTab`.
    FocusTerminal,
    /// Shows a terminal tab: an index into `TerminalView::tabs`.
    SelectTerminalTab(usize),
    /// Tells the core how many rows and columns of cells fit in the
    /// terminal pane. The shell is told too (SIGWINCH), and the grid
    /// reflows.
    SetTerminalSize { rows: usize, columns: usize },
    /// Types text into the terminal: a key press or an IME commit.
    TerminalText(String),
    /// The IME's marked text in the terminal while a dead key composes,
    /// drawn at the cursor but not sent. An empty string ends it; the
    /// composed text then arrives as `TerminalText`.
    TerminalPreedit(String),
    /// A key that isn't plain text, or one with ⌃ or ⌥ held: sent to the
    /// program the way xterm sends it. Return after the shell has exited
    /// (or failed to start) starts a new one.
    TerminalKey(TerminalKey, Modifiers),
    /// The scroll wheel over the terminal, in rows (negative is up, towards
    /// older output), over the cell at `line` and `column`. It scrolls the
    /// scrollback, unless the program takes it: a program that reports the
    /// mouse gets wheel events, and a full-screen one gets ↑ and ↓.
    ScrollTerminal { rows: i32, line: usize, column: usize },
    /// ⌘V in the terminal: types the clipboard's text, as one bracketed
    /// paste if the program asked for that (so a shell doesn't run each
    /// line as it arrives).
    TerminalPaste,
    /// A mouse button or movement over the terminal's cell at `line` and
    /// `column` (0-based visible row and grid column). Reported to a
    /// program that asked for the mouse, in the encoding it chose;
    /// otherwise nothing happens.
    TerminalMouse { action: MouseAction, line: usize, column: usize, modifiers: Modifiers },
    // Terminal tabs and links (ticket #39). The commands above act on the
    // showing tab.
    /// ⌘T in the terminal, or the pane's +: opens a tab with a new shell,
    /// shows it and focuses the pane.
    NewTerminalTab,
    /// Closes a terminal tab (by index), hanging up its shell. The pane
    /// shows the tab to its right, else the one to its left; closing the
    /// last tab collapses the pane, and showing it again opens a new one.
    CloseTerminalTab(usize),
    /// ⌘-click on the active tab's cell at `line` and `column` (0-based
    /// visible row and grid column): if a `path:line:col` reference
    /// (`TerminalLine::links`) covers it, opens that file at that place,
    /// like `OpenFileAt`, resolving a relative path against the tab's
    /// directory. The editor takes the focus.
    OpenTerminalLink { line: usize, column: usize },

    // The script runner (ticket #40).
    /// Runs a `package.json` script of the package in folder `package`
    /// (relative to the root; empty for the root package, as in
    /// `ProjectView::scripts`) as `<package manager> run <script>` in that
    /// folder, in a terminal tab named `<package>: <script>`, and shows it.
    /// A tab already running the script is just shown; one where it ended
    /// runs it again.
    RunScript { package: PathBuf, script: String },
    /// Stops a command's terminal tab (by index): interrupts its program
    /// (SIGINT), and kills it (SIGKILL) if it hasn't ended after
    /// `STOP_TIMEOUT`. The tab stays, showing it stopped.
    StopTerminalTab(usize),
    /// Runs a command's terminal tab (by index) again, in the same tab:
    /// a program still running is hung up and replaced.
    RerunTerminalTab(usize),

    /// The Search view's query changed (⌘⇧F, ticket #34): cancels the
    /// search in flight and searches the project in the background, results
    /// streaming into `ProjectView::search`. An empty query clears the
    /// results. A click on a result opens it with `OpenFileAt`.
    Search(SearchQuery),

    // The fuzzy finder (ticket #33): `ProjectView::finder`.
    /// Opens the finder in a mode with an empty query, replacing a finder
    /// that is open.
    OpenFinder(FinderMode),
    /// The finder's query changed (typing in it). Results are matched in
    /// the background: `settle` (tests) or the change notification (the
    /// app) says when they are in.
    SetFinderQuery(String),
    /// ↑ and ↓ in the finder: moves the selection by a number of results
    /// (negative is up), wrapping around at either end.
    MoveFinderSelection(isize),
    /// Selects a result by its index (the pointer over it).
    SelectFinderItem(usize),
    /// Return, or a click: closes the finder and opens the selected file
    /// or runs the selected action.
    AcceptFinder,
    /// Esc: closes the finder.
    CloseFinder,

    // Quick fixes and organize imports (ticket #45): the language servers'
    // code actions. ADR 0002: no refactors.
    /// ⌥⏎: asks the language servers for their fixes at the primary caret
    /// (or selection) for the problems there, and shows them in
    /// `ProjectView::quick_fixes` once they answer. Any command but the
    /// quick-fix ones closes it.
    ShowQuickFixes,
    /// ↑ and ↓ in the quick-fix popup: moves the selection by a number of
    /// fixes (negative is up), wrapping around at either end.
    MoveQuickFixSelection(isize),
    /// Return (the selected fix) or a click: applies a fix by its index in
    /// `QuickFixesView::items`, as one undo step per file, and closes the
    /// popup. Files that changed since the fixes were offered are left
    /// alone.
    ApplyQuickFix(usize),
    /// Esc: closes the quick-fix popup.
    CloseQuickFixes,
    /// ⌃⌥O: sorts the focused file's imports and removes unused ones, as
    /// the language server does it (`source.organizeImports`), as one undo
    /// step. Only ever on this command: saving never organizes imports.
    OrganizeImports,
    /// ⌥⌘L: formats the focused file with Oxfmt, as one undo step, whether
    /// or not `formatOnSave` is on (ticket #50). Nothing happens while Oxfmt
    /// isn't running or doesn't format the file.
    ReformatFile,

    // Code navigation and rename (ticket #44), at the focused file's primary
    // caret, answered by the language server. Places open in tabs with
    // `OpenFileAt`. Without an answer to give (no language server, nothing
    // found), `ProjectView::hint` says why until the next command.
    /// ⌘B: opens the definition of the symbol at the caret. Several
    /// definitions are listed in the Usages view instead.
    GoToDefinition,
    /// ⇧⌘B: opens the definition of the type of the symbol at the caret,
    /// like `GoToDefinition`.
    GoToTypeDefinition,
    /// ⌥⌘B: opens the implementation of the interface, class or method at
    /// the caret, or lists them in the Usages view if there are several.
    GoToImplementation,
    /// ⌥F7: lists every usage of the symbol at the caret in the Usages view
    /// (`ProjectView::usages`), grouped by file, and shows it. A click on a
    /// usage opens it with `OpenFileAt`.
    FindUsages,
    /// ⇧F6: asks for the symbol at the caret's new name
    /// (`ProjectView::rename`), if it can be renamed.
    StartRename,
    /// Renames the symbol the rename prompt is for, everywhere, and closes
    /// the prompt: every reference is edited, in open files as one undo step
    /// each, and in files that weren't open by opening them in tabs behind
    /// the focused one, unsaved. Saving them is Genea's own write, so none of
    /// it is an external change.
    Rename(String),
    /// Closes the rename prompt without renaming.
    CancelRename,
    // Completion, hover and signature help (ticket #43): the popups in
    // `EditorView`. Each asks the language server in the background; an
    // answer that comes after the text or the caret moved on is dropped.
    /// ⌃Space: lists completions at the caret. The list also opens by
    /// itself while typing a word and after the server's trigger
    /// characters (`.`, quotes, `/`, …), and typing narrows it.
    ShowCompletion,
    /// ↑ and ↓ in the completion list: moves the selection by a number of
    /// items (negative is up), wrapping around at either end.
    MoveCompletionSelection(isize),
    /// Selects a completion by its index (the pointer over it).
    SelectCompletionItem(usize),
    /// Return or Tab (or a click) in the completion list: replaces the
    /// typed word with the selected item, adding its import if it comes
    /// from another module (an auto-import), as one undo step.
    AcceptCompletion,
    /// Esc: closes the completion list.
    CloseCompletion,
    /// The pointer rests over a grid cell (0-based line and display
    /// column): shows what the server says about the code there. A cell
    /// past the end of a line closes the hover.
    HoverAt { line: usize, column: usize },
    /// F1 or ⌃J (Quick Documentation): shows the hover for the code at
    /// the caret.
    ShowHover,
    /// The pointer left, or Esc: closes the hover. Typing and moving the
    /// caret close it too.
    HideHover,
    /// ⌘P (Parameter Info): shows the signature of the call the caret is
    /// in. It also opens after typing `(` or `,` in a call, and follows the
    /// caret until it leaves the call.
    ShowSignatureHelp,
    /// Esc: closes the signature help.
    HideSignatureHelp,
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

/// What the mouse did over the terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press(MouseButton),
    Release(MouseButton),
    /// Moved with the button down.
    Drag(MouseButton),
    /// Moved with no button down.
    Move,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
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

/// What to do when an open file with unsaved edits changed on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConflictChoice {
    /// Replace the buffer with the file on disk, as one undo step, so Undo
    /// brings the unsaved edits back.
    Reload,
    /// Keep the buffer as it is; the next save overwrites the file on disk.
    KeepMyEdits,
}

/// What the Search view searches the project for.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchQuery {
    pub text: String,
    /// `text` is a regular expression (Rust `regex` syntax); otherwise it
    /// is matched literally.
    pub regex: bool,
    /// Match case; otherwise upper and lower case match each other.
    pub case_sensitive: bool,
    /// Only matches with no word character just before or after them.
    pub whole_word: bool,
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
