//! The host boundary (spec #19, Architecture).
//!
//! Every effect outside Genea's process and the project folder goes through a
//! [`Host`]: spawning child processes, HTTP downloads and the clock. The core
//! receives one as a [`SharedHost`] and never reaches past it.
//!
//! - [`RealHost`] runs real things. The Slint app uses it.
//! - `genea_testkit::TestHost` scripts them, so core API tests are hermetic
//!   and deterministic.
//!
//! The filesystem and the file watcher are *not* behind the host: they are
//! real in both hosts, and tests use temp-dir fixture projects instead.
//!
//! ## Extending the boundary
//!
//! A new kind of effect (a PTY, say) gets its own trait here, an accessor on
//! [`Host`], a real implementation in [`real`] and a scripted one in
//! `genea-testkit`. Keep the traits small and blocking: the core calls them
//! from background threads, never from the main thread.

mod clock;
mod downloads;
mod processes;
pub mod real;

use std::{path::Path, sync::Arc};

pub use clock::{Clock, TimerCallback};
pub use downloads::{DownloadError, Downloads};
pub use processes::{Child, Exit, ProcessControl, ProcessSpec, Processes};
pub use real::RealHost;

/// Everything the core may do outside its process and the project folder.
pub trait Host: Send + Sync + 'static {
    /// Time, timers and timeouts. Undo grouping, restart windows and request
    /// timeouts all read this clock, never `std::time` directly.
    fn clock(&self) -> &dyn Clock;
    /// Child processes: language servers, the project check, the login-shell
    /// environment capture, scripts.
    fn processes(&self) -> &dyn Processes;
    /// HTTP downloads: toolchain archives and the release-update check.
    fn downloads(&self) -> &dyn Downloads;
    /// Genea's application-support folder (`~/Library/Application
    /// Support/Genea` for the real host, a temp dir for the test host): the
    /// toolchain store and Genea's other own files live under it. It is
    /// used directly through the filesystem; this only says where it is.
    fn support_dir(&self) -> &Path;
}

/// How the core holds its host.
pub type SharedHost = Arc<dyn Host>;
