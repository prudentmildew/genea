//! View state: what a project's window shows, as plain data.
//!
//! The Slint view binds its models to these structs and tests assert on
//! them. They hold what the user sees (grid text, 1-based labels, display
//! columns), not internal representations, so the internals can change
//! freely. They are snapshots: re-read them after a command or a change
//! notification.

use std::{ops::Range, path::PathBuf};

use crate::{
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
    /// The open file, if any.
    pub editor: Option<EditorView>,
    pub status: StatusBar,
    /// Messages for the user, oldest first.
    pub notices: Vec<Notice>,
    /// The effective config: `genea.jsonc` over the defaults.
    pub config: Config,
    /// The Problems view's items, from every source: by file, then by
    /// place in the file.
    pub problems: Vec<ProblemItem>,
    /// The view the left column shows, or `None` while it is collapsed.
    pub left_column: Option<LeftColumnView>,
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
    /// Where the caret is in the file. While composing, the view draws it
    /// after the preedit.
    pub caret: Caret,
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
}

/// A message for the user.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Notice {
    pub message: String,
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
