//! Test support for core API tests (spec #19, Testing Decisions).
//!
//! A good core test builds a [`FixtureProject`] in a temp dir, creates a
//! `genea_core::Workbench` on a [`TestHost`], drives it with commands, calls
//! `settle()` instead of sleeping, and asserts on view state or on files on
//! disk. It never reaches into a crate's internals.
//!
//! ```ignore
//! let fixture = FixtureProject::new().file("src/main.ts", "let a = 1;\n").build();
//! let host = TestHost::new();
//! let mut workbench = Workbench::new(host.shared());
//! let project = workbench.open_project(fixture.root()).unwrap();
//! workbench.dispatch(project, Command::OpenFile("src/main.ts".into()));
//! workbench.settle().unwrap();
//! ```
//!
//! This crate depends on `genea-host` only, never on `genea-core`, so that
//! core's tests and core itself agree on one copy of every core type.

mod download_server;
pub mod fake_lsp;
mod fake_symbols;
mod fixture;
mod host;
mod pty;
mod tools;

pub use download_server::DownloadServer;
pub use fake_lsp::{FakeLsp, LspScript, Marker, Received};
pub use fixture::{FixtureBuilder, FixtureProject};
pub use host::{FakeProcess, ManualClock, ScriptedDownloads, ScriptedProcesses, TestClipboard, TestHost};
pub use pty::{FakePty, ScriptedPtys};
pub use tools::FakeTools;
