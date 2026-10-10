//! Folder expansion in the Files view (ticket #63).
//!
//! On each reference workspace, Genea opens with its file and is left until
//! it has read the project's files. Then the workspace's folders are
//! expanded one after the other, parents first, and collapsed again in
//! reverse, `tree_rounds` times, as clicks on their rows do. An expansion
//! is timed from Genea dispatching it to the first frame showing the folder
//! expanded, as Genea's work (the wait for the display-link tick left out).
//! Collapses are timed the same way and reported.

use std::time::{Duration, Instant};

use serde_json::json;

use super::{Context, Reference, Scenario, large_missing, quiesce, reaction_ms, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    genea::{Genea, Met},
    journal::Window,
    stats::{Summary, summary_json},
    sys::now_ns,
};

pub struct FolderExpand;

const TOGGLE_TIMEOUT: Duration = Duration::from_secs(3);

impl Scenario for FolderExpand {
    fn name(&self) -> &'static str {
        "tree"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut checks = Vec::new();
        for reference in cx.references() {
            let expand = measure(cx, &reference)?;
            let p95 = Summary::of(&expand).map(|s| s.p95);
            checks.push(
                reference
                    .budget(budgets::TREE_EXPAND_TYPICAL, budgets::TREE_EXPAND_LARGE)
                    .check(p95)
                    .with_note(format!("{} expansions", expand.len())),
            );
        }
        if cx.large.is_none() {
            checks.push(large_missing(budgets::TREE_EXPAND_LARGE));
        }
        Ok(checks)
    }
}

/// Genea's work per expansion.
fn measure(cx: &mut Context, reference: &Reference) -> Result<Vec<f64>, String> {
    let mut genea = cx.launch_on(reference)?;
    genea.wait_content()?;
    require_visible(&mut genea)?;
    let settled = quiesce(&genea, Duration::from_secs(120))?;
    // Opening a file may have expanded its folders: start from a collapsed tree.
    for folder in reference.folders.iter().rev() {
        if genea.check(&format!("expanded {folder}"))? {
            toggle(&mut genea, folder, "collapsed")?;
        }
    }

    let from = now_ns();
    let (mut expanded, mut collapsed) = (Vec::new(), Vec::new());
    for _ in 0..cx.options.tree_rounds {
        for folder in reference.folders {
            wait_for_row(&mut genea, folder)?;
            expanded.push(toggle(&mut genea, folder, "expanded")?);
            sleep(Duration::from_millis(100));
        }
        for folder in reference.folders.iter().rev() {
            collapsed.push(toggle(&mut genea, folder, "collapsed")?);
            sleep(Duration::from_millis(100));
        }
    }
    let window = Window { from, to: now_ns() };
    let journal = genea.journal()?;
    genea.quit();

    let times = |toggles: &[(u64, Met)]| -> Result<(Vec<f64>, Vec<f64>), String> {
        let mut work = Vec::new();
        let mut present = Vec::new();
        for &(at, met) in toggles {
            let (w, p) = reaction_ms(&journal, at, met).ok_or("the journal has no frame showing the folder")?;
            work.push(w);
            present.push(p);
        }
        Ok((work, present))
    };
    let (expand, expand_present) = times(&expanded)?;
    let (collapse, _) = times(&collapsed)?;
    cx.record(
        "result",
        json!({
            "workspace": reference.name(),
            "folders": reference.folders,
            "rounds": cx.options.tree_rounds,
            "settled_after_ms": settled.as_millis() as u64,
            "expand_ms": summary_json(&expand),
            "expand_to_present_ms": summary_json(&expand_present),
            "collapse_ms": summary_json(&collapse),
            "stalls": journal.stalls(window).to_json(window.from),
        }),
    );
    Ok(expand)
}

/// Clicks a folder's row and waits for it to show `state` (`expanded` or
/// `collapsed`).
fn toggle(genea: &mut Genea, folder: &str, state: &str) -> Result<(u64, Met), String> {
    genea.expect("folder", &format!("{state} {folder}"))?;
    let at = genea.toggle_folder(folder)?;
    let met = genea.await_met("folder", TOGGLE_TIMEOUT).map_err(|e| format!("{folder} not {state}: {e}"))?;
    Ok((at, met))
}

/// Waits until the tree shows `folder`'s row, collapsed.
fn wait_for_row(genea: &mut Genea, folder: &str) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !genea.check(&format!("collapsed {folder}"))? {
        if Instant::now() > deadline {
            return Err(format!("the Files view doesn't show {folder}"));
        }
        sleep(Duration::from_millis(100));
    }
    Ok(())
}
