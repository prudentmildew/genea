//! Genea's framework-free core (spec #19, Architecture; ADR 0004).
//!
//! The core owns all state and behaviour. Its single entry point is the
//! [`Workbench`]: it opens projects, takes [`Command`]s, exposes view state
//! ([`ProjectView`]) and a change notification, and offers `settle()` so
//! tests can wait for background work. The Slint view (`genea-view`) and the
//! tests use this API and nothing else.
//!
//! Rules for this crate and every core crate beside it:
//!
//! - **No GUI dependency.** No Slint, winit or AppKit, directly or
//!   transitively. `tests/no_gui_dependency.rs` checks the dependency tree.
//! - **Effects go through the host.** Processes, downloads and the clock
//!   come from the [`genea_host::Host`] passed to [`Workbench::new`]. The
//!   filesystem is used directly.
//! - **The main thread never blocks** on I/O or another process. Slow work
//!   runs through the workbench's background jobs and comes back as a state
//!   change applied by `pump` or `settle`.
//! - **Test through the API.** Tests live in `tests/`, one file per area,
//!   build projects with `genea_testkit::FixtureProject` and drive them with
//!   commands.

mod action;
mod command;
mod config;
mod dependencies;
mod diff;
mod disk;
mod editor;
mod environment;
mod files;
mod finder;
mod foreign;
mod git;
mod grid;
mod history;
mod indentation;
mod jobs;
mod lsp;
mod new_project;
mod problems;
mod project;
mod project_check;
mod reading;
mod recent;
mod review;
mod search;
mod session;
mod syntax;
mod templates;
mod terminal;
mod text;
mod toolchain;
mod update;
mod view;
mod watcher;
mod workbench;
mod workspace;

pub use action::Action;
pub use command::{CaretMove, Command, Modifiers, MouseAction, MouseButton, SearchQuery, TerminalKey};
pub use search::MAX_SEARCH_MATCHES;
pub use config::{Config, TerminalPosition, Theme};
pub use command::{CloseChoice, ConflictChoice};
pub use editor::{LARGE_FILE_BYTES, MAX_VISIBLE_COLUMNS};
pub use problems::{ProblemSource, Severity, TextPosition};
pub use syntax::Highlight;
pub use environment::LOGIN_SHELL_TIMEOUT;
pub use terminal::SCRIPT_STOP_TIMEOUT;
pub use lsp::{MAX_RESTARTS, RESTART_DELAY, RESTART_WINDOW, START_TIMEOUT};
pub use project::FORMAT_TIMEOUT;
/// What `Workbench::spawn` takes and returns, from the host boundary.
pub use genea_host::{Child, ProcessSpec};
pub use grid::{GridPiece, grid_pieces};
pub use new_project::{NewProjectCommand, NewProjectDialog, NewProjectOption};
pub use templates::{NewProject, PackageManagerPin, ProjectCreation, RuntimePin, Template};
pub use update::{RELEASES_URL, UpdateNotice};
pub use view::{
    Caret, EditorView, Fold, GutterMark, HighlightSpan, HunkView, InlineProblem, LanguageServerState,
    LanguageServerStatus, LeftColumnView, LineChange, Notice, NoticeAction, Preedit, ProblemItem, ProjectView,
    RecentProject, StatusBar, ToolState, ToolView, ToolchainOption, ToolchainPicker, ToolchainPickerKind,
    ToolchainView, VisibleLine, WelcomeView,
};
pub use view::{
    ChangeItem, ChangeKind, ClosePrompt, EditorTab, FileRow, FileRowKind, PaneView, RevertAllPrompt, SearchFile,
    SearchMatch, SearchView,
};
pub use view::{
    TerminalColor, TerminalCursor, TerminalFileLink, TerminalLine, TerminalRun, TerminalStatus, TerminalStyle,
    TerminalTab, TerminalView,
};
pub use view::{FinderItem, FinderItemKind, FinderMode, FinderView};
pub use view::QuickFixesView;
pub use view::{DiffAgainst, InlineDiffView, RemovedLine};
pub use view::{PackageScripts, Script, ScriptLink};
pub use view::{RenamePrompt, UsagesView};
pub use view::{DEFAULT_FONT_SIZE, MAX_FONT_SIZE, MIN_FONT_SIZE, WindowFrame, WindowLayout};
pub use view::{
    CompletionItem, CompletionKind, CompletionView, HoverView, MAX_COMPLETION_ITEMS, MarkupBlock, SignatureHelpView,
};
pub use workbench::{OpenProjectError, ProjectId, SettleError, Workbench};
