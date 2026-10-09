//! View state: what a project's window shows, as plain data.
//!
//! The Slint view binds its models to these structs and tests assert on
//! them. They hold what the user sees (grid text, 1-based labels, display
//! columns), not internal representations, so the internals can change
//! freely. They are snapshots: re-read them after a command or a change
//! notification.

use std::{ops::Range, path::PathBuf};

use crate::Highlight;
use crate::command::Command;

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

/// The editor surface: a grid of visible lines plus the caret.
#[derive(Clone, Debug, PartialEq)]
pub struct EditorView {
    /// The file, relative to the project root when it is inside it.
    pub path: PathBuf,
    /// The tab title: the file name.
    pub title: String,
    pub read_only: bool,
    /// The buffer has edits that aren't on disk yet: the tab and window
    /// show it as unsaved.
    pub modified: bool,
    /// Lines in the file. A file ending in a newline has an empty last line.
    pub line_count: usize,
    /// The first visible row, fractional while scrolling smoothly. The view
    /// offsets the grid by `scroll_top - lines[0].index` rows.
    pub scroll_top: f64,
    /// The lines in the viewport, top to bottom, including a partly visible
    /// last one.
    pub lines: Vec<VisibleLine>,
    /// Where the caret is in the file. While composing, the view draws it
    /// after the preedit.
    pub caret: Caret,
    /// The IME composition being typed, if any. Its text is already spliced
    /// into the caret's line in `lines`; the view underlines it.
    pub preedit: Option<Preedit>,
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
    /// The open file's encoding (`UTF-8`, the only one Genea reads), or
    /// `None` with no editor.
    pub encoding: Option<String>,
    /// The open file's line ending, `LF` or `CRLF`, or `None` with no
    /// editor.
    pub line_ending: Option<String>,
    /// Toolchain download progress, e.g. `Downloading Node 24.18.0 42%`,
    /// while a download runs.
    pub toolchain: Option<String>,
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
