//! View state: what a project's window shows, as plain data.
//!
//! The Slint view binds its models to these structs and tests assert on
//! them. They hold what the user sees (grid text, 1-based labels, display
//! columns), not internal representations, so the internals can change
//! freely. They are snapshots: re-read them after a command or a change
//! notification.

use std::{ops::Range, path::PathBuf};

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
