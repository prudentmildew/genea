//! The fuzzy finder (ticket #63): each keystroke's results.
//!
//! On each reference workspace, Genea opens with its file and is left until
//! it has read the project's files. Then the finder (Go to File) opens and
//! each of the workspace's queries is typed one key at a time through
//! Genea's real input path. Each key is timed from its arrival to the first
//! frame showing the results for the query so far (the finder no longer
//! matching), as Genea's work like a keystroke: the wait for the
//! display-link tick in between is left out. When the results for the
//! longer query are the same, no frame comes, and the time to the view
//! state counts.

use std::time::Duration;

use serde_json::json;

use super::{Context, Reference, Scenario, large_missing, quiesce, reaction_ms, require_visible, sleep};
use crate::{
    budgets::{self, Check},
    journal::Window,
    keys,
    stats::{Summary, summary_json},
    sys::now_ns,
};

pub struct FuzzyFinder;

/// Between keys: a quick typist.
const INTERVAL: Duration = Duration::from_millis(120);
/// A key whose results haven't shown by then is too slow to time.
const KEY_TIMEOUT: Duration = Duration::from_secs(3);

impl Scenario for FuzzyFinder {
    fn name(&self) -> &'static str {
        "finder"
    }

    fn run(&self, cx: &mut Context) -> Result<Vec<Check>, String> {
        let mut checks = Vec::new();
        for reference in cx.references() {
            let budget = reference.budget(budgets::FINDER_TYPICAL, budgets::FINDER_LARGE);
            let work = measure(cx, &reference)?;
            let p95 = Summary::of(&work).map(|s| s.p95);
            checks.push(budget.check(p95).with_note(format!("{} keys", work.len())));
        }
        if cx.large.is_none() {
            checks.push(large_missing(budgets::FINDER_LARGE));
        }
        Ok(checks)
    }
}

/// Types the queries `finder_rounds` times; Genea's work per key.
fn measure(cx: &mut Context, reference: &Reference) -> Result<Vec<f64>, String> {
    let mut genea = cx.launch_on(reference)?;
    genea.wait_content()?;
    let info = require_visible(&mut genea)?;
    let settled = quiesce(&genea, Duration::from_secs(120))?;

    let from = now_ns();
    let mut sent = Vec::new();
    for _ in 0..cx.options.finder_rounds {
        for query in reference.finder_queries {
            genea.finder("files")?;
            sleep(Duration::from_millis(300));
            for (i, c) in query.char_indices() {
                let key = keys::letter(c).ok_or_else(|| format!("no key for {c:?}"))?;
                let typed = &query[..i + c.len_utf8()];
                genea.expect("finder", &format!("finder {typed}"))?;
                let at = now_ns();
                genea.key(key)?;
                let met = genea.await_met("finder", KEY_TIMEOUT)?;
                sent.push((at, met));
                sleep(INTERVAL);
            }
            genea.finder("close")?;
            sleep(Duration::from_millis(200));
        }
    }
    let window = Window { from, to: now_ns() };
    let journal = genea.journal()?;
    genea.quit();

    let (mut work, mut present, mut unchanged) = (Vec::new(), Vec::new(), 0);
    for (at, met) in sent {
        let key = journal.key_after(at).ok_or("the journal has no key the harness pressed")?;
        let (w, p) = reaction_ms(&journal, key, met).ok_or("the journal has no frame showing the finder's results")?;
        work.push(w);
        present.push(p);
        unchanged += usize::from(met.unchanged);
    }
    cx.record(
        "result",
        json!({
            "workspace": reference.name(),
            "queries": reference.finder_queries,
            "rounds": cx.options.finder_rounds,
            "keys": work.len(),
            "unchanged_results": unchanged,
            "settled_after_ms": settled.as_millis() as u64,
            "display_hz": info.display_hz,
            "genea_work_ms": summary_json(&work),
            "key_to_present_ms": summary_json(&present),
            "stalls": journal.stalls(window).to_json(window.from),
        }),
    );
    Ok(work)
}
