//! View state: what a project's window shows, as plain data.
//!
//! The Slint view binds its models to these structs and tests assert on
//! them. They hold what the user sees (grid text, 1-based labels, display
//! columns), not internal representations, so the internals can change
//! freely. They are snapshots: re-read them after a command or a change
//! notification.

use std::{ops::Range, path::PathBuf};

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
    /// The terminal pane (ticket #38).
    pub terminal: TerminalView,
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

/// The terminal pane next to the editor (ticket #38): one shell, drawn on a
/// grid like the editor.
#[derive(Clone, Debug, PartialEq)]
pub struct TerminalView {
    /// The pane is showing; ⌥F12 collapses it.
    pub visible: bool,
    /// Keys and text go to the terminal, not the editor.
    pub focused: bool,
    pub status: TerminalStatus,
    /// The title the program set (OSC 0 or 2), else the shell's name.
    pub title: String,
    /// The grid's size in cells.
    pub rows: usize,
    pub columns: usize,
    /// The visible rows, top to bottom: always `rows` of them.
    pub lines: Vec<TerminalLine>,
    /// Where the terminal's cursor is on the visible rows; `None` while the
    /// program hides it or it is scrolled out of view.
    pub cursor: Option<TerminalCursor>,
    /// Lines in the scrollback, above the screen (at most 10,000).
    pub history: usize,
    /// How many lines the view is scrolled back into the scrollback: 0
    /// shows the screen.
    pub scrolled_back: usize,
    /// A full-screen program has switched to the alternate screen, which
    /// has no scrollback.
    pub alternate_screen: bool,
    /// The program takes mouse clicks (mouse reporting is on).
    pub mouse_reporting: bool,
}

/// Whether the terminal's shell runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalStatus {
    /// Waiting for the project environment, or starting.
    Starting,
    Running,
    /// The shell exited, with its code if it exited normally.
    Exited { code: Option<i32> },
    /// The shell couldn't start: why.
    Failed(String),
}

/// One visible row of the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalLine {
    /// The row's text in grid columns, without trailing blanks. A wide
    /// character takes two columns.
    pub text: String,
    /// The row in stretches of one style, left to right, covering `text`
    /// and any blank cells after it that have a background colour.
    pub runs: Vec<TerminalRun>,
}

/// A stretch of a terminal row in one style.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalRun {
    /// The grid columns it covers.
    pub columns: Range<usize>,
    pub text: String,
    pub style: TerminalStyle,
}

/// How terminal text looks. Inverse video is already applied (the colours
/// are swapped), and hidden text has its background as its foreground.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalStyle {
    pub foreground: TerminalColor,
    pub background: TerminalColor,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strikeout: bool,
    /// Faint text.
    pub dim: bool,
    /// The target of an OSC 8 hyperlink.
    pub link: Option<String>,
}

impl Default for TerminalStyle {
    fn default() -> Self {
        TerminalStyle {
            foreground: TerminalColor::Foreground,
            background: TerminalColor::Background,
            bold: false,
            italic: false,
            underline: false,
            strikeout: false,
            dim: false,
            link: None,
        }
    }
}

/// A terminal colour. The theme decides the default colours and the 16
/// ANSI ones; 256-colour indices above 15 and 24-bit colours are exact.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TerminalColor {
    /// The theme's terminal text colour.
    Foreground,
    /// The theme's terminal background.
    Background,
    /// ANSI colour 0–15: black, red, green, yellow, blue, magenta, cyan,
    /// white, then their bright versions.
    Ansi(u8),
    Rgb(u8, u8, u8),
}

/// The terminal's cursor cell.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalCursor {
    /// 0-based visible row.
    pub line: usize,
    /// 0-based grid column.
    pub column: usize,
}
