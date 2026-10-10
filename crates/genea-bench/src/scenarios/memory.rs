//! Memory with a reference workspace open (ticket #63), and the language
//! server's memory on Typical (an end-to-end target).
//!
//! Each run launches Genea on the workspace with its file, resizes the
//! window to the 1200×800 pt the budget assumes, and waits until the
//! language server is ready (or off) and Genea has had nothing to do for a
//! second: the project's files read, the open file's diagnostics in. From
//! well past 2 s after that, it samples `phys_footprint` once a second, of
//! Genea and of the language server (Genea's child processes running tsgo).
//! The budget is Genea's footprint at p95 over every sample of every run;
//! the language server is the target's, not the budget's.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Context, Reference, Scenario, Size, large_missing, quiesce, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    genea::Genea,
    stats::{Summary, summary_json},
    sys,
};

pub struct WorkspaceMemory;

/// The window the memory budgets are defined for, in points.
const BUDGET_WINDOW: (f64, f64) = (1200.0, 800.0);
/// From Genea going quiet to the first sample: past the 2 s after the last
/// frame in which the GPU driver lets go of memory.
const SETTLE: Duration = Duration::from_secs(4);

impl Scenario for WorkspaceMemory {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut checks = Vec::new();
        for reference in cx.references() {
            let (mut genea_mb, mut server_mb, mut windows) = (Vec::new(), Vec::new(), Vec::new());
            for run in 0..cx.options.memory_runs {
                let sample = sample(cx, &reference)?;
                cx.record(
                    "run",
                    json!({
                        "workspace": reference.name(),
                        "run": run,
                        "footprint_mb": summary_json(&sample.genea_mb),
                        "language_server_mb": summary_json(&sample.server_mb),
                        "language_server_processes": sample.servers,
                        "settled_after_ms": sample.settled.as_millis() as u64,
                        "window": { "width": sample.window.0, "height": sample.window.1, "scale": sample.window.2 },
                    }),
                );
                genea_mb.extend(sample.genea_mb);
                server_mb.extend(sample.server_mb);
                windows.push(sample.window);
                eprint!(".");
            }
            eprintln!();
            cx.record(
                "result",
                json!({
                    "workspace": reference.name(),
                    "runs": cx.options.memory_runs,
                    "footprint_mb": summary_json(&genea_mb),
                    "language_server_mb": summary_json(&server_mb),
                }),
            );
            let budget = reference.budget(budgets::MEMORY_TYPICAL, budgets::MEMORY_LARGE);
            let as_assumed = windows.iter().all(|&(w, h, scale)| (w, h) == BUDGET_WINDOW && scale == 2.0);
            checks.push(match (as_assumed, Summary::of(&genea_mb)) {
                (true, Some(s)) => budget.check(Some(s.p95)).with_note(format!("{} samples", genea_mb.len())),
                (false, _) => budget.skipped(&format!("the window wasn't 1200×800 pt at 2×: {windows:?}")),
                (true, None) => budget.check(None),
            });
            if reference.size == Size::Typical {
                let p95 = Summary::of(&server_mb).map(|s| s.p95);
                checks.push(match p95 {
                    Some(p95) => budgets::LSP_MEMORY.check(Some(p95)),
                    None => budgets::LSP_MEMORY.skipped("no language server ran"),
                });
            }
        }
        if cx.large.is_none() {
            checks.push(large_missing(budgets::MEMORY_LARGE));
        }
        Ok(checks)
    }
}

struct Sample {
    genea_mb: Vec<f64>,
    server_mb: Vec<f64>,
    servers: usize,
    settled: Duration,
    /// Width, height (pt) and scale.
    window: (f64, f64, f64),
}

fn sample(cx: &mut Context, reference: &Reference) -> Result<Sample, String> {
    let mut genea = cx.launch_on(reference)?;
    genea.wait_content()?;
    genea.resize(BUDGET_WINDOW.0, BUDGET_WINDOW.1)?;
    let info = require_visible(&mut genea)?;
    let started = Instant::now();
    wait_for_server(&mut genea)?;
    quiesce(&genea, Duration::from_secs(180))?;
    let settled = started.elapsed();
    sleep(SETTLE);

    let servers = language_servers(&mut genea)?;
    let pid = genea.pid();
    let (mut genea_mb, mut server_mb) = (Vec::new(), Vec::new());
    for _ in 0..cx.options.idle_seconds {
        genea_mb.push(sys::usage(pid).ok_or("Genea exited")?.footprint_mb());
        let server: f64 = servers.iter().filter_map(|&p| sys::usage(p)).map(|u| u.footprint_mb()).sum();
        if !servers.is_empty() {
            server_mb.push(server);
        }
        sleep(Duration::from_secs(1));
    }
    genea.quit();
    Ok(Sample {
        genea_mb,
        server_mb,
        servers: servers.len(),
        settled,
        window: (info.window_width, info.window_height, info.window_scale),
    })
}

/// Waits until the language server is ready, or Genea says it is off, for
/// up to a minute.
fn wait_for_server(genea: &mut Genea) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        if genea.check("server ready")? || genea.check("server off")? || genea.check("server failed")? {
            return Ok(());
        }
        sleep(Duration::from_millis(250));
    }
    Err("the language server wasn't ready after a minute".into())
}

/// Genea's child processes running tsgo.
pub fn language_servers(genea: &mut Genea) -> Result<Vec<u32>, String> {
    Ok(genea
        .processes()?
        .into_iter()
        .filter(|&pid| sys::executable(pid).is_some_and(|path| path.ends_with("lib/tsc")))
        .collect())
}
