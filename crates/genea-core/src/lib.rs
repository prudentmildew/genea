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

mod command;
mod editor;
mod jobs;
mod project;
mod recent;
mod text;
mod update;
mod view;
mod workbench;

pub use command::{CaretMove, Command};
pub use editor::MAX_VISIBLE_COLUMNS;
pub use update::{RELEASES_URL, UpdateNotice};
pub use view::{
    Caret, EditorView, Notice, Preedit, ProjectView, RecentProject, StatusBar, VisibleLine, WelcomeView,
};
pub use workbench::{OpenProjectError, ProjectId, SettleError, Workbench};
