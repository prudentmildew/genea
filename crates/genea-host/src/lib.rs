//! The host boundary (spec #19, Architecture).
//!
//! Every effect outside Genea's process and the project folder goes through a
//! [`Host`]: spawning child processes and terminal shells, HTTP downloads, the clock and the
//! clipboard. The core
//! receives one as a [`SharedHost`] and never reaches past it.
//!
//! - [`RealHost`] runs real things. The Slint app uses it.
//! - `genea_testkit::TestHost` scripts them, so core API tests are hermetic
//!   and deterministic.
//!
//! The filesystem and the file watcher are *not* behind the host: they are
//! real in both hosts, and tests use temp-dir fixture projects instead. The
//! host only says *where* Genea keeps its own files
//! ([`Host::support_dir`]), so that tests never touch the user's.
//!
//! ## Extending the boundary
//!
//! A new kind of effect (as [`Ptys`] was) gets its own trait here, an accessor on
//! [`Host`], a real implementation in [`real`] and a scripted one in
//! `genea-testkit`. Keep the traits small and blocking: the core calls them
//! from background threads, never from the main thread.

mod clipboard;
mod clock;
mod downloads;
mod processes;
mod pty;
pub mod real;

use std::{
    ffi::OsString,
    path::Path,
    sync::Arc,
};

pub use clipboard::Clipboard;
pub use clock::{Clock, TimerCallback};
pub use downloads::{DownloadError, Downloads};
pub use processes::{Child, Exit, ProcessControl, ProcessSpec, Processes};
pub use pty::{Pty, PtyControl, PtySize, Ptys};
pub use real::RealHost;

/// Everything the core may do outside its process and the project folder.
pub trait Host: Send + Sync + 'static {
    /// Time, timers and timeouts. Undo grouping, restart windows and request
    /// timeouts all read this clock, never `std::time` directly.
    fn clock(&self) -> &dyn Clock;
    /// Child processes: language servers, the project check, the login-shell
    /// environment capture, scripts.
    fn processes(&self) -> &dyn Processes;
    /// Programs on pseudo-terminals: the terminal's shells.
    fn ptys(&self) -> &dyn Ptys;
    /// HTTP downloads: toolchain archives and the release-update check.
    fn downloads(&self) -> &dyn Downloads;
    /// The system clipboard, for Cut, Copy and Paste.
    fn clipboard(&self) -> &dyn Clipboard;
    /// Genea's application-support folder, where it keeps its own files
    /// (recent projects, session state, review baselines). It may not exist
    /// yet: whoever writes into it creates it. The real host uses
    /// `~/Library/Application Support/Genea`; the test host a temp dir.
    fn support_dir(&self) -> &Path;
    /// The environment variables Genea itself was started with. Each
    /// project's processes get the variables of the user's login shell
    /// instead (ticket #36); this is what the shell capture starts from,
    /// and the fallback when it fails. `SHELL` in it names the login shell.
    fn launch_environment(&self) -> Vec<(OsString, OsString)>;
}

/// How the core holds its host.
pub type SharedHost = Arc<dyn Host>;
