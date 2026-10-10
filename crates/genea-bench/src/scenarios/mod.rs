//! The scenarios the harness runs, in order.
//!
//! **Adding a scenario**: write a module here with a type implementing
//! [`Scenario`], add its budgets to `crate::budgets`, and list it in
//! [`all`]. A scenario launches Genea through [`Context::launch`], drives it
//! with the control-channel commands on [`Genea`], reads the journal back,
//! writes what it measured with [`Context::record`] (one JSONL line each)
//! and returns a [`Check`] per budget. If it needs a command Genea doesn't
//! have yet, add it to `crates/genea-view/src/remote.rs`, and a mark the
//! journal doesn't record yet to `crates/genea-view/src/journal.rs` and
//! [`crate::journal::Journal`]. To time something that shows in view state,
//! `expect` a condition on it, act, and `await` it ([`Genea::expect`]),
//! then take [`reaction_ms`] from the journal; the navigation scenarios
//! (ticket #63) do, on each [`Reference`] workspace.

mod dead_keys;
mod external;
mod finder;
mod idle;
mod language_server;
mod memory;
mod open;
mod scroll;
mod search;
mod start;
mod tree;
mod typing;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use serde_json::{Value, json};

use crate::{
    budgets::{Budget, Check},
    genea::{Genea, Met},
    journal::{Journal, ms},
    report::Output,
    sys,
};

pub trait Scenario {
    /// Short, stable, used by `--only` and in the results.
    fn name(&self) -> &'static str;
    /// Runs the scenario and checks its budgets. An error means it couldn't
    /// be measured at all; that fails the run.
    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String>;
}

/// Every scenario, in the order a full run takes them.
pub fn all() -> Vec<Box<dyn Scenario>> {
    vec![
        Box::new(start::Start),
        Box::new(typing::Typing),
        Box::new(typing::TypingSilentServer),
        Box::new(scroll::Scroll),
        Box::new(dead_keys::DeadKeys),
        Box::new(idle::Idle),
        Box::new(open::OpenOneMb),
        Box::new(open::OpenHundredMb),
        Box::new(finder::FuzzyFinder),
        Box::new(search::ProjectSearch),
        Box::new(tree::FolderExpand),
        Box::new(external::ExternalChange),
        Box::new(memory::WorkspaceMemory),
        Box::new(language_server::LanguageServer),
    ]
}

/// The Typical workspace's file every scenario opens (649 lines): the
/// largest source file in it.
pub const FILE: &str = "packages/web/src/index.ts";

/// How many times things run. `--quick` cuts every count for a smoke test.
#[derive(Clone, Debug)]
pub struct Options {
    pub warm_runs: usize,
    pub cold_runs: usize,
    pub cold: bool,
    pub typing_keys: usize,
    pub scroll_seconds: u64,
    pub idle_runs: usize,
    pub idle_seconds: u64,
    /// Opens of the 1 MB file and of the 100 MB file, each in a fresh Genea.
    pub open_runs: usize,
    pub open_large_runs: usize,
    /// Rounds of typing the finder's queries, per reference workspace.
    pub finder_rounds: usize,
    /// Searches, per reference workspace.
    pub search_runs: usize,
    /// Rounds of expanding and collapsing the folders, per workspace.
    pub tree_rounds: usize,
    /// Outside changes, per reference workspace.
    pub external_runs: usize,
    /// Fresh Geneas sampled for memory, per reference workspace.
    pub memory_runs: usize,
    /// Fresh Geneas timed to their first diagnostics (tsgo cold).
    pub lsp_runs: usize,
    /// Edits timed to their diagnostics, and requests timed per kind.
    pub lsp_edits: usize,
    pub lsp_requests: usize,
}

impl Options {
    pub fn full() -> Options {
        Options {
            warm_runs: 30,
            cold_runs: 10,
            cold: true,
            typing_keys: 400,
            scroll_seconds: 6,
            idle_runs: 3,
            idle_seconds: 10,
            open_runs: 20,
            open_large_runs: 5,
            finder_rounds: 3,
            search_runs: 20,
            tree_rounds: 10,
            external_runs: 20,
            memory_runs: 3,
            lsp_runs: 5,
            lsp_edits: 20,
            lsp_requests: 30,
        }
    }

    pub fn quick() -> Options {
        Options {
            warm_runs: 5,
            cold_runs: 2,
            cold: true,
            typing_keys: 60,
            scroll_seconds: 2,
            idle_runs: 1,
            idle_seconds: 4,
            open_runs: 3,
            open_large_runs: 2,
            finder_rounds: 1,
            search_runs: 3,
            tree_rounds: 2,
            external_runs: 3,
            memory_runs: 1,
            lsp_runs: 1,
            lsp_edits: 6,
            lsp_requests: 5,
        }
    }
}

pub struct Context {
    pub genea: PathBuf,
    pub floor: PathBuf,
    /// `genea-fake-lsp`, which stands in for tsgo where a scenario needs a
    /// language server that misbehaves.
    pub fake_lsp: PathBuf,
    pub workspace: PathBuf,
    /// The Large workspace, if it is set up (ticket #63).
    pub large: Option<PathBuf>,
    pub options: Options,
    pub out: Output,
    /// The scenario running now, stamped on its records.
    pub scenario: &'static str,
}

impl Context {
    /// Starts Genea on [`FILE`] in the workspace.
    pub fn launch(&self, journal: bool) -> Result<Genea, String> {
        Genea::launch(&self.genea, &self.workspace, FILE, journal)
    }

    /// Writes one JSONL record for the running scenario.
    pub fn record(&mut self, kind: &str, mut value: Value) {
        if let Value::Object(map) = &mut value {
            map.insert("type".into(), json!(kind));
            map.insert("scenario".into(), json!(self.scenario));
        }
        self.out.write(&value);
    }

    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Typical, and Large if it is set up (ticket #63).
    pub fn references(&self) -> Vec<Reference> {
        let mut references = vec![Reference::typical(self.workspace.clone())];
        references.extend(self.large.clone().map(Reference::large));
        references
    }

    /// Starts Genea, with the journal, on a reference workspace's file.
    pub fn launch_on(&self, reference: &Reference) -> Result<Genea, String> {
        Genea::launch(&self.genea, &reference.root, reference.file, true)
    }
}

/// Which reference workspace (GLOSSARY.md): Genea budgets apply in full to
/// Typical; Large must stay usable, with budgets of its own where #6 set
/// them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Typical,
    Large,
}

/// A reference workspace and what the navigation scenarios do in it.
#[derive(Clone, Debug)]
pub struct Reference {
    pub size: Size,
    pub root: PathBuf,
    /// The file Genea opens.
    pub file: &'static str,
    /// Typed into the finder one key at a time (lowercase letters only).
    pub finder_queries: &'static [&'static str],
    /// Searched for in the project: common enough to fill the results, under
    /// `MAX_SEARCH_MATCHES` so the search runs to the end.
    pub search: &'static str,
    /// Expanded in this order (each one's parent comes first), then
    /// collapsed in reverse.
    pub folders: &'static [&'static str],
}

impl Reference {
    pub fn typical(root: PathBuf) -> Reference {
        Reference {
            size: Size::Typical,
            root,
            file: FILE,
            finder_queries: &["invoicedraft", "webcartconfig", "apiorderentry"],
            // 5,498 matches in Typical.
            search: "invoice",
            folders: &[
                "packages",
                "packages/web",
                "packages/web/src",
                "packages/web/src/invoice",
                "packages/api",
                "packages/api/src",
                "packages/api/src/order",
            ],
        }
    }

    pub fn large(root: PathBuf) -> Reference {
        Reference {
            size: Size::Large,
            root,
            // 2,745 lines.
            file: "src/vs/editor/common/model/textModel.ts",
            finder_queries: &["textmodel", "editoroptions", "workbenchlayout"],
            // 1,402 matches in Large.
            search: "createDecorator",
            folders: &[
                "src",
                "src/vs",
                "src/vs/workbench",
                "src/vs/workbench/contrib",
                "src/vs/workbench/contrib/chat",
                "src/vs/editor",
                "extensions",
            ],
        }
    }

    pub fn name(&self) -> &'static str {
        match self.size {
            Size::Typical => "Typical",
            Size::Large => "Large",
        }
    }

    /// The budget for this workspace, of a Typical and Large pair.
    pub fn budget(&self, typical: Budget, large: Budget) -> Budget {
        match self.size {
            Size::Typical => typical,
            Size::Large => large,
        }
    }
}

/// The Large workspace's budget, skipped because it isn't set up.
pub fn large_missing(budget: Budget) -> Check {
    budget.skipped("the Large workspace isn't set up: run bench/workspaces/large/fetch.sh --ignore-scripts")
}

/// From something done at `from` to the frame showing what it did: Genea's
/// work and the time to the present, in ms. When nothing on screen changed
/// (no frame came), both are the time to the view state.
pub fn reaction_ms(journal: &Journal, from: u64, met: Met) -> Option<(f64, f64)> {
    if met.unchanged {
        let d = ms(met.at.saturating_sub(from));
        return Some((d, d));
    }
    let reaction = journal.reaction(from, met.at)?;
    Some((reaction.genea_work_ms(), reaction.input_to_present_ms()))
}

/// Waits until Genea has had nothing to do for a second (the project's
/// files read, its first background work done), up to `limit`. Returns how
/// long it took. Its child processes (the language server) don't count.
pub fn quiesce(genea: &Genea, limit: Duration) -> Result<Duration, String> {
    let started = Instant::now();
    let pid = genea.pid();
    let gone = || format!("Genea (pid {pid}) exited");
    let mut quiet = 0;
    let mut last = sys::usage(pid).ok_or_else(gone)?;
    while started.elapsed() < limit {
        sleep(Duration::from_millis(250));
        let now = sys::usage(pid).ok_or_else(gone)?;
        // Under 2 % of a core.
        quiet = if now.cpu_ns - last.cpu_ns < 5_000_000 { quiet + 1 } else { 0 };
        last = now;
        if quiet >= 4 {
            return Ok(started.elapsed());
        }
    }
    Err(format!("Genea was still busy after {limit:?}"))
}

pub fn sleep(duration: Duration) {
    std::thread::sleep(duration);
}

/// A check that Genea's window is what the budgets assume: visible, so
/// presentation isn't throttled.
pub fn require_visible(genea: &mut Genea) -> Result<crate::genea::Info, String> {
    let info = genea.info()?;
    if !info.visible {
        return Err("Genea's window is occluded, so frames are throttled; keep it in front and unobstructed".into());
    }
    Ok(info)
}
