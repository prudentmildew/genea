//! The scenarios the harness runs, in order.
//!
//! **Adding a scenario** (#63: navigation, memory with workspaces open,
//! end-to-end targets): write a module here with a type implementing
//! [`Scenario`], add its budgets to `crate::budgets`, and list it in
//! [`all`]. A scenario launches Genea through [`Context::launch`], drives it
//! with the control-channel commands on [`Genea`], reads the journal back,
//! writes what it measured with [`Context::record`] (one JSONL line each)
//! and returns a [`Check`] per budget. If it needs a command Genea doesn't
//! have yet, add it to `crates/genea-view/src/remote.rs`, and a mark the
//! journal doesn't record yet to `crates/genea-view/src/journal.rs` and
//! [`crate::journal::Journal`].

mod dead_keys;
mod idle;
mod scroll;
mod start;
mod typing;

use std::{
    path::{Path, PathBuf},
    time::Duration,
};

use serde_json::{Value, json};

use crate::{budgets::Check, genea::Genea, report::Output};

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
        Box::new(scroll::Scroll),
        Box::new(dead_keys::DeadKeys),
        Box::new(idle::Idle),
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
        }
    }
}

pub struct Context {
    pub genea: PathBuf,
    pub floor: PathBuf,
    pub workspace: PathBuf,
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
