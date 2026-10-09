//! View state: what a project's window shows, as plain data.
//!
//! The Slint view binds its models to these structs and tests assert on
//! them. They hold what the user sees (grid text, 1-based labels, display
//! columns), not internal representations, so the internals can change
//! freely. They are snapshots: re-read them after a command or a change
//! notification.

use std::{ops::Range, path::PathBuf, sync::Arc};

use crate::{
    Highlight,
    command::Command,
    config::Config,
    problems::{ProblemSource, Severity, TextPosition},
};

/// One open project, as its window shows it.
#[derive(Clone, Debug, PartialEq)]
pub struct ProjectView {
    /// The project folder.
    pub root: PathBuf,
    /// The folder's name, for the window title.
    pub name: String,
    /// The focused tab's file, if any: what typing, ⌘S and the status bar
    /// act on. The same as the focused pane's `editor`.
    pub editor: Option<EditorView>,
    pub status: StatusBar,
    /// Messages for the user, oldest first.
    pub notices: Vec<Notice>,
    /// The project's runtime and package manager (ADR 0005).
    pub toolchain: ToolchainView,
    /// The editor area's sides, left to right: one, or two after a split.
    /// There is always at least one, possibly without tabs.
    pub panes: Vec<PaneView>,
    /// Index into `panes` of the side that has the focus.
    pub focused_pane: usize,
    /// Whether Split Right is available: one side, with a tab open.
    pub can_split: bool,
    /// A tab with unsaved edits is closing and asks Save, Don't Save or
    /// Cancel. Answer with `Command::ResolveClose`.
    pub close_prompt: Option<ClosePrompt>,
    /// The effective config: `genea.jsonc` over the defaults.
    pub config: Config,
    /// The Problems view's items, from every source: by file, then by
    /// place in the file.
    pub problems: Vec<ProblemItem>,
    /// The view the left column shows, or `None` while it is collapsed.
    pub left_column: Option<LeftColumnView>,
    /// The open toolchain picker ("Set runtime…", "Set package manager…",
    /// "Update toolchain…"), if any.
    pub toolchain_picker: Option<ToolchainPicker>,
    /// The Files view's tree: the rows it shows, top to bottom. Shared, so
    /// a snapshot doesn't copy a large tree, and unchanged trees compare
    /// equal at once.
    pub files: Arc<[FileRow]>,
}

/// A row in the Files view: a file, or a folder the user can expand.
/// Clicking a file opens it with `Command::OpenFile(path)`; clicking a
/// folder toggles it with `Command::ToggleFolder(path)`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileRow {
    /// Relative to the project root.
    pub path: PathBuf,
    /// The file or folder name.
    pub name: String,
    /// How deep it is: 0 for the root's entries.
    pub depth: usize,
    pub kind: FileRowKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileRowKind {
    File,
    /// A folder; its entries follow it, one level deeper, while expanded.
    Folder { expanded: bool },
}

/// One side of the editor area: a tab strip and the active tab's editor.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneView {
    pub tabs: Vec<EditorTab>,
    /// Index into `tabs` of the tab shown; `None` without tabs.
    pub active: Option<usize>,
    /// The active tab's file, scrolled and with the caret where this side
    /// left it.
    pub editor: Option<EditorView>,
}

/// A tab in a pane's tab strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorTab {
    /// The file, relative to the project root when it is inside it.
    pub path: PathBuf,
    /// The file name.
    pub title: String,
    /// The file has unsaved edits.
    pub modified: bool,
}

/// Asks what to do with a closing tab's unsaved edits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClosePrompt {
    pub path: PathBuf,
    /// The file name.
    pub title: String,
}

/// A view the left column can show. Each has a shortcut that shows it, or
/// collapses the column when it is already showing.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeftColumnView {
    /// ⌘1: the project's file tree. A project opens showing it.
    Files,
    /// ⌘6.
    Problems,
}

/// An item in the Problems view. Clicking it opens the file at the problem
/// with `Command::OpenFileAt`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProblemItem {
    pub source: ProblemSource,
    pub severity: Severity,
    /// The file, relative to the project root.
    pub path: PathBuf,
    /// Where the problem starts (0-based line, char column).
    pub position: TextPosition,
    /// `position` as the user reads it: `line:column`, 1-based.
    pub location: String,
    pub message: String,
}

/// A problem underlined in the editor, on one visible line.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InlineProblem {
    /// 0-based line index in the file.
    pub line: usize,
    /// The display columns to underline. A problem at a single place, or
    /// past the end of the line, takes one column.
    pub columns: Range<usize>,
    pub severity: Severity,
    pub message: String,
}

/// The editor surface: a grid of visible lines plus the caret.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorView {
    /// The file, relative to the project root when it is inside it.
    pub path: PathBuf,
    /// The tab title: the file name.
    pub title: String,
    pub read_only: bool,
    /// Only the file's beginning is in: the rest is still being read
    /// (ticket #27). Read-only until it is.
    pub loading: bool,
    /// The buffer has edits that aren't on disk yet: the tab and window
    /// show it as unsaved.
    pub modified: bool,
    /// The file changed on disk, outside Genea, while the buffer had
    /// unsaved edits: the editor shows a bar offering Reload or Keep my
    /// edits (`Command::ResolveConflict`). A buffer without unsaved edits
    /// reloads instead, as one undo step.
    pub conflict: bool,
    /// Lines in the file. A file ending in a newline has an empty last line.
    pub line_count: usize,
    /// The first visible row, fractional while scrolling smoothly. Rows
    /// count the lines that aren't hidden in a collapsed fold (ticket #25),
    /// so without folds a row is a line. The view offsets the grid by
    /// `scroll_top - lines[0].row` rows.
    pub scroll_top: f64,
    /// The lines in the viewport, top to bottom, including a partly visible
    /// last one. Lines hidden in a collapsed fold are left out.
    pub lines: Vec<VisibleLine>,
    /// Where the primary caret is in the file: the one the view scrolls to
    /// and the status bar reports. While composing, the view draws it after
    /// the preedit.
    pub caret: Caret,
    /// Every caret on the visible lines, primary included, top to bottom.
    /// One entry unless there are several carets (⌥-click, ⌃G, …).
    pub carets: Vec<Caret>,
    /// The IME composition being typed, if any. Its text is already spliced
    /// into the caret's line in `lines`; the view underlines it.
    pub preedit: Option<Preedit>,
    /// Problems in this file on the visible lines, from every source, top
    /// to bottom. A problem spanning lines has one entry per line.
    pub problems: Vec<InlineProblem>,
    /// The bracket at the primary caret (the one after it, else the one
    /// before it) and the bracket that matches it, top to bottom; empty
    /// when the caret isn't at a bracket or it has no match. Brackets in
    /// strings and comments don't count.
    pub brackets: Vec<Caret>,
    /// Git gutter markers on the visible lines, top to bottom: how the
    /// buffer differs from the file at HEAD (ticket #56). Empty outside a
    /// git repository and for files that aren't in HEAD. Worked out in the
    /// background as you type, so they can be a moment behind the text.
    pub gutter: Vec<GutterMark>,
    /// The change shown by a click on its gutter marker
    /// (`Command::ShowHunk`): the lines it replaced at HEAD, offered for
    /// Rollback. Typing, a click in the text and most other commands close
    /// it (`Command::HideHunk` does too); scrolling doesn't.
    pub hunk: Option<HunkView>,
}

/// A change against HEAD, as its popover shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HunkView {
    /// The buffer's lines it covers (0-based, end exclusive). Empty for
    /// deleted lines: they were just above `lines.start`.
    pub lines: Range<usize>,
    pub change: LineChange,
    /// The lines at HEAD, joined by `\n` without a final line break. Empty
    /// for added lines.
    pub head: String,
}

/// A git gutter marker on one line. Clicking it shows the change with
/// `Command::ShowHunk { line }`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GutterMark {
    /// 0-based line index in the file.
    pub line: usize,
    pub change: LineChange,
}

/// How a line differs from HEAD.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineChange {
    /// The line isn't in HEAD.
    Added,
    /// The line replaces lines that are in HEAD.
    Modified,
    /// Lines that are in HEAD were deleted just above this line. (A file
    /// ending in a newline has an empty last line, so there is always a
    /// line below a deletion.)
    Deleted,
}

/// Marked text from the IME (a dead key waiting for the next key), shown
/// inline at the caret.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preedit {
    /// 0-based line index.
    pub line: usize,
    /// The display column it starts at: the caret's.
    pub column: usize,
    /// Display columns it takes.
    pub width: usize,
}

/// A line in the viewport.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibleLine {
    /// 0-based line index in the file (the gutter shows `index + 1`).
    pub index: usize,
    /// The row it is drawn on: its index minus the lines hidden in folds
    /// above it.
    pub row: usize,
    /// Whether a fold region starts on this line, for the gutter's marker:
    /// a click on it is `Command::ToggleFold`.
    pub fold: Option<Fold>,
    /// The text as laid out on the grid: no line ending, tabs expanded to
    /// spaces, cut off after [`crate::MAX_VISIBLE_COLUMNS`] columns.
    pub text: String,
    /// Selected display columns, left to right. When a selection goes on
    /// past the end of the line, its range takes one more column for the
    /// line break.
    pub selections: Vec<Range<usize>>,
    /// Highlighted stretches of `text`, left to right, not overlapping.
    /// Text outside them is plain.
    pub highlights: Vec<HighlightSpan>,
}

/// A fold region's state, on the line it starts on (ticket #25).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fold {
    /// Its lines are shown.
    Expanded,
    /// Its lines are hidden; the view marks the line as folded.
    Collapsed,
}

/// A stretch of a visible line in one highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    /// The display columns it covers.
    pub columns: Range<usize>,
    pub highlight: Highlight,
}

/// The caret's grid cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Caret {
    /// 0-based line index.
    pub line: usize,
    /// 0-based display column: tabs and wide characters take several.
    pub column: usize,
}

/// The status bar's items.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StatusBar {
    /// The caret position as `line:column`, 1-based, or `None` with no editor.
    pub caret: Option<String>,
    /// Errors and warnings in Problems, from every source.
    pub errors: usize,
    pub warnings: usize,
    /// Set while `genea.jsonc` has errors or warnings, e.g. "genea.jsonc
    /// has 1 error"; clicking it shows Problems.
    pub config_notice: Option<String>,
    /// The open file's encoding (`UTF-8`, the only one Genea reads), or
    /// `None` with no editor.
    pub encoding: Option<String>,
    /// The open file's line ending, `LF` or `CRLF`, or `None` with no
    /// editor.
    pub line_ending: Option<String>,
    /// How the open file is indented, as `.oxfmtrc.json` and
    /// `.editorconfig` resolve it (ticket #26): `2 spaces`, `4 spaces` or
    /// `Tabs`; `None` with no editor.
    pub indentation: Option<String>,
    /// Toolchain download progress, e.g. `Downloading Node 24.18.0 42%`,
    /// while a download runs.
    pub toolchain: Option<String>,
    /// The git branch the project is on (the short commit id while HEAD is
    /// detached), or `None` outside a git repository.
    pub branch: Option<String>,
    /// Set while the open file is a large file (over
    /// [`crate::LARGE_FILE_BYTES`]): says why it has no highlighting or
    /// language intelligence.
    pub large_file: Option<String>,
}

/// A message for the user.
#[derive(Clone, Debug, PartialEq)]
pub struct Notice {
    pub message: String,
    /// A button on the notice, if it offers one.
    pub action: Option<NoticeAction>,
}

/// A notice's button: its label and the command a click dispatches.
#[derive(Clone, Debug, PartialEq)]
pub struct NoticeAction {
    pub label: String,
    pub command: Command,
}

/// The toolchain roles of a project with a root `package.json`. Both are
/// `None` for a folder without one, which has no toolchain.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ToolchainView {
    /// Node or Bun.
    pub runtime: Option<ToolView>,
    /// pnpm or Bun.
    pub package_manager: Option<ToolView>,
}

/// One toolchain role's tool.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolView {
    /// `Node`, `Bun`, `pnpm`, or the foreign tool's name (`npm`, …).
    pub tool: String,
    /// The version in use once resolved, else the pin as written (`^24`).
    pub version: String,
    pub state: ToolState,
}

/// Where a role's tool is. Every state but `Ready` means the role is off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolState {
    /// Working out which version a range pin means.
    Resolving,
    /// Downloading; `percent` is known once the server sends a length.
    Downloading { percent: Option<u8> },
    /// In the store, ready to run.
    Ready,
    /// The download or the pin failed; a notice says why.
    Failed,
    /// A foreign tool (npm, Yarn, …): Genea never runs it.
    Off,
}

/// Which toolchain picker to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ToolchainPickerKind {
    /// "Set runtime…": every Node and Bun version.
    Runtime,
    /// "Set package manager…": every pnpm and Bun version.
    PackageManager,
    /// "Update toolchain…": the versions newer than the ones in use, per
    /// role.
    Update,
}

/// A toolchain picker: a filterable list of versions. Picking an option
/// dispatches its command, which writes an exact pin and downloads it;
/// nothing changes until then.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolchainPicker {
    pub kind: ToolchainPickerKind,
    /// "Set runtime", "Set package manager" or "Update toolchain".
    pub title: String,
    /// The filter text (`Command::FilterToolchainPicker`).
    pub query: String,
    /// The version lists are still being fetched.
    pub loading: bool,
    /// Said above the list: why a list is missing or empty.
    pub message: Option<String>,
    /// Newest first, grouped by tool, filtered by `query`.
    pub options: Vec<ToolchainOption>,
}

/// One version in a toolchain picker.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolchainOption {
    /// `Node`, `Bun` or `pnpm`.
    pub tool: String,
    pub version: String,
    /// `in use`, `downloaded`, or (updates) the role and the version it
    /// replaces, e.g. `runtime, now 24.18.0`; empty otherwise.
    pub detail: String,
    /// What picking it dispatches: `SetRuntime` or `SetPackageManager`.
    pub command: Command,
}

/// The welcome, shown while no project is open: Open…, New Project… and the
/// recent projects.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WelcomeView {
    /// Projects opened before, most recent first.
    pub recent_projects: Vec<RecentProject>,
}

/// A project in the recent-projects list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecentProject {
    /// The project folder, as it was opened.
    pub root: PathBuf,
    /// The folder's name.
    pub name: String,
}
