//! Start: warm, and cold after `purge`, each Genea run paired with a start
//! floor run in the same session.
//!
//! Genea opens the Typical workspace with [`super::FILE`]. Its start time is
//! process start (the kernel's) to *content visible*: the later of AppKit
//! reporting the window visible and the first frame with the file's content
//! presented. The floor's is process start to visible. Runs alternate floor,
//! Genea, floor, Genea, … so drift hits both, after one throwaway launch of
//! each for warm. The budgets limit Genea's p95 minus the floor's p95.
//!
//! Cold runs `sudo -n purge` and waits 2 s before every launch; they are
//! skipped when `sudo` would ask for a password (run `sudo -v` first).

use std::{process::Command, time::Duration};

use serde_json::{Value, json};

use super::{Context, Scenario, sleep};
use crate::{
    budgets::{self, Budget, Check},
    journal::{Window, ms},
    stats::{margin_over_floor, round, summary_json},
};

pub struct Start;

impl Scenario for Start {
    fn name(&self) -> &'static str {
        "start"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        // Throwaway launches, so warm really is warm.
        floor(cx)?;
        genea(cx)?;
        let mut checks = vec![series(cx, "warm", cx.options.warm_runs, false, budgets::WARM_START)?];
        checks.push(if !cx.options.cold {
            budgets::COLD_START.skipped("cold runs turned off (--no-cold)")
        } else if !can_purge() {
            budgets::COLD_START.skipped("needs passwordless sudo for `purge`: run `sudo -v` first")
        } else {
            series(cx, "cold", cx.options.cold_runs, true, budgets::COLD_START)?
        });
        Ok(checks)
    }
}

/// `runs` pairs of floor and Genea starts.
fn series(cx: &mut Context, kind: &str, runs: usize, cold: bool, budget: Budget) -> Result<Check, String> {
    let (mut floor_ms, mut genea_ms) = (Vec::new(), Vec::new());
    for run in 0..runs {
        if cold {
            purge()?;
        }
        let f = floor(cx)?;
        if cold {
            purge()?;
        }
        let g = genea(cx)?;
        floor_ms.push(f);
        genea_ms.push(g["content_visible_ms"].as_f64().unwrap_or(f64::NAN));
        cx.record("run", json!({ "start": kind, "run": run, "floor_visible_ms": f, "genea": g }));
        eprint!(".");
    }
    eprintln!();
    let margin = margin_over_floor(&genea_ms, &floor_ms);
    cx.record(
        "result",
        json!({
            "start": kind,
            "runs": runs,
            "margin": margin.map(|m| m.to_json()),
            "genea_content_visible_ms": summary_json(&genea_ms),
            "floor_visible_ms": summary_json(&floor_ms),
        }),
    );
    let Some(margin) = margin else { return Ok(budget.skipped("no runs")) };
    Ok(budget.check(Some(margin.p95_ms)).with_note(format!(
        "Genea p95 {} ms, floor p95 {} ms, {runs} runs each",
        round(margin.genea.p95),
        round(margin.floor.p95)
    )))
}

/// One floor launch: ms from process start to visible.
fn floor(cx: &Context) -> Result<f64, String> {
    let output = Command::new(&cx.floor).output().map_err(|e| format!("{}: {e}", cx.floor.display()))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let last = stdout.lines().last().unwrap_or_default();
    let value: Value = serde_json::from_str(last).map_err(|e| format!("floor printed {last:?}: {e}"))?;
    value["visible_ms"].as_f64().ok_or_else(|| format!("floor printed {last:?}"))
}

/// One Genea launch: its start milestones, in ms since process start.
fn genea(cx: &Context) -> Result<Value, String> {
    let mut genea = cx.launch(true)?;
    genea.wait_content()?;
    let journal = genea.journal()?;
    genea.quit();
    let start = journal.process_start;
    let since_start = |t: Option<u64>| t.map(|t| round(ms(t.saturating_sub(start))));
    let content_visible = journal.content_visible().ok_or("the journal has no content-visible milestone")?;
    let first_frame = journal.frames().first().map(|f| f.end);
    Ok(json!({
        "content_visible_ms": since_start(Some(content_visible)),
        "visible_ms": since_start(journal.visible),
        "first_frame_ms": since_start(first_frame),
        "content_synced_ms": since_start(journal.content),
        "main_ms": since_start(Some(journal.started)),
        "longest_stall_ms": round(journal.stalls(Window { from: 0, to: content_visible }).max_ms),
    }))
}

fn can_purge() -> bool {
    Command::new("sudo").args(["-n", "true"]).status().is_ok_and(|s| s.success())
}

/// Drops the file cache so binaries and libraries come from disk.
fn purge() -> Result<(), String> {
    let status = Command::new("sudo").args(["-n", "purge"]).status().map_err(|e| format!("purge: {e}"))?;
    if !status.success() {
        return Err(format!("purge failed: {status}"));
    }
    sleep(Duration::from_secs(2));
    Ok(())
}
