//! Genea's benchmark harness (spec #19, ticket #23).
//!
//! A separate binary that drives the real `genea` binary through its
//! env-gated instrumentation journal and control channel
//! (`crates/genea-view/src/journal.rs` and `remote.rs`), and checks the
//! Genea budgets. `bench/README.md` says how to run it.
//!
//! - [`journal`]: reads the journal and derives frames, stalls, keystroke
//!   latencies, frame cadence, milestones and wake-ups;
//! - [`budgets`]: the budget table and the pass/fail verdict;
//! - [`stats`]: percentiles and the margin over the start floor;
//! - [`scenarios`]: what runs, one module per scenario;
//! - [`genea`]: launching and commanding Genea; [`keys`]: key codes and the
//!   keyboard layout; [`sys`]: kernel clocks and process accounting;
//!   [`lsp`]: a language-server client that times tsgo directly;
//!   [`report`]: the JSONL results and the machine they came from.
//!
//! The start floor is the `genea-floor` binary (`src/floor.rs`).

pub mod budgets;
pub mod genea;
pub mod journal;
pub mod keys;
pub mod lsp;
pub mod report;
pub mod scenarios;
pub mod stats;
pub mod sys;
